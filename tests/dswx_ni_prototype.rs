use std::collections::BTreeMap;

use tile_ripper::{
    products::{
        dswx_ni::{self, Policy},
        profile::QualitySample,
        routing::Provider,
        Observation, ProductAsset, ProductRequest,
    },
    Layout, Result, Spatial, Variable,
};

fn request(provider: Provider) -> ProductRequest {
    ProductRequest {
        product: dswx_ni::ID.into(),
        provider,
        aoi: Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [68.9, 25.0, 69.1, 25.2],
        },
        start: "2026-01-12T00:45:24Z".into(),
        end: "2026-01-12T00:45:24Z".into(),
        bands: vec!["WTR".into(), "BWTR".into(), "CONF".into(), "DIAG".into()],
        output: None,
        observations: Some(vec![observation()]),
        max_observations: 1,
        buffer_bytes: 16 * 1024 * 1024,
        chunk_edge: 64,
        quality_policies: vec![
            Policy::WtrOpenWater.name().into(),
            Policy::WtrWater.name().into(),
            Policy::WtrEdgeWater.name().into(),
            Policy::BwtrWater.name().into(),
        ],
    }
}

fn observation() -> Observation {
    let mut assets = BTreeMap::new();
    for band in ["WTR", "BWTR", "CONF", "DIAG"] {
        let href = dswx_ni::band_href(band).unwrap();
        let etag = match band {
            "WTR" => "\"4c24ac7234e0d63d71ade7f8269b455d\"",
            "BWTR" => "\"63c17a56666857346c31dd92df615ffd\"",
            "CONF" => "\"5bcf61a8f64146e91a7d50984b4162d7\"",
            "DIAG" => "\"f67e4e123f22f440267381d6bd2ae57d\"",
            _ => unreachable!(),
        };
        assets.insert(
            band.into(),
            ProductAsset {
                href,
                etag: Some(etag.into()),
                metadata: serde_json::json!({}),
            },
        );
    }
    Observation {
        id: dswx_ni::GRANULE.into(),
        datetime: "2026-01-12T00:45:24Z".into(),
        collection: dswx_ni::CANONICAL_COLLECTION.into(),
        platform: Some("NISAR".into()),
        processing: Some("L3".into()),
        assets,
        properties: serde_json::json!({
            "mosaic_start_datetime":"2026-01-12T00:44:50Z",
            "mosaic_end_datetime":"2026-01-12T00:45:58Z",
            "filename_processing_datetime":"2026-04-17T09:49:01Z",
            "source_processing_datetime":"2026-04-17T09:49:12Z"
        }),
    }
}

#[test]
fn prototype_identity_is_explicitly_pinned_and_distinguishes_times() -> Result<()> {
    let observation = observation();
    dswx_ni::validate_identity(
        &observation.id,
        observation.platform.as_deref(),
        observation.processing.as_deref(),
    )?;
    dswx_ni::validate_datetime(&observation.id, &observation.datetime)?;
    assert_eq!(
        dswx_ni::filename_for_band("WTR")?,
        format!("{}_B01_WTR.tif", dswx_ni::GRANULE)
    );
    assert!(dswx_ni::validate_datetime(&observation.id, "2026-01-12T00:44:50Z").is_err());
    assert!(dswx_ni::validate_identity(
        &observation.id.replace("v0.1", "v1.0"),
        Some("NISAR"),
        Some("L3"),
    )
    .is_err());
    Ok(())
}

