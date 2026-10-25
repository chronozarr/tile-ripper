//! Recognizable product selection without exposing physical-reader details.
use std::{collections::BTreeMap, io::Write};
use tile_ripper::{
    products::{self, ProductRequest},
    Result, Spatial,
};
pub fn list() -> Result<()> {
    #[cfg(feature = "native-hdf5")]
    let arrays = products::arrays::profiles();
    #[cfg(not(feature = "native-hdf5"))]
    let arrays = serde_json::json!({"requires":"native-hdf5"});
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"schema":"tile-ripper-products-v1","profiles":products::profile::profiles(),"array_profiles":arrays,"capability":"descriptors require physical native COG inspection; see docs/products/qualification.md for real-source qualification and access boundaries","build":"--no-default-features --features native-cog; Cargo defaults unchanged"})
        )?
    );
    Ok(())
}
pub fn discover(args: &[String]) -> Result<()> {
    let mut flags = BTreeMap::new();
    let mut pairs = args.chunks_exact(2);
    for pair in &mut pairs {
        if ![
            "--product",
            "--provider",
            "--bbox",
            "--start",
            "--end",
            "--bands",
            "--limit",
            "--output",
        ]
        .contains(&pair[0].as_str())
            || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(
                "product discovery accepts --product --bbox --start --end --bands --limit --output"
                    .into(),
            );
        }
    }
    if !pairs.remainder().is_empty() {
        return Err("product discovery option requires value".into());
    }
    let get = |key| {
        flags
            .get(key)
            .copied()
            .ok_or_else(|| format!("missing {key}"))
    };
    let bbox = get("--bbox")?
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<f64>, _>>()?
        .try_into()
        .map_err(|_| "bbox needs four coordinates")?;
    let r: ProductRequest = serde_json::from_value(
        serde_json::json!({"product":get("--product")?,"provider":flags.get("--provider").copied().unwrap_or("auto"),"aoi":Spatial::Bounds{crs:"EPSG:4326".into(),bbox},"start":get("--start")?,"end":get("--end")?,"bands":get("--bands")?.split(',').collect::<Vec<_>>(),"max_observations":flags.get("--limit").map_or(Ok(8),|v|v.parse::<usize>())?}),
    )?;
    #[cfg(feature = "native-hdf5")]
    let catalog = if products::arrays::supports(&r.product) {
        products::arrays::discover(&r)?
    } else {
        serde_json::to_value(products::discover(&r)?)?
    };
    #[cfg(not(feature = "native-hdf5"))]
    let catalog = products::discover(&r)?;
    let bytes = serde_json::to_vec_pretty(&catalog)?;
    if let Some(path) = flags.get("--output") {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    } else {
        std::io::stdout().lock().write_all(&bytes)?;
        println!();
    }
    Ok(())
}
pub fn run(options: super::Options) -> Result<()> {
    #[cfg(feature = "native-hdf5")]
    if options.input["product"]
        .as_str()
        .is_some_and(products::arrays::supports)
    {
        return run_array(options);
    }
    if options.checkpoint.is_some()
        || options.quality_policy.is_some()
        || options.cache_path.is_some()
        || options
            .output_format
            .as_ref()
            .is_some_and(|s| s != "zarr_v2")
        || options.backend == Some(super::Backend::Gdal)
    {
        return Err("product workflow requires native COG/scientific Zarr; quality policies belong in product JSON; archive authentication is product-specific (see qualification documentation)".into());
    }
    std::env::set_var("TILE_RIPPER_COG_BACKEND", "native");
    if options.gdal_free {
        tile_ripper::cog::require_gdal_free()?;
    }
    if let Some(n) = options.workers {
        std::env::set_var("TILE_RIPPER_WORKERS", n.to_string());
    }
    if let Some(n) = options.per_host {
        std::env::set_var("TILE_RIPPER_PER_HOST", n.to_string());
    }
    let mut r: ProductRequest = serde_json::from_value(options.input)
        .map_err(|e| format!("invalid product request: {e}"))?;
    if let Some(path) = options.output {
        r.output = Some(path);
    }
    if options.access.as_ref().is_some_and(|access| {
        access.profile != "earthdata" || access.policy.is_some() || access.ca.is_some()
    }) {
        return Err("product authentication supports explicit --auth earthdata --netrc PRIVATE_FILE for qualified Earthdata routes".into());
    }
    if options.discover_only {
        println!(
            "{}",
            serde_json::to_string_pretty(&products::discover_with_authorization(
                &r,
                options.access.is_some()
            )?)?
        );
        return Ok(());
    }
    if options.command == "stream" {
        r.output = None;
    }
    if options.command == "fetch" && r.output.is_none() {
        return Err("product fetch requires output or --output NEW.zarr".into());
    }
    if options.command == "fetch"
        && r.output
            .as_ref()
            .is_some_and(|path| std::fs::symlink_metadata(path).is_ok())
    {
        return Err("output directory already exists; choose a new product destination".into());
    }
    let prepared = if let Some(access) = options.access {
        if access.profile != "earthdata" || access.policy.is_some() || access.ca.is_some() {
            return Err("product authentication supports explicit --auth earthdata --netrc PRIVATE_FILE for qualified HLS/OPERA routes".into());
        }
        #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
        {
            products::prepare_earthdata_authenticated(
                &r,
                &mut Default::default(),
                std::path::Path::new(&access.netrc),
            )?
        }
        #[cfg(not(all(feature = "native-cog", feature = "authenticated-http")))]
        {
            let _ = access;
            return Err(
                "product authentication requires native-cog and authenticated-http features".into(),
            );
        }
    } else {
        products::prepare(&r, &mut Default::default())?
    };
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if options.command == "fetch" || options.command == "stream" {
        let flag = cancelled.clone();
        ctrlc::set_handler(move || flag.store(true, std::sync::atomic::Ordering::Release))?;
    }
    match options.command.as_str() {
        "plan" => {
            if options.human {
                print!("{}", prepared.human())
            } else {
                println!("{}", serde_json::to_string_pretty(&prepared.explain())?);
            }
        }
        "fetch" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&prepared.fetch_cancellable(cancelled.clone())?)?
            );
        }
        "stream" => {
            use base64::Engine;
            let plan = prepared.plan().clone();
            let profile = products::profile::profile(&r.product)?;
            let mut stream =
                prepared.into_stream_cancellable(Default::default(), cancelled.clone())?;
            println!(
                "{}",
                serde_json::json!({"event":"prepared","provenance":stream.provenance})
            );
            for item in stream.by_ref() {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    stream.cancel();
                    return Err(
                        "product stream cancelled; provisional chunks are incomplete".into(),
                    );
                }
                let c = item?;
                let mut row = serde_json::json!({"event":"chunk","asset_id":c.asset_id,"datetime":c.datetime,"window":c.window,"crs":c.crs,"transform":c.transform,"physical_valid_pixels":c.validity().filter(|v|*v).count(),"provisional":true});
                let selected = plan
                    .selected
                    .iter()
                    .find(|s| s.asset_id == c.asset_id)
                    .ok_or("missing product stream selection")?;
                let aoi = if let Some(geometry) = &selected.mask_geometry {
                    let geometry = tile_ripper::spatial::from_geojson(geometry, &c.crs)?;
                    tile_ripper::spatial::mask(
                        &geometry,
                        selected.layout.as_ref().ok_or("missing native layout")?,
                        &c.window,
                    )?
                    .data()
                    .iter()
                    .map(|v| *v != 0)
                    .collect::<Vec<_>>()
                } else {
                    vec![true; c.pixels.len()]
                };
                let band = c
                    .asset_id
                    .split_once('_')
                    .map(|(_, b)| b)
                    .ok_or("missing product band identity")?;
                let band_profile = profile.band(band)?;
                let source_valid = c
                    .validity()
                    .zip(products::source_validity(&profile, band_profile, &c.pixels))
                    .map(|(physical, product)| physical && product)
                    .collect::<Vec<_>>();
                row["source_valid_pixels"] =
                    serde_json::json!(source_valid.iter().filter(|valid| **valid).count());
                row["valid_pixels"] = serde_json::json!(source_valid
                    .iter()
                    .zip(&aoi)
                    .filter(|(valid, included)| **valid && **included)
                    .count());
                row["product"] = profile.id.into();
                row["product_version"] = profile.version.into();
                row["band"] = band.into();
                row["dtype"] = band_profile.dtype.name().into();
                row["units"] = band_profile.unit.into();
                row["source_nodata"] = serde_json::json!(c.nodata);
                row["scale_factor"] = serde_json::json!(c.scale.or(band_profile.scale));
                row["add_offset"] = serde_json::json!(c.offset.or(band_profile.offset));
                row["missingness"] = "source nodata and all Float32 NaNs invalid; exact source nodata token is retained in prepared provenance".into();
                let mut quality = serde_json::Map::new();
                for name in &r.quality_policies {
                    let policy = profile.quality_policy(name)?;
                    if policy.required_bands().contains(&band) {
                        let mut mask = products::derive_quality_mask(policy, &c.pixels)?;
                        for (i, valid) in source_valid.iter().copied().enumerate() {
                            mask[i] &= u8::from(valid && aoi[i]);
                        }
                        quality.insert(name.clone(), if options.values {serde_json::json!({"accepted":mask.iter().filter(|v|**v==1).count(),"mask_base64":base64::engine::general_purpose::STANDARD.encode(&mask)})} else {serde_json::json!({"accepted":mask.iter().filter(|v|**v==1).count()})});
                    }
                }
                for (name, value) in &mut quality {
                    value["definition"] = profile.quality_policy(name)?.description().into();
                }
                row["quality"] = serde_json::Value::Object(quality);
                if options.values {
                    let (dtype, bytes) = match &c.pixels {
                        tile_ripper::Pixels::Int8(_) => {
                            return Err("COG product stream does not admit Int8".into())
                        }
                        tile_ripper::Pixels::UInt8(v) => ("uint8", v.clone()),
                        tile_ripper::Pixels::UInt16(v) => {
                            ("uint16", v.iter().flat_map(|v| v.to_le_bytes()).collect())
                        }
                        tile_ripper::Pixels::Int16(v) => {
                            ("int16", v.iter().flat_map(|v| v.to_le_bytes()).collect())
                        }
                        tile_ripper::Pixels::UInt32(v) => {
                            ("uint32", v.iter().flat_map(|v| v.to_le_bytes()).collect())
                        }
                        tile_ripper::Pixels::Float32(v) => {
                            ("float32", v.iter().flat_map(|v| v.to_le_bytes()).collect())
                        }
                    };
                    row["source_valid_mask_base64"] = base64::engine::general_purpose::STANDARD
                        .encode(
                            source_valid
                                .iter()
                                .map(|v| u8::from(*v))
                                .collect::<Vec<_>>(),
                        )
                        .into();
                    row["aoi_mask_base64"] = base64::engine::general_purpose::STANDARD
                        .encode(aoi.iter().map(|v| u8::from(*v)).collect::<Vec<_>>())
                        .into();
                    row["dtype"] = dtype.into();
                    row["values_le_base64"] = base64::engine::general_purpose::STANDARD
                        .encode(bytes)
                        .into();
                }
                println!("{}", serde_json::to_string(&row)?);
            }
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                stream.cancel();
                return Err("product stream cancelled; provisional chunks are incomplete".into());
            }
            let report = stream.finish()?;
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err("product stream cancelled before completion".into());
            }
            println!(
                "{}",
                serde_json::json!({"event":"complete","source_versions_validated":true,"observed":report})
            );
        }
        _ => return Err("unknown product command".into()),
    }
    Ok(())
}

