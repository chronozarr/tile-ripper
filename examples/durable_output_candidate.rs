//! Benchmark driver using the public lifecycle. No production environment knobs.
use std::sync::{atomic::AtomicBool, Arc};
use tile_ripper::{acquisition, cog::LayoutCache, ScientificEncoding, ScientificOutputOptions};
fn main() -> tile_ripper::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: durable_output_candidate request.json raw|gzip|blosc_zstd|sharded output_chunk_edge".into());
    }
    let encoding = match args[2].as_str() {
        "raw" => ScientificEncoding::Uncompressed,
        "gzip" => ScientificEncoding::Gzip,
        "blosc_zstd" => ScientificEncoding::BloscZstd,
        #[cfg(feature = "experimental-output")]
        "sharded" => ScientificEncoding::ExperimentalV3ShardedGzip,
        _ => {
            return Err("unknown encoding, or sharding needs --features experimental-output".into())
        }
    };
    let request = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let prepared = acquisition::prepare_gdal_free(&request, &mut LayoutCache::default())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    ctrlc::set_handler(move || flag.store(true, std::sync::atomic::Ordering::Release))?;
    let value = prepared.fetch_scientific_with_options(
        ScientificOutputOptions {
            encoding,
            chunk_edge: args[3].parse()?,
        },
        cancelled,
    )?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
