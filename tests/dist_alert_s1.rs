use std::collections::BTreeMap;
use tile_ripper::{
    products::{
        dist_s1,
        profile::{profile, DataType, QualityPolicy, QualitySample},
        Observation, ProductAsset, ProductRequest,
    },
    Layout, Spatial, Variable,
};

const ID: &str = "OPERA_L3_DIST-ALERT-S1_T15UWR_20260228T235832Z_20260714T134753Z_S1C_30_v1.0";
const ROOT: &str = "https://cumulus.asf.earthdatacloud.nasa.gov/OPERA/OPERA_L3_DIST-ALERT-S1_V1";

fn href(layer: &str) -> String {
    format!("{ROOT}/{ID}/{ID}_{layer}.tif")
}

fn observation(layer: &str) -> Observation {
    Observation {
        id: ID.into(),
        datetime: "2026-02-28T23:58:32Z".into(),
        collection: "OPERA_L3_DIST-ALERT-S1_V1".into(),
        platform: Some("Sentinel-1C".into()),
        processing: Some("L3".into()),
        assets: BTreeMap::from([(
            layer.into(),
            ProductAsset {
                href: href(layer),
                etag: Some("\"pinned\"".into()),
                metadata: serde_json::json!({}),
            },
        )]),
        properties: serde_json::json!({}),
    }
}

fn request(layer: &str) -> ProductRequest {
    ProductRequest {
        product: dist_s1::ID.into(),
        provider: Default::default(),
        aoi: Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [-94., 45., -93., 46.],
        },
        start: "2026-02-28T23:58:32Z".into(),
        end: "2026-02-28T23:58:32Z".into(),
        bands: vec![layer.into()],
        output: None,
        observations: None,
        max_observations: 1,
        buffer_bytes: 4 * 1024 * 1024,
        chunk_edge: 64,
        quality_policies: vec![],
    }
}

fn layout(dtype: &str, nodata: Option<f64>) -> Layout {
    Layout {
        width: 3660,
        height: 3660,
        transform: [499_980., 30., 0., 5_600_040., 0., -30.],
        crs: "EPSG:32615".into(),
        variable: Variable::Band { index: 1 },
        block: [512, 512],
        dtype: dtype.into(),
        nodata,
        scale: None,
        offset: None,
        tiles: vec![],
    }
}

fn physical_xml(sensor: &str, post_rtc: &str) -> String {
    format!(
        "<GDALMetadata><Item name=\"mgrs_tile_id\">15UWR</Item><Item name=\"sensor\">{sensor}</Item><Item name=\"post_rtc_opera_ids\">{post_rtc}</Item><Item name=\"version\">2.0.15</Item></GDALMetadata>"
    )
}

#[test]
fn profile_has_the_ten_dist_s1_layers_and_source_specific_fills() {
    let profile = profile(dist_s1::ID).unwrap();
    assert_eq!(profile.bands.len(), 10);
    assert_eq!(
        profile.band("GEN-DIST-STATUS").unwrap().dtype,
        DataType::UInt8
    );
    assert_eq!(profile.band("GEN-DIST-DUR").unwrap().dtype, DataType::Int16);
    let confidence = profile.band("GEN-DIST-CONF").unwrap();
    assert_eq!(confidence.dtype, DataType::Float32);
    assert!(confidence.nodata.unwrap().as_f64().is_nan());
    assert_eq!(
        profile
            .band("GEN-DIST-COUNT")
            .unwrap()
            .nodata
            .unwrap()
            .as_f64(),
        255.
    );
    assert_eq!(profile.band("GEN-DIST-DUR").unwrap().unit, "days");
    assert_eq!(
        profile.band("GEN-DIST-DATE").unwrap().unit,
        "days since 2020-12-31"
    );
}

