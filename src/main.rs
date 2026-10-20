mod cli;
use std::{env, path::Path};
use tile_ripper::{acquisition, cog::LayoutCache, Format, Request, Result, Sink, Source};
fn run() -> Result<()> {
    let mut args: Vec<_> = env::args().collect();
    if let Some(command) = args.get_mut(1) {
        match command.as_str() {
            "stac-plan" => *command = "plan".into(),
            "stac-fetch" => *command = "fetch".into(),
            _ => {}
        }
    }
    let options = cli::parse(&args[1..])?;
    let Some(options) = options else {
        return Ok(());
    };
    let cli::Options {
        command,
        input,
        discover_only,
        human,
        cache_path,
        output,
        workers,
        per_host,
        backend,
        gdal_free,
    } = options;
    if let Some(backend) = backend {
        env::set_var("TILE_RIPPER_COG_BACKEND", backend.name());
    } else if gdal_free && cfg!(feature = "native-cog") {
        // Explicit GDAL-free operation selects native regardless of inherited settings.
        env::set_var("TILE_RIPPER_COG_BACKEND", "native");
    }
    if gdal_free {
        tile_ripper::cog::require_gdal_free()?;
    }
    if let Some(n) = workers {
        env::set_var("TILE_RIPPER_WORKERS", n.to_string());
    }
    if let Some(n) = per_host {
        env::set_var("TILE_RIPPER_PER_HOST", n.to_string());
    }
    // Quiet native driver diagnostics; they can include signed locators.
    #[cfg(feature = "gdal")]
    gdal::config::set_error_handler(|_, _, _| {});
    #[cfg(feature = "gdal")]
    for (key, value) in [
        ("GDAL_DISABLE_READDIR_ON_OPEN", "EMPTY_DIR"),
        ("GDAL_INGESTED_BYTES_AT_OPEN", "32768"),
        ("GDAL_NUM_THREADS", "1"),
        ("GDAL_CACHEMAX", "32"),
        ("GDAL_HTTP_MAX_RETRY", "3"),
        ("GDAL_HTTP_TIMEOUT", "45"),
        ("CPL_CURL_VERBOSE", "NO"),
        ("CPL_DEBUG", "OFF"),
    ] {
        // Explicit process settings take precedence over the CLI defaults.
        if env::var_os(key).is_none() {
            gdal::config::set_config_option(key, value)?;
        }
    }
    let mut cache = if let Some(path) = cache_path.as_deref().filter(|p| Path::new(p).exists()) {
        LayoutCache::load(Path::new(path))?
    } else {
        LayoutCache::default()
    };
    let mut request: Request = if input.get("source").is_some() {
        serde_json::from_value(input)
            .map_err(|_| "invalid request schema; see docs/unified-workflow.md")?
    } else {
        acquisition::from_stac(
            serde_json::from_value(input)
                .map_err(|_| "invalid request schema; see docs/unified-workflow.md")?,
        )
    };
    if let Some(path) = output {
        request.sink = match request.sink {
            Sink::ZarrDirectory { .. } => Sink::ZarrDirectory { path },
            Sink::GeoTiffDirectory { .. } => Sink::GeoTiffDirectory { path },
            _ => Sink::Directory { path },
        };
    }
    if command == "stream" {
        request.sink = Sink::Callback;
    }
    if cog_source(&request.source) {
        // Resolve the selected reader before discovery or source inspection.
        tile_ripper::cog::backend()?;
    }
    if command == "fetch" && !cfg!(feature = "gdal") && cog_source(&request.source) {
        return Err("COG/STAC fetch writes GeoTIFF and requires the gdal feature; use stream --gdal-free for chunk descriptors or the Rust Prepared::into_stream API for native values, or build with --features gdal for GeoTIFF output".into());
    }
    if discover_only && command != "plan" {
        return Err("--discover-only requires plan".into());
    }
    let destination = match &request.sink {
        Sink::Directory { path }
        | Sink::ZarrDirectory { path }
        | Sink::GeoTiffDirectory { path } => Some(std::path::absolute(path)?),
        Sink::Callback => None,
    };
    if command == "fetch" && destination.as_ref().is_some_and(|path| path.exists()) {
        return Err("output directory already exists; choose --output NEW_DIRECTORY".into());
    }
    if human {
        eprintln!("Inspecting source metadata and native selection…");
    }
    let value = if discover_only {
        acquisition::discover(&request)?
    } else {
        let prepared = acquisition::prepare(&request, &mut cache)?;
        match command.as_str() {
            "plan" => {
                if human {
                    let result = prepared.human();
                    if let Some(path) = cache_path {
                        cache.save(Path::new(&path))?;
                    }
                    println!("{result}");
                    return Ok(());
                }
                prepared.explain()
            }
            "stream" => {
                use std::io::Write;
                let started = std::time::Instant::now();
                let mut stream = prepared.into_stream(Default::default())?;
                let stdout = std::io::stdout();
                let mut out = stdout.lock();
                for chunk in stream.by_ref() {
                    let c = chunk?;
                    writeln!(
                        out,
                        "{}",
                        serde_json::json!({"type":"chunk","asset_id":c.asset_id,"node_id":c.node_id,"variable":c.variable,"datetime":c.datetime,"indices":c.indices,"window":c.window,"crs":c.crs,"transform":c.transform,"nodata":c.nodata,"scale":c.scale,"offset":c.offset,"dtype":if c.pixels.as_f32().is_some(){"float32"}else{"uint16"},"bytes":c.pixels.byte_len(),"valid_pixels":c.validity().filter(|v|*v).count(),"seconds":started.elapsed().as_secs_f64()})
                    )?;
                }
                let report = stream.finish()?;
                writeln!(
                    out,
                    "{}",
                    serde_json::json!({"type":"complete","observed":report})
                )?;
                return Ok(());
            }
            "fetch" => {
                if human {
                    eprintln!("Fetching native pixels; validating source identities…");
                }
                prepared.fetch()?
            }
            _ => return Err("expected plan, fetch or stream".into()),
        }
    };
    if let Some(path) = cache_path {
        cache.save(Path::new(&path))?;
    }
    if human && command == "fetch" {
        println!("Acquisition complete: {} output records.\nOutput: {}\nCompletion and provenance: manifest.json", value["outputs"].as_array().map_or(0, |a| a.len()), destination.as_ref().map_or_else(|| "callback".into(), |p|p.display().to_string()));
        let transport = &value["observed"]["transport"];
        if let Some((requests, bytes)) = cli::transport_totals(transport) {
            println!("Observed HTTP: {requests} requests, {bytes} response-body bytes (metadata and validation included).");
        } else {
            println!("Observed HTTP totals unavailable for this backend; manifest records measured stages and planned reads separately.");
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}
fn cog_source(source: &Source) -> bool {
    match source {
        Source::Stac { .. } | Source::Catalog { .. } => true,
        Source::Assets { assets } => assets.iter().any(|asset| asset.format == Format::Cog),
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("tile-ripper: {e}");
        std::process::exit(1);
    }
}
