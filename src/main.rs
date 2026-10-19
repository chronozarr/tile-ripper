use std::{env, fs, path::Path};
use tile_ripper::{cog::LayoutCache, fetch, plan, Request, Result, Sink};
fn run() -> Result<()> {
    let args: Vec<_> = env::args().collect();
    if args.len() < 3 {
        return Err("usage: tile-ripper plan|fetch request.json [--cache layout.json] [--output new-directory]".into());
    }
    let mut cache_path = None;
    let mut output = None;
    let mut i = 3;
    while i < args.len() {
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
    let mut request: Request = serde_json::from_slice(&fs::read(&args[2])?)?;
    if let Some(path) = output {
        request.sink = Sink::GeoTiffDirectory { path };
    }
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
