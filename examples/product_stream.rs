//! Stream any qualified named COG profile; optional authorization is explicit.
//! cargo run --no-default-features --features native-cog,authenticated-http \
//!   --example product_stream -- REQUEST.json [PRIVATE.netrc]
#[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
use std::path::Path;
use tile_ripper::{
    products::{self, ProductRequest},
    Result,
};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("expected product REQUEST.json [PRIVATE.netrc]")?;
    let netrc = args.next();
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let mut request: ProductRequest = serde_json::from_slice(&std::fs::read(path)?)?;
    request.output = None;
    std::env::set_var("TILE_RIPPER_COG_BACKEND", "native");
    let prepared = if let Some(path) = netrc {
        #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
        {
            products::prepare_earthdata_authenticated(
                &request,
                &mut Default::default(),
                Path::new(&path),
            )?
        }
        #[cfg(not(all(feature = "native-cog", feature = "authenticated-http")))]
        {
            let _ = path;
            return Err("explicit auth requires native-cog,authenticated-http".into());
        }
    } else {
        products::prepare(&request, &mut Default::default())?
    };
    let mut stream = prepared.into_stream(Default::default())?;
    for chunk in stream.by_ref() {
        let chunk = chunk?;
        // Native typed values remain in chunk.pixels. Every chunk is provisional
        // until finish validates all source versions. No calibration is applied.
        println!(
            "{}",
            serde_json::json!({"asset":chunk.asset_id,"datetime":chunk.datetime,"window":chunk.window,"native_values":chunk.pixels.len(),"provisional":true})
        );
    }
    let report = stream.finish()?;
    println!(
        "{}",
        serde_json::json!({"complete":true,"source_versions_validated":true,"observed":report})
    );
    Ok(())
}
