//! HDF5 validation consumer of the public acquisition and provisional stream API.
use std::{fs, io::Write, path::Path, time::Instant};
use tile_ripper::{acquisition, cog::LayoutCache, Pixels, Request, Result};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let request: Request =
        serde_json::from_slice(&fs::read(args.get(1).ok_or("request required")?)?)?;
    let mode = args.get(2).map(String::as_str).unwrap_or("stream");
    let started = Instant::now();
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
    let inspection = if mode == "discard" {
        serde_json::Value::Null
    } else {
        prepared.explain()
    };
    let prepare_seconds = started.elapsed().as_secs_f64();
    if mode == "inspect" {
        println!("{inspection}");
        return Ok(());
    }
    if mode == "fetch" {
        let result = prepared.fetch()?;
        println!(
            "{}",
            serde_json::json!({"status":"complete","seconds":started.elapsed().as_secs_f64(),"inspection":inspection,"result":result})
        );
        return Ok(());
    }
    if mode == "before" {
        control()?;
    }
    let mut runs = Vec::new();
    for _ in 0..if mode == "warm" { 2 } else { 1 } {
        let mut stream = prepared
            .clone()
            .into_stream(tile_ripper::stream::StreamOptions { capacity: 0 })?;
        if mode == "cancel-inflight" {
            // The local test controller waits until a merged payload GET starts.
            control()?;
            stream.cancel();
            stream.finish()?;
            return Err("cancelled stream incorrectly returned completion".into());
        }
        let mut chunks = Vec::new();
        let mut first = None;
        let mut valid = 0u64;
        let mut sum = 0f64;
        for (i, chunk) in stream.by_ref().enumerate() {
            let c = chunk?;
            first.get_or_insert_with(|| started.elapsed().as_secs_f64());
            let mask = c.validity().map(u8::from).collect::<Vec<_>>();
            let bytes: Vec<_> = match &c.pixels {
                Pixels::UInt8(v) => v.clone(),
                Pixels::UInt32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                Pixels::UInt16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                Pixels::Int16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                Pixels::Float32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            };
            match &c.pixels {
                Pixels::UInt8(v) => {
                    for (v, m) in v.iter().zip(&mask) {
                        if *m != 0 {
                            sum += *v as f64;
                            valid += 1;
                        }
                    }
                }
                Pixels::UInt32(v) => {
                    for (v, m) in v.iter().zip(&mask) {
                        if *m != 0 {
                            sum += *v as f64;
                            valid += 1;
                        }
                    }
                }
                Pixels::UInt16(v) => {
                    for (v, m) in v.iter().zip(&mask) {
                        if *m != 0 {
                            sum += *v as f64;
                            valid += 1;
                        }
                    }
                }
                Pixels::Int16(v) => {
                    for (v, m) in v.iter().zip(&mask) {
                        if *m != 0 {
                            sum += *v as f64;
                            valid += 1;
                        }
                    }
                }
                Pixels::Float32(v) => {
                    for (v, m) in v.iter().zip(&mask) {
                        if *m != 0 {
                            sum += *v as f64;
                            valid += 1;
                        }
                    }
                }
            }
            if let Some(dir) = args.get(3) {
                if mode == "dump" {
                    fs::create_dir_all(dir)?;
                    fs::File::create(Path::new(dir).join(format!("{i}.bin")))?.write_all(&bytes)?;
                    fs::write(Path::new(dir).join(format!("{i}.mask")), &mask)?;
                }
            }
            if mode != "discard" {
                chunks.push(serde_json::json!({"asset_id":c.asset_id,"variable":c.variable,"indices":c.indices,"datetime":c.datetime,"window":c.window,"crs":c.crs,"transform":c.transform,"nodata":c.nodata.map(|v|if v.is_nan(){serde_json::json!("NaN")}else{serde_json::json!(v)}),"scale":c.scale,"offset":c.offset,"coordinates":c.coordinates,"bytes":bytes.len(),"adler32":adler(&bytes),"mask_adler32":adler(&mask)}));
            }
            if i == 0 {
                if mode == "cancel" {
                    stream.cancel();
                    break;
                }
                if mode == "during" {
                    control()?;
                }
            }
        }
        let report = stream.finish()?;
        runs.push(serde_json::json!({"chunks":chunks,"first_chunk_seconds":first,"valid_pixels":valid,"sum":sum,"observed":report}));
    }
    println!(
        "{}",
        serde_json::json!({"status":"complete","seconds":started.elapsed().as_secs_f64(),"prepare_seconds":prepare_seconds,"inspection":inspection,"runs":runs})
    );
    Ok(())
}
fn control() -> Result<()> {
    reqwest::blocking::get(std::env::var("TILE_RIPPER_TEST_CONTROL")?)?.error_for_status()?;
    Ok(())
}
fn adler(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for part in bytes.chunks(5552) {
        for v in part {
            a += u32::from(*v);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}
