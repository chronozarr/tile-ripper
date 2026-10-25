//! External public-API consumer. NDJSON chunks are provisional; only the last
//! successful `complete` record commits the aggregate. No pixel arrays retained.
use std::io::{self, Write};
use tile_ripper::{acquisition, cog::LayoutCache, stream::StreamOptions, Pixels, Request, Result};

fn emit(value: serde_json::Value) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &value)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
fn option(args: &[String], flag: &str) -> Result<Option<usize>> {
    args.iter()
        .position(|a| a == flag)
        .map(|i| {
            args.get(i + 1)
                .ok_or("option requires a number")?
                .parse::<usize>()
                .map_err(Into::into)
        })
        .transpose()
}
fn adler(bytes: impl IntoIterator<Item = u8>) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = args
        .first()
        .ok_or("request.json [--cancel-after N] [--pause-after N] [--max-chunk-bytes N]")?;
    let cancel = option(&args, "--cancel-after")?;
    let pause = option(&args, "--pause-after")?;
    let max_chunk = option(&args, "--max-chunk-bytes")?.unwrap_or(2 * 1024 * 1024);
    let request: Request = serde_json::from_slice(&std::fs::read(path)?)?;
    let started = std::time::Instant::now();
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
    let prepare_seconds = started.elapsed().as_secs_f64();
    let mut stream = prepared.into_stream(StreamOptions { capacity: 0 })?;
    emit(
        serde_json::json!({"type":"provisional_start", "prepare_seconds":prepare_seconds,
        "provenance":stream.provenance, "max_chunk_bytes":max_chunk}),
    )?;
    let (mut chunks, mut valid, mut sum) = (0usize, 0usize, 0f64);
    let mut first = None;
    while let Some(item) = stream.next() {
        let c = item?;
        first.get_or_insert_with(|| started.elapsed().as_secs_f64());
        let bytes = c.pixels.byte_len()
            + c.coordinates
                .as_ref()
                .map_or(0, |(x, y)| (x.len() + y.len()) * 8);
        if bytes > max_chunk {
            return Err("delivered chunk exceeds consumer payload budget".into());
        }
        if c.pixels.len() != c.window.width * c.window.height {
            return Err("native chunk shape mismatch".into());
        }
        let (dtype, digest) = match &c.pixels {
            Pixels::Int8(v) => ("int8", adler(v.iter().map(|v| *v as u8))),
            Pixels::UInt8(v) => ("uint8", adler(v.iter().copied())),
            Pixels::UInt32(v) => ("uint32", adler(v.iter().flat_map(|v| v.to_le_bytes()))),
            Pixels::Int16(v) => ("int16", adler(v.iter().flat_map(|v| v.to_le_bytes()))),
            Pixels::UInt16(v) => ("uint16", adler(v.iter().flat_map(|v| v.to_le_bytes()))),
            Pixels::Float32(v) => ("float32", adler(v.iter().flat_map(|v| v.to_le_bytes()))),
        };
        let (mut n, mut total) = (0usize, 0f64);
        for (i, keep) in c.validity().enumerate() {
            if keep {
                n += 1;
                total += match &c.pixels {
                    Pixels::Int8(v) => f64::from(v[i]),
                    Pixels::UInt8(v) => f64::from(v[i]),
                    Pixels::UInt32(v) => f64::from(v[i]),
                    Pixels::Int16(v) => f64::from(v[i]),
                    Pixels::UInt16(v) => f64::from(v[i]),
                    Pixels::Float32(v) => f64::from(v[i]),
                };
            }
        }
        chunks += 1;
        valid += n;
        sum += total;
        emit(
            serde_json::json!({"type":"provisional_chunk", "node_id":c.node_id,
            "asset_id":c.asset_id, "variable":c.variable,"datetime":c.datetime,"indices":c.indices,
            "window":c.window,"transform":c.transform,"crs":c.crs,"coordinates":c.coordinates,
            "dtype":dtype,"nodata":c.nodata,"nodata_is_nan":c.nodata.is_some_and(f64::is_nan),
            "scale":c.scale,"offset":c.offset,"valid_pixels":n,"native_sum":total,
            "adler32_le":digest,"payload_bytes":bytes}),
        )?;
        // Pause permits an external test to mutate an already-read source while
        // backpressure prevents the producer from completing the acquisition.
        if pause == Some(chunks) {
            let mut line = String::new();
            if io::stdin().read_line(&mut line)? == 0 {
                return Err("pause acknowledgement required".into());
            }
        }
        if cancel == Some(chunks) {
            stream.cancel();
        }
    }
    let report = stream.finish()?;
    if chunks != report.chunks || valid != report.non_nodata_pixels {
        return Err("consumer totals disagree with completion receipt".into());
    }
    emit(
        serde_json::json!({"type":"complete","chunks":chunks,"valid_pixels":valid,
        "native_sum":sum,"first_chunk_seconds":first,"prepare_seconds":prepare_seconds,
        "seconds":started.elapsed().as_secs_f64(),"observed":report}),
    )
}
fn main() {
    if let Err(error) = run() {
        let _ = emit(serde_json::json!({"type":"aborted","error":error.to_string()}));
        std::process::exit(1);
    }
}
