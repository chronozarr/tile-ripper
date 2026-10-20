//! Benchmark-only direct sample comparison; expected little-endian bytes come
//! from the independent rasterio oracle, outside acquisition timing.
use std::io::Read;
use tile_ripper::{acquisition, cog::LayoutCache, Request, Result};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("native_cog_oracle request.json expected-u16-le.bin".into());
    }
    let request: Request = serde_json::from_slice(&std::fs::read(&args[0])?)?;
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
    let mut expected = std::io::BufReader::new(std::fs::File::open(&args[1])?);
    let mut samples = 0usize;
    let report = prepared.execute(|chunk| {
        let values = chunk.pixels.as_u16().ok_or("oracle requires UInt16")?;
        let mut bytes = vec![0; values.len() * 2];
        expected.read_exact(&mut bytes)?;
        for (value, pair) in values.iter().zip(bytes.chunks_exact(2)) {
            if *value != u16::from_le_bytes([pair[0], pair[1]]) {
                return Err("native sample differs from independent rasterio oracle".into());
            }
        }
        samples += values.len();
        Ok(())
    })?;
    if expected.read(&mut [0])? != 0 {
        return Err("native coverage omitted expected values".into());
    }
    println!(
        "{}",
        serde_json::json!({"samples_compared_bit_for_bit":samples,"chunks":report.chunks,"complete":true})
    );
    Ok(())
}
