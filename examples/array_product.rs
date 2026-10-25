//! A named native-array product through the same preparation and source validation lifecycle.
#[cfg(feature = "native-hdf5")]
fn main() -> tile_ripper::Result<()> {
    use tile_ripper::products::{arrays, ProductRequest};
    let args: Vec<_> = std::env::args().collect();
    let request: ProductRequest = serde_json::from_slice(&std::fs::read(
        args.get(1)
            .ok_or("usage: array_product REQUEST.json [NEW.zarr]")?,
    )?)?;
    let prepared = arrays::prepare(&request)?;
    println!("{}", serde_json::to_string_pretty(&prepared.explain())?);
    if let Some(path) = args.get(2) {
        println!(
            "{}",
            serde_json::to_string(&prepared.fetch(path, Default::default())?)?
        );
    } else {
        let mut count = 0;
        prepared.execute(&Default::default(), |_, c| {
            count += c.pixels.len();
            Ok(())
        })?;
        println!("validated {count} native samples");
    }
    Ok(())
}
#[cfg(not(feature = "native-hdf5"))]
fn main() {
    eprintln!("array_product requires native-hdf5");
    std::process::exit(2);
}
