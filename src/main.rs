use std::{env, fs, path::Path};
use tile_ripper::{cog::LayoutCache, fetch, plan, Request, Result, Sink};
fn run() -> Result<()> {
    let args: Vec<_> = env::args().collect();
    if args.len() < 3 {
        return Err("usage: tile-ripper plan|fetch|stac-plan|stac-fetch request.json [--discover-only] [--cache layout.json] [--output new-directory]".into());
    }
    let mut discover_only = false;
    let mut cache_path = None;
    let mut output = None;
    let mut i = 3;
    while i < args.len() {
        if args[i] == "--discover-only" {
            discover_only = true;
            i += 1;
            continue;
        }
        if i + 1 >= args.len() {
            return Err("option requires a value".into());
        }
        match args[i].as_str() {
            "--cache" => cache_path = Some(args[i + 1].clone()),
            "--output" => output = Some(args[i + 1].clone()),
            _ => return Err("unknown option".into()),
        };
        i += 2;
    }
    let input = fs::read(&args[2])?;
    // Quiet native driver diagnostics; they can include signed locators.
    gdal::config::set_error_handler(|_, _, _| {});
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
        gdal::config::set_config_option(key, value)?;
    }
    let mut cache = if let Some(path) = cache_path.as_deref().filter(|p| Path::new(p).exists()) {
        LayoutCache::load(Path::new(path))?
    } else {
        LayoutCache::default()
    };
    if args[1] == "stac-plan" || args[1] == "stac-fetch" {
        let mut request: tile_ripper::stac_workflow::StacRequest = serde_json::from_slice(&input)?;
        if let Some(path) = output {
            request.output = path;
        }
        let discovered = tile_ripper::stac_workflow::discover(&request)?;
        let value = if discover_only {
            if args[1] != "stac-plan" {
                return Err("--discover-only requires stac-plan".into());
            }
            serde_json::json!({"discovery":discovered.report,"sources":discovered.sources(),"raster_metadata_opened":false})
        } else {
            let prepared = discovered.prepare(&mut cache)?;
            if args[1] == "stac-fetch" {
                prepared.fetch()?
            } else {
                prepared.explain()
            }
        };
        if let Some(path) = cache_path {
            cache.save(Path::new(&path))?;
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if discover_only {
        return Err("--discover-only requires stac-plan".into());
    }
    let mut request: Request = serde_json::from_slice(&input)?;
    if let Some(path) = output {
        request.sink = Sink::GeoTiffDirectory { path };
    }
    let value = match args[1].as_str() {
        "plan" => serde_json::to_value(plan(&request, &mut cache)?)?,
        "fetch" => serde_json::to_value(fetch(&request, &mut cache)?)?,
        _ => return Err("expected plan or fetch".into()),
    };
    if let Some(path) = cache_path {
        cache.save(Path::new(&path))?;
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("tile-ripper: {e}");
        std::process::exit(1);
    }
}
