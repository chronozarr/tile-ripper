//! Same prepared lifecycle; reuse only version-checked metadata across fetches.
use tile_ripper::{acquisition, cog::LayoutCache, Request, Result};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let request: Request =
        serde_json::from_slice(&std::fs::read(args.get(1).ok_or("request required")?)?)?;
    let start = std::time::Instant::now();
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
    let prepare_seconds = start.elapsed().as_secs_f64();
    // Modes are intentionally benchmark/example-only; the CLI remains plan/fetch.
    let mode = args.get(2).map(String::as_str).unwrap_or("fetch");
    if mode == "inspect" {
        println!("{}", prepared.explain());
        return Ok(());
    }
    let reps = if mode == "warm" { 2 } else { 1 };
    let mut results = Vec::new();
    for rep in 0..reps {
        let output = if let tile_ripper::Sink::Directory { path } = &request.sink {
            path.clone()
        } else {
            return Err("directory sink required".into());
        };
        let t = std::time::Instant::now();
        let result = prepared.fetch()?;
        let seconds = t.elapsed().as_secs_f64();
        let archived = format!("{output}-{rep}");
        std::fs::rename(&output, &archived)?;
        results.push(serde_json::json!({"fetch_seconds":seconds,"output":archived,"observed":result["observed"]}));
    }
    println!(
        "{}",
        serde_json::json!({"prepare_seconds":prepare_seconds,"inspection":prepared.explain(),"runs":results})
    );
    Ok(())
}
