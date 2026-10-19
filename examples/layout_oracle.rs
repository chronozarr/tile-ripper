//! Compare the native window/tile algorithm against the study's SQL manifests.
use serde::Deserialize;
use tile_ripper::{cog, Layout, Result, Spatial};
#[derive(Deserialize)]
struct Case {
    id: String,
    layout: Layout,
    spatial: Spatial,
}
fn main() -> Result<()> {
    let cases: Vec<Case> = serde_json::from_slice(&std::fs::read(
        std::env::args().nth(1).ok_or("input JSON required")?,
    )?)?;
    let t = std::time::Instant::now();
    let rows: Vec<_> = cases
        .iter()
        .map(|c| {
            let w = cog::window(&c.layout, &c.spatial)?;
            let tiles = w
                .as_ref()
                .map(|w| cog::tiles(&c.layout, w))
                .unwrap_or_default();
            Ok(serde_json::json!({"id":c.id,"window":w,"tiles":tiles}))
        })
        .collect::<Result<_>>()?;
    println!(
        "{}",
        serde_json::json!({"planning_seconds":t.elapsed().as_secs_f64(),"rows":rows})
    );
    Ok(())
}
