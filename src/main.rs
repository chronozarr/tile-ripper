use std::{env, fs, path::Path};
use tile_ripper::{acquisition, cog::LayoutCache, Request, Result, Sink};
fn run() -> Result<()> {
    let mut args: Vec<_> = env::args().collect();
    if let Some(command) = args.get_mut(1) {
        match command.as_str() {
            "stac-plan" => *command = "plan".into(),
            "stac-fetch" => *command = "fetch".into(),
            _ => {}
        }
    }
    if args.get(1).is_some_and(|a| a == "--help" || a == "-h") {
        println!("tile-ripper plan|fetch request.json [--human|--json] [--output new-directory] [--cache layout.json] [--discover-only]\nLegacy aliases: stac-plan, stac-fetch. See docs/unified-workflow.md.");
        return Ok(());
    }
    if args.len() < 3 {
        return Err("usage: tile-ripper plan|fetch|stac-plan|stac-fetch request.json [--human|--json] [--discover-only] [--cache layout.json] [--output new-directory]".into());
    }
    let mut discover_only = false;
    let mut human = false;
    let mut cache_path = None;
    let mut output = None;
    let mut i = 3;
    while i < args.len() {
        if args[i] == "--human" || args[i] == "--json" {
            human = args[i] == "--human";
            i += 1;
            continue;
        }
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
    if !matches!(args[1].as_str(), "plan" | "fetch") {
        return Err("expected plan or fetch".into());
    }
    let input = fs::read(&args[2]).map_err(|_| "cannot read request file")?;
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
    let json: serde_json::Value =
        serde_json::from_slice(&input).map_err(|_| "invalid request JSON syntax")?;
    let mut request: Request = if json.get("source").is_some() {
        serde_json::from_value(json)
            .map_err(|_| "invalid request schema; see docs/unified-workflow.md")?
    } else {
        acquisition::from_stac(
            serde_json::from_value(json)
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
    if discover_only && args[1] != "plan" {
        return Err("--discover-only requires plan".into());
    }
    let value = if discover_only {
        acquisition::discover(&request)?
    } else {
        let prepared = acquisition::prepare(&request, &mut cache)?;
        match args[1].as_str() {
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
            "fetch" => prepared.fetch()?,
            _ => return Err("expected plan or fetch".into()),
        }
    };
    if let Some(path) = cache_path {
        cache.save(Path::new(&path))?;
    }
    if human && args[1] == "fetch" {
        println!(
            "Acquisition complete: {} output records. See manifest.json in the output directory.",
            value["outputs"].as_array().map_or(0, |a| a.len())
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("tile-ripper: {e}");
        std::process::exit(1);
    }
}
