//! Public quality-aware consumer. Dump files are provisional validation data;
//! the completion JSON is printed only after successful finish().
#[cfg(feature = "native-hdf5")]
fn main() -> tile_ripper::Result<()> {
    use std::{fs, path::Path, time::Instant};
    use tile_ripper::{cog::LayoutCache, swot, Pixels, Request};
    let args: Vec<_> = std::env::args().collect();
    let r: Request = serde_json::from_slice(&fs::read(args.get(1).ok_or("request required")?)?)?;
    let policy: swot::Policy =
        serde_json::from_slice(&fs::read(args.get(2).ok_or("policy required")?)?)?;
    let mode = args.get(3).map(String::as_str).unwrap_or("stream");
    let started = Instant::now();
    let prepared = swot::prepare(&r, policy, &mut LayoutCache::default())?;
    let inspection = prepared.explain();
    let prepare_seconds = started.elapsed().as_secs_f64();
    if mode == "inspect" {
        println!("{inspection}");
        return Ok(());
    }
    let control = || -> tile_ripper::Result<()> {
        reqwest::blocking::get(std::env::var("TILE_RIPPER_TEST_CONTROL")?)?.error_for_status()?;
        Ok(())
    };
    if mode == "before" {
        control()?;
    }
    let mut stream = prepared.into_stream()?;
    if mode == "cancel-inflight" {
        control()?;
        stream.cancel();
        stream.finish()?;
        return Err("cancel accepted".into());
    }
    let (
        mut count,
        mut sum,
        mut uncertainty_count,
        mut uncertainty_sum,
        mut pixels_count,
        mut pixels_sum,
    ) = (0u64, 0f64, 0u64, 0f64, 0u64, 0u64);
    let mut tiles = Vec::new();
    let mut first = None;
    for (i, tile) in stream.by_ref().enumerate() {
        let tile = tile?;
        first.get_or_insert_with(|| started.elapsed().as_secs_f64());
        let mut descriptions = Vec::new();
        for (j, c) in tile.layers.iter().enumerate() {
            let bytes: Vec<u8> = match &c.pixels {
                Pixels::UInt8(v) => v.clone(),
                Pixels::UInt16(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                Pixels::UInt32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
                Pixels::Float32(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            };
            let mask = c.validity().map(u8::from).collect::<Vec<_>>();
            if mode == "dump" {
                let dir = Path::new(args.get(4).ok_or("dump directory required")?);
                fs::create_dir_all(dir)?;
                fs::write(dir.join(format!("{i}-{j}.bin")), &bytes)?;
                fs::write(dir.join(format!("{i}-{j}.mask")), &mask)?;
                if j == 0 {
                    fs::write(dir.join(format!("{i}.validity")), &tile.validity)?;
                }
            }
            descriptions.push(serde_json::json!({"variable":c.variable,"window":c.window,"coordinates":c.coordinates,"nodata":c.nodata,"scale":c.scale,"offset":c.offset,"bytes":bytes.len()}));
        }
        let wse = tile.layers[0].pixels.as_f32().ok_or("WSE dtype")?;
        let uncert = tile.layers[3].pixels.as_f32().ok_or("uncertainty dtype")?;
        let n = tile.layers[4].pixels.as_u32().ok_or("count dtype")?;
        let um = tile.layers[3].validity().collect::<Vec<_>>();
        let nm = tile.layers[4].validity().collect::<Vec<_>>();
        for k in 0..wse.len() {
            if tile.validity[k] != 0 {
                count += 1;
                sum += f64::from(wse[k]);
                if um[k] && uncert[k].is_finite() {
                    uncertainty_count += 1;
                    uncertainty_sum += f64::from(uncert[k]);
                }
                if nm[k] {
                    pixels_count += 1;
                    pixels_sum += u64::from(n[k]);
                }
            }
        }
        if mode != "discard" {
            tiles.push(serde_json::json!({"layers":descriptions}));
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
    println!(
        "{}",
        serde_json::json!({"status":"complete","prepare_seconds":prepare_seconds,"first_result_seconds":first,"completion_seconds":started.elapsed().as_secs_f64(),"inspection":inspection,"tiles":tiles,"observed":report,"summary":{"selected_count":count,"wse_sum_m":sum,"wse_mean_m":if count>0{Some(sum/count as f64)}else{None},"uncertainty_count":uncertainty_count,"uncertainty_sum_m":uncertainty_sum,"mean_pixel_uncertainty_m":if uncertainty_count>0{Some(uncertainty_sum/uncertainty_count as f64)}else{None},"contributing_count":pixels_count,"contributing_pixel_sum":pixels_sum}})
    );
    Ok(())
}
#[cfg(not(feature = "native-hdf5"))]
fn main() {
    eprintln!("build with native-hdf5");
    std::process::exit(1);
}
