//! cargo run --no-default-features --example alphaearth_stream -- request.json
//! NDJSON records: metadata, native Int8 chunks, then validated completion.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
fn main() -> tile_ripper::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: alphaearth_stream REQUEST.json")?;
    let request = serde_json::from_slice(&std::fs::read(path)?)?;
    let stop = Arc::new(AtomicBool::new(false));
    let handler = stop.clone();
    ctrlc::set_handler(move || handler.store(true, Ordering::Release))?;
    let mut source = tile_ripper::alphaearth::prepare(request)?;
    println!(
        "{}",
        serde_json::json!({"kind":"metadata","metadata":source.metadata()})
    );
    while let Some(chunk) = source.next_chunk(&stop)? {
        println!("{}", serde_json::json!({"kind":"chunk","chunk":chunk}));
    }
    println!(
        "{}",
        serde_json::json!({"kind":"completion","report":source.report()})
    );
    Ok(())
}
