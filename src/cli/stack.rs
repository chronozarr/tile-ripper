use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tile_ripper::{stack, Result};
const HELP: &str = r#"tile-ripper stack INPUT.zarr OUTPUT.zarr [--chunk-edge N] [--buffer-mib N]
tile-ripper stack INPUT.zarr --plan
  Stack a completed fetch output: observations of one collection on one exact
  native grid, dtype, nodata and calibration share a [time, band, y, x] axis.
  No resampling, mosaic or composite; ambiguous series are refused with reasons.
  --plan          print stack keys, refusals and calibration breaks; write nothing
  --chunk-edge N  spatial chunk edge of the new store (default 512)
  --buffer-mib N  pixel/decoded-chunk/codec reservation (default 64; not process RSS)
  The input is never modified. See docs/0004-same-grid-time-stacking.md."#;
pub fn run(args: &[String]) -> Result<()> {
    if args.is_empty() || args.iter().any(|s| s == "--help" || s == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    let input = Path::new(&args[0]);
    if args[1..] == ["--plan"] {
        println!("{}", serde_json::to_string_pretty(&stack::plan(input)?)?);
        return Ok(());
    }
    let output = args
        .get(1)
        .filter(|s| !s.starts_with('-'))
        .ok_or_else(|| format!("stack needs an output path\n{HELP}"))?;
    let mut options = stack::Options::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut rest = args[2..].iter();
    while let Some(flag) = rest.next() {
        if !seen.insert(flag) {
            return Err(format!("duplicate stack option {flag}").into());
        }
        let text = rest
            .next()
            .ok_or_else(|| format!("{flag} requires a positive integer\n{HELP}"))?;
        let n: u64 = text
            .parse()
            .map_err(|e| format!("{flag} {text}: expected a positive integer ({e})"))?;
        if n == 0 {
            return Err(format!("{flag} must be positive").into());
        }
        match flag.as_str() {
            "--chunk-edge" => options.chunk_edge = n,
            "--buffer-mib" => {
                options.buffer_bytes = n
                    .checked_mul(1024 * 1024)
                    .ok_or("--buffer-mib byte count overflow")?
            }
            _ => return Err(format!("unknown stack option {flag}\n{HELP}").into()),
        }
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::Release))?;
    let manifest = stack::write_with_options(input, Path::new(output), options, cancelled)?;
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}