#[cfg(feature = "native-hdf5")]
fn run_array(options: super::Options) -> Result<()> {
    use base64::Engine;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tile_ripper::Pixels;
    if options.checkpoint.is_some()
        || options.quality_policy.is_some()
        || options.cache_path.is_some()
        || options.access.is_some()
        || options
            .output_format
            .as_ref()
            .is_some_and(|s| s != "zarr_v2")
        || options.backend == Some(super::Backend::Gdal)
    {
        return Err("array products require native HDF5/scientific Zarr; policies belong in JSON; GOES is anonymous".into());
    }
    let mut r: ProductRequest = serde_json::from_value(options.input)?;
    if let Some(path) = options.output {
        r.output = Some(path);
    }
    if options.discover_only {
        println!(
            "{}",
            serde_json::to_string_pretty(&products::arrays::discover(&r)?)?
        );
        return Ok(());
    }
    let p = products::arrays::prepare(&r)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    if options.command == "fetch" || options.command == "stream" {
        let c = cancelled.clone();
        ctrlc::set_handler(move || c.store(true, Ordering::Release))?;
    }
    match options.command.as_str() {
        "plan" => println!("{}", serde_json::to_string_pretty(&p.explain())?),
        "fetch" => {
            let path = r
                .output
                .as_ref()
                .ok_or("array fetch requires output or --output NEW.zarr")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&p.fetch(path, cancelled)?)?
            );
        }
        "stream" => {
            println!(
                "{}",
                serde_json::json!({"event":"prepared","provenance":p.explain()})
            );
            let reports=p.execute(&cancelled,|o,c|{
                let mut row=serde_json::json!({"event":"chunk","asset_id":c.asset_id,"variable":c.variable,"window":c.window,"coordinates":c.coordinates,"provisional":true,"coordinate_model":o.grid});
                if options.values {
                    let(dtype,bytes):(&str,Vec<u8>)=match &c.pixels {Pixels::Int8(v)=>("int8",v.iter().map(|v|*v as u8).collect()),Pixels::UInt8(v)=>("uint8",v.clone()),Pixels::Int16(v)=>("int16",v.iter().flat_map(|v|v.to_le_bytes()).collect()),Pixels::UInt16(v)=>("uint16",v.iter().flat_map(|v|v.to_le_bytes()).collect()),Pixels::UInt32(v)=>("uint32",v.iter().flat_map(|v|v.to_le_bytes()).collect()),Pixels::Float32(v)=>("float32",v.iter().flat_map(|v|v.to_le_bytes()).collect())};
                    row["dtype"]=dtype.into();row["values_le_base64"]=base64::engine::general_purpose::STANDARD.encode(bytes).into();
                    row["source_valid_mask_base64"]=base64::engine::general_purpose::STANDARD.encode(c.validity().map(u8::from).collect::<Vec<_>>()).into();
                }
                println!("{}",serde_json::to_string(&row)?);Ok(())
            })?;
            println!(
                "{}",
                serde_json::json!({"event":"complete","source_versions_validated":true,"observed":reports})
            );
        }
        _ => return Err("unknown array command".into()),
    }
    Ok(())
}
