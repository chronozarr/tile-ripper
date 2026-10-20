//! Independent-oracle bridge using only the public streaming acquisition API.
//!
//! `native_sentinel_oracle request.json prefix` writes prefix.ndjson metadata and
//! prefix.u16le exact row-major stored values. `request.json --compare prefix`
//! compares both against an earlier independent receipt. Output remains
//! provisional until `finish()` validates every source version.
use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
};
use tile_ripper::{acquisition, cog::LayoutCache, stream::StreamOptions, Request, Result};

fn path(prefix: &str, suffix: &str) -> String {
    format!("{prefix}.{suffix}")
}
fn emit(out: &mut impl Write, value: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (prefix, comparing) = match args.as_slice() {
        [_, prefix] => (prefix.as_str(), false),
        [_, flag, prefix] if flag == "--compare" => (prefix.as_str(), true),
        _ => return Err("usage: native_sentinel_oracle request.json [--compare] prefix".into()),
    };
    let request: Request = serde_json::from_slice(&std::fs::read(&args[0])?)?;
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
    let mut stream = prepared.into_stream(StreamOptions { capacity: 0 })?;
    let mut expected_metadata = if comparing {
        let rows = BufReader::new(File::open(path(prefix, "ndjson"))?)
            .lines()
            .map(|line| -> Result<serde_json::Value> { Ok(serde_json::from_str(&line?)?) })
            .collect::<Result<Vec<_>>>()?;
        if rows.last().is_none_or(|row| row["type"] != "complete") {
            return Err("independent oracle metadata lacks a final completion receipt".into());
        }
        rows.into_iter()
            .filter(|v| v["type"] == "provisional_chunk")
            .collect::<Vec<_>>()
            .into_iter()
    } else {
        Vec::new().into_iter()
    };
    let mut expected_bytes = comparing
        .then(|| File::open(path(prefix, "u16le")).map(BufReader::new))
        .transpose()?;
    let mut output_bytes = if comparing {
        None
    } else {
        Some(BufWriter::new(File::create(path(prefix, "u16le"))?))
    };
    let mut metadata: Box<dyn Write> = if comparing {
        Box::new(std::io::stdout())
    } else {
        Box::new(BufWriter::new(File::create(path(prefix, "ndjson"))?))
    };
    emit(
        &mut metadata,
        &serde_json::json!({"type":"provisional_start","provenance":stream.provenance}),
    )?;
    let (mut samples, mut chunks, mut valid) = (0usize, 0usize, 0usize);
    for item in stream.by_ref() {
        let c = item?;
        let values = c.pixels.as_u16().ok_or("oracle requires UInt16")?;
        if values.len() != c.window.width * c.window.height {
            return Err("chunk shape differs from exact UInt16 array".into());
        }
        let n = c.validity().filter(|v| *v).count();
        let row = serde_json::json!({
            "type":"provisional_chunk", "asset_id":c.asset_id,
            "datetime":c.datetime,"variable":c.variable,"indices":c.indices,
            "window":c.window,"transform":c.transform,"crs":c.crs,
            "nodata":c.nodata,"scale":c.scale,"offset":c.offset,
            "coordinates":c.coordinates,"dtype":"uint16", "samples":values.len(),
            "data_offset_bytes":samples*2,"valid_pixels":n,
            "first_pixel_center":c.pixel_center(0,0),
            "last_pixel_center":c.pixel_center(c.window.width-1,c.window.height-1)
        });
        if comparing && expected_metadata.next().as_ref() != Some(&row) {
            return Err(format!("chunk {chunks} metadata differs from independent oracle").into());
        }
        for value in values {
            let bytes = value.to_le_bytes();
            if let Some(expected) = &mut expected_bytes {
                let mut reference = [0u8; 2];
                expected.read_exact(&mut reference)?;
                if reference != bytes {
                    return Err(
                        format!("stored UInt16 sample {} differs from oracle", samples).into(),
                    );
                }
            } else if let Some(out) = &mut output_bytes {
                out.write_all(&bytes)?;
            }
            samples += 1;
        }
        valid += n;
        chunks += 1;
        emit(&mut metadata, &row)?;
    }
    let report = stream.finish()?;
    if report.chunks != chunks || report.non_nodata_pixels != valid {
        return Err("stream totals differ from final completion report".into());
    }
    if expected_metadata.next().is_some()
        || expected_bytes
            .as_mut()
            .map(|r| r.read(&mut [0u8; 1]))
            .transpose()?
            .is_some_and(|n| n != 0)
    {
        return Err("native stream omitted expected metadata or stored values".into());
    }
    if let Some(out) = &mut output_bytes {
        out.flush()?;
    }
    emit(
        &mut metadata,
        &serde_json::json!({"type":"complete","chunks":chunks,"samples":samples,
            "valid_pixels":valid,"compared":comparing,"observed":report}),
    )
}
fn main() {
    if let Err(error) = run() {
        eprintln!("native Sentinel oracle aborted: {error}");
        std::process::exit(1);
    }
}