#[test]
fn identity_time_urls_and_status_policy_are_closed() {
    let profile = profile(dist_s1::ID).unwrap();
    profile
        .validate_identity(ID, Some("sentinel-1c"), Some("DIST-ALERT-S1"))
        .unwrap();
    profile
        .validate_datetime(ID, "2026-02-28T23:58:32Z")
        .unwrap();
    assert!(profile
        .validate_identity(&ID.replace("v1.0", "v1.1"), None, None)
        .is_err());
    assert!(profile
        .validate_identity(&ID.replace("S1C", "S2A"), None, None)
        .is_err());
    assert!(profile
        .validate_datetime(ID, "2026-02-28T23:58:33Z")
        .is_err());
    assert!(dist_s1::band_href("GEN-DIST-CONF", &href("GEN-DIST-CONF")));
    assert!(!dist_s1::band_href("GEN-DIST-CONF", &href("GEN-METRIC")));

    let confirmed = QualityPolicy::DistS1(dist_s1::Policy::Confirmed);
    let ongoing = QualityPolicy::DistS1(dist_s1::Policy::ConfirmedOngoing);
    for value in [3, 6] {
        assert!(confirmed.accepts(QualitySample::UInt8(value)).unwrap());
        assert!(ongoing.accepts(QualitySample::UInt8(value)).unwrap());
    }
    for value in [7, 8] {
        assert!(confirmed.accepts(QualitySample::UInt8(value)).unwrap());
        assert!(!ongoing.accepts(QualitySample::UInt8(value)).unwrap());
    }
    assert!(confirmed.accepts(QualitySample::UInt8(9)).is_err());
    assert!(confirmed.accepts(QualitySample::Float32(1.)).is_err());
}

#[test]
fn canonical_asset_and_float_nan_source_token_are_required() {
    let profile = profile(dist_s1::ID).unwrap();
    let band = profile.band("GEN-DIST-CONF").unwrap();
    let observation = observation(&band.name);
    let asset = observation.assets.get(&band.name).unwrap();
    dist_s1::validate_observation(&request(&band.name), &observation).unwrap();
    let post_rtc = "OPERA_L2_RTC-S1_T092-195753-IW2_20260228T235832Z_20260301T054159Z_S1C_30_v1.0";
    let source = serde_json::json!({"object_version":"\"pinned\"","object_identities":{"native_cog":{
        "gdal_nodata_text":"nan",
        "gdal_metadata_xml":physical_xml("S1C", post_rtc)
    }}});
    dist_s1::validate_physical(&observation, band, asset, &source, &layout("Float32", None))
        .unwrap();
    let wrong_fill = serde_json::json!({"object_version":"\"pinned\"","object_identities":{"native_cog":{
        "gdal_nodata_text":"255",
        "gdal_metadata_xml":physical_xml("S1C", post_rtc)
    }}});
    assert!(dist_s1::validate_physical(
        &observation,
        band,
        asset,
        &wrong_fill,
        &layout("Float32", None)
    )
    .is_err());
    let mut wrong = observation.clone();
    wrong.assets.get_mut(&band.name).unwrap().href = href("GEN-METRIC");
    assert!(dist_s1::validate_observation(&request(&band.name), &wrong).is_err());

    let mut changed = source.clone();
    changed["object_version"] = serde_json::json!("changed");
    assert!(dist_s1::validate_physical(
        &observation,
        band,
        asset,
        &changed,
        &layout("Float32", None)
    )
    .is_err());
    let mut unpinned = observation.clone();
    unpinned.assets.get_mut(&band.name).unwrap().etag = None;
    let unpinned_asset = unpinned.assets.get(&band.name).unwrap();
    assert!(dist_s1::validate_physical(
        &unpinned,
        band,
        unpinned_asset,
        &source,
        &layout("Float32", None)
    )
    .is_ok());
    let mut wrong_etag = observation.clone();
    wrong_etag.assets.get_mut(&band.name).unwrap().etag = Some("\"other\"".into());
    let wrong_etag_asset = wrong_etag.assets.get(&band.name).unwrap();
    assert!(dist_s1::validate_physical(
        &wrong_etag,
        band,
        wrong_etag_asset,
        &source,
        &layout("Float32", None)
    )
    .is_err());
    let mut wrong_sensor = source.clone();
    wrong_sensor["object_identities"]["native_cog"]["gdal_metadata_xml"] =
        serde_json::json!(physical_xml("S1A", post_rtc));
    assert!(dist_s1::validate_physical(
        &observation,
        band,
        asset,
        &wrong_sensor,
        &layout("Float32", None)
    )
    .is_err());
}
