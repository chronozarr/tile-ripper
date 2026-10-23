//! Same job implementation as the CLI. No credentials in manifests or plans.
//! cargo run --release --no-default-features --features native-hdf5,authenticated-http \
//!   --example swot_job -- fetch examples/swot-job.json NEW.zarr NETRC CHECKPOINT
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
fn main() -> tile_ripper::Result<()> {
    use std::sync::Arc;
    use tile_ripper::{
        authenticated_http::AuthenticatedHttp,
        checkpoint,
        swot_job::{Job, Manifest},
        Asset, Result,
    };
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 || !matches!(args[0].as_str(), "plan" | "fetch") {
        return Err("usage: swot_job plan MANIFEST [NETRC [CHECKPOINT]] | fetch MANIFEST NEW.zarr [NETRC [CHECKPOINT]]".into());
    }
    let manifest = Manifest::from_json(&std::fs::read(&args[1])?)?;
    let offset = if args[0] == "fetch" { 3 } else { 2 };
    let mut sessions = |asset: &Asset| -> Result<Arc<AuthenticatedHttp>> {
        Ok(Arc::new(AuthenticatedHttp::earthdata(
            &asset.locator,
            args.get(offset).ok_or("explicit NETRC path required")?,
        )?))
    };
    let job = if args.get(offset).is_some() {
        // Options are AGGREGATE bytes/entries, statically divided among sources.
        let checkpoint = args.get(offset + 1).map(checkpoint::Options::new);
        Job::authenticated(&manifest, &mut sessions, checkpoint)?
    } else {
        Job::new(&manifest)?
    };
    let receipt = if args[0] == "plan" {
        job.plan()?
    } else {
        // A host can set this shared flag from its own cancellation mechanism.
        job.fetch(
            args.get(2).ok_or("fetch requires NEW.zarr")?,
            Arc::default(),
        )?
    };
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
#[cfg(not(all(feature = "native-hdf5", feature = "authenticated-http")))]
fn main() {
    eprintln!("build with --no-default-features --features native-hdf5,authenticated-http");
}
