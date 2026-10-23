//! Bounded acceptance adapter; normal scientists use discover-job/plan/fetch.
#[cfg(feature = "native-hdf5")]
fn main() -> tile_ripper::Result<()> {
    use serde_json::{json, Value};
    use tile_ripper::{swot_aoi, swot_job::Observation, Spatial};
    let path = std::env::args().nth(1).ok_or("oracle JSON path required")?;
    let bytes = std::fs::read(path)?;
    if bytes.len() > 1024 * 1024 {
        return Err("oracle input exceeds bound".into());
    }
    let input: Value = serde_json::from_slice(&bytes)?;
    let aoi: Spatial = serde_json::from_value(input["aoi"].clone())?;
    let output = if input["mode"] == "inspect" {
        let entry: Observation = serde_json::from_value(input["observation"].clone())?;
        let (entry, report) = swot_aoi::inspect(entry, &aoi)?;
        json!({"observation":entry,"report":report})
    } else {
        let crs = input["crs"].as_str().ok_or("crs required")?;
        let y: Vec<f64> = serde_json::from_value(input["y"].clone())?;
        let x: Vec<f64> = serde_json::from_value(input["x"].clone())?;
        let (resolved, report) = swot_aoi::resolve_grid(&aoi, crs, &y, &x, &Value::Null)?;
        let evaluator = swot_aoi::Evaluator::new(&aoi, crs)?;
        let mask = y
            .iter()
            .map(|y| {
                x.iter()
                    .map(|x| evaluator.contains(*x, *y))
                    .collect::<tile_ripper::Result<Vec<_>>>()
            })
            .collect::<tile_ripper::Result<Vec<_>>>()?;
        let projected = input["points"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                evaluator.project(
                    p[0].as_f64().ok_or("longitude")?,
                    p[1].as_f64().ok_or("latitude")?,
                )
            })
            .collect::<tile_ripper::Result<Vec<_>>>()?;
        json!({"resolved":resolved,"report":report,"source_global_mask":mask,"projected_points":projected})
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
#[cfg(not(feature = "native-hdf5"))]
fn main() {
    eprintln!("build with native-hdf5");
}
