//! Uses exactly the same request, planner and execution entry points as COG.
use tile_ripper::{cog::LayoutCache, execute, fetch, plan, Request, Result};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let request: Request =
        serde_json::from_slice(&std::fs::read(args.get(1).ok_or("request JSON required")?)?)?;
    let mut cache = LayoutCache::default();
    match args.get(2).map(String::as_str) {
        Some("--fetch") => println!(
            "{}",
            serde_json::to_string_pretty(&fetch(&request, &mut cache)?)?
        ),
        Some("--stream") => {
            execute(&request, &mut cache, |chunk| {
                println!(
                    "{} {:?} {:?} {} cells",
                    chunk.asset_id,
                    chunk.variable,
                    chunk.datetime,
                    chunk.pixels.len()
                );
                Ok(())
            })?;
        }
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&plan(&request, &mut cache)?)?
        ),
    }
    Ok(())
}
