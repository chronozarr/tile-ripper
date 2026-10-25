//! Controlled local/loopback lifecycle gate; no production source is mutated.
#[cfg(feature = "native-hdf5")]
fn main() -> tile_ripper::Result<()> {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tile_ripper::products::{arrays, ProductRequest};
    let args: Vec<_> = std::env::args().collect();
    let r: ProductRequest =
        serde_json::from_slice(&std::fs::read(args.get(1).ok_or("REQUEST.json required")?)?)?;
    let mode = args.get(2).map(String::as_str).unwrap_or("cancel-output");
    let p = arrays::prepare(&r)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    match mode {
        "cancel-output" => {
            let path = args.get(3).ok_or("NEW.zarr required")?;
            cancelled.store(true, Ordering::Release);
            assert!(p.fetch(path, cancelled).is_err());
            assert!(!std::path::Path::new(path).exists());
        }
        "cancel-stream" => {
            let mut delivered = 0;
            let result = p.execute(&cancelled, |_, _| {
                delivered += 1;
                cancelled.store(true, Ordering::Release);
                Ok(())
            });
            assert!(result.is_err());
            assert_eq!(delivered, 1);
        }
        "consumer-error" => {
            assert!(p
                .execute(&cancelled, |_, _| Err("injected consumer failure".into()))
                .is_err());
        }
        _ => return Err("unknown lifecycle test mode".into()),
    }
    println!("{{\"status\":\"passed\",\"mode\":\"{mode}\"}}");
    Ok(())
}
#[cfg(not(feature = "native-hdf5"))]
fn main() {
    eprintln!("requires native-hdf5");
    std::process::exit(2);
}
