//! cargo run --release --example stac_acquire -- examples/stac-request.json [--fetch]
use tile_ripper::{
    cog::LayoutCache,
    stac_workflow::{self, StacRequest},
    Result,
};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let file = args.get(1).ok_or("request JSON path required")?;
    let request: StacRequest = serde_json::from_slice(&std::fs::read(file)?)?;
    let mut cache = LayoutCache::default();
    let prepared = stac_workflow::prepare(&request, &mut cache)?;
    if args.get(2).is_some_and(|v| v == "--fetch") {
        println!("{}", serde_json::to_string_pretty(&prepared.fetch()?)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&prepared.explain())?);
    }
    Ok(())
}