#[test]
fn policy_domains_keep_contextual_codes_raw_valid_but_unselected() -> Result<()> {
    for policy in [Policy::WtrOpenWater, Policy::WtrWater, Policy::WtrEdgeWater] {
        assert!(!policy.accepts(QualitySample::UInt8(0))?);
        assert!(!policy.accepts(QualitySample::UInt8(250))?);
        assert!(!policy.accepts(QualitySample::UInt8(251))?);
        assert!(!policy.accepts(QualitySample::UInt8(254))?);
        assert!(!policy.accepts(QualitySample::UInt8(255))?);
    }
    assert!(Policy::WtrOpenWater.accepts(QualitySample::UInt8(1))?);
    assert!(!Policy::WtrOpenWater.accepts(QualitySample::UInt8(3))?);
    assert!(Policy::WtrWater.accepts(QualitySample::UInt8(3))?);
    assert!(!Policy::WtrWater.accepts(QualitySample::UInt8(8))?);
    assert!(Policy::WtrEdgeWater.accepts(QualitySample::UInt8(8))?);
    assert!(Policy::BwtrWater.accepts(QualitySample::UInt8(1))?);
    assert!(Policy::BwtrWater.accepts(QualitySample::UInt8(3)).is_err());
    assert!(Policy::WtrWater.accepts(QualitySample::UInt8(2)).is_err());
    Ok(())
}

#[test]
fn prototype_observation_refuses_auto_routes_and_unpinned_assets() {
    let auto = request(Provider::Auto);
    assert!(dswx_ni::validate_observation(&auto, &observation()).is_err());

    let pinned = request(Provider::Pinned);
    dswx_ni::validate_observation(&pinned, &observation()).unwrap();

    let mut changed = observation();
    changed.assets.get_mut("CONF").unwrap().etag = Some("\"different\"".into());
    assert!(dswx_ni::validate_observation(&pinned, &changed).is_err());
    changed = observation();
    changed.properties["source_processing_datetime"] = serde_json::json!("2026-04-17T09:49:01Z");
    assert!(dswx_ni::validate_observation(&pinned, &changed).is_err());
}

fn layout() -> Layout {
    Layout {
        width: 3660,
        height: 3660,
        transform: [399960., 30., 0., 2900040., 0., -30.],
        crs: "EPSG:32642".into(),
        variable: Variable::Band { index: 1 },
        block: [512, 512],
        dtype: "Byte".into(),
        nodata: Some(255.),
        scale: Some(1.),
        offset: Some(0.),
        tiles: vec![],
    }
}

fn source(description: &str) -> serde_json::Value {
    serde_json::json!({"object_identities":{"native_cog":{"gdal_metadata_xml":format!(
        "<GDALMetadata><Item name=\"PROJECT\">OPERA</Item><Item name=\"PRODUCT_TYPE\">DSWx-NI</Item><Item name=\"DSWX_PRODUCT_VERSION\">0.1</Item><Item name=\"SOFTWARE_VERSION\">0.3.0</Item><Item name=\"PRODUCT_SOURCE\">NISAR_GCOV</Item><Item name=\"SENSOR\">LSAR</Item><Item name=\"SPACECRAFT_NAME\">NISAR</Item><Item name=\"ZERO_DOPPLER_START_TIME\">2026-01-12T00:44:50Z</Item><Item name=\"ZERO_DOPPLER_END_TIME\">2026-01-12T00:45:58Z</Item><Item name=\"PROCESSING_DATETIME\">2026-04-17T09:49:12Z</Item><Item name=\"DESCRIPTION\">{description}</Item></GDALMetadata>"
    )}}})
}

#[test]
fn physical_gate_requires_exact_layout_and_metadata_layer_binding() -> Result<()> {
    let observation = observation();
    let profile = dswx_ni::profile();
    let band = profile.band("WTR")?;
    let asset = observation.assets.get("WTR").unwrap();
    dswx_ni::validate_physical(
        &observation,
        band,
        asset,
        &source("Water classification (WTR)"),
        &layout(),
    )?;
    assert!(dswx_ni::validate_physical(
        &observation,
        band,
        asset,
        &source("Diagnostic layer (DIAG)"),
        &layout(),
    )
    .is_err());
    let mut wrong = layout();
    wrong.block = [256, 256];
    assert!(dswx_ni::validate_physical(
        &observation,
        band,
        asset,
        &source("Water classification (WTR)"),
        &wrong,
    )
    .is_err());
    Ok(())
}
