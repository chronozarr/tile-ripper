//! Standalone native consumer: no intermediate files, no reopen or conversion.
//! cargo run --release --locked --example chunk_stream -- request.json
use tile_ripper::{acquisition, cog::LayoutCache, Pixels, Request, Result};
fn main() -> Result<()> {
    let request: Request = serde_json::from_slice(&std::fs::read(
        std::env::args().nth(1).ok_or("request JSON required")?,
    )?)?;
    let with_digests = std::env::args().skip(2).any(|v| v == "--digests");
    let mut digests = Vec::new();
    let started = std::time::Instant::now();
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
    let mut stream = prepared.into_stream(Default::default())?;
    let mut first = None;
    let mut sum = 0f64;
    let mut valid = 0usize;
    for chunk in stream.by_ref() {
        let c = chunk?;
        first.get_or_insert_with(|| started.elapsed().as_secs_f64());
        // Consume the original native dtype and explicit missingness policy.
        let values: Box<dyn Iterator<Item = f64>> = match &c.pixels {
            Pixels::Int8(v) => Box::new(v.iter().map(|v| *v as f64)),
            Pixels::UInt8(v) => Box::new(v.iter().map(|v| *v as f64)),
            Pixels::UInt32(v) => Box::new(v.iter().map(|v| *v as f64)),
            Pixels::UInt16(v) => Box::new(v.iter().map(|v| *v as f64)),
            Pixels::Int16(v) => Box::new(v.iter().map(|v| *v as f64)),
            Pixels::Float32(v) => Box::new(v.iter().map(|v| *v as f64)),
            Pixels::Float64(v) => Box::new(v.iter().copied()),
        };
        for (v, keep) in values.zip(c.validity()) {
            if keep {
                sum += v;
                valid += 1;
            }
        }
        if with_digests {
            let digest = match &c.pixels {
                Pixels::Int8(v) => adler(v.iter().map(|v| *v as u8)),
                Pixels::UInt8(v) => adler(v.iter().copied()),
                Pixels::UInt32(v) => adler(v.iter().flat_map(|v| v.to_le_bytes())),
                Pixels::UInt16(v) => adler(v.iter().flat_map(|v| v.to_le_bytes())),
                Pixels::Int16(v) => adler(v.iter().flat_map(|v| v.to_le_bytes())),
                Pixels::Float32(v) => adler(v.iter().flat_map(|v| v.to_le_bytes())),
                Pixels::Float64(v) => adler(v.iter().flat_map(|v| v.to_le_bytes())),
            };
            let coordinate_digest = c
                .coordinates
                .as_ref()
                .map(|(x, y)| adler(x.iter().chain(y).flat_map(|v| v.to_le_bytes())));
            digests.push(serde_json::json!({"asset_id":c.asset_id,"variable":c.variable,"datetime":c.datetime,"indices":c.indices,"window":c.window,"transform":c.transform,"crs":c.crs,"adler32":digest,"coordinate_adler32":coordinate_digest}));
        }
        // c.pixel_center(column,row) uses actual coordinates when necessary.
    }
    let report = stream.finish()?; // Includes final source-version checks.
    println!(
        "{}",
        serde_json::json!({"status":"complete","first_chunk_seconds":first,"seconds":started.elapsed().as_secs_f64(),"valid_pixels":valid,"native_sum":sum,"observed":report,"digests":digests})
    );
    Ok(())
}

// Standard Adler-32 over stored little-endian bits; benchmark-only checksum.
fn adler(bytes: impl IntoIterator<Item = u8>) -> u32 {
    let (mut a, mut b, mut count) = (1u32, 0u32, 0usize);
    for byte in bytes {
        a += u32::from(byte);
        b += a;
        count += 1;
        if count == 5552 {
            a %= 65521;
            b %= 65521;
            count = 0;
        }
    }
    ((b % 65521) << 16) | (a % 65521)
}
