//! cargo run --release --locked --example acquire -- request.json [--fetch|--stream]
use tile_ripper::{acquisition, cog::LayoutCache, Request, Result};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let request: Request =
        serde_json::from_slice(&std::fs::read(args.get(1).ok_or("request file required")?)?)?;
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
    match args.get(2).map(String::as_str) {
        Some("--fetch") => println!("{}", serde_json::to_string_pretty(&prepared.fetch()?)?),
        Some("--stream") => {
            prepared.execute(|chunk| {
                println!(
                    "{} {:?} {:?}: {} native cells",
                    chunk.asset_id,
                    chunk.variable,
                    chunk.datetime,
                    chunk.pixels.len()
                );
                Ok(())
            })?;
        }
        _ => println!("{}", prepared.human()),
    }
    Ok(())
}
