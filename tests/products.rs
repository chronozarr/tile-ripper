use tile_ripper::products::{self, ProductRequest};
fn request() -> ProductRequest {
    serde_json::from_value(serde_json::json!({"product":"landsat-c2-l2-sr","aoi":{"kind":"bounds","crs":"EPSG:4326","bbox":[-79.,35.,-78.,36.]},"start":"2024-06-01T00:00:00Z","end":"2024-06-30T23:59:59Z","bands":["SR_B4"],"observations":[]})).unwrap()
}
#[test]
fn product_selection_rejects_scientifically_ambiguous_requests() {
    let mut r = request();
    r.validate().unwrap();
    r.bands.push("red".into());
    assert!(r.validate().is_err());
    let mut r = request();
    r.start = "2024-04-01T00:00:00Z".into();
    assert!(r.validate().is_err());
    let mut r = request();
    r.quality_policies.push("hls_fmask_clear".into());
    assert!(r.validate().is_err());
    let mut r = request();
    r.max_observations = 33;
    assert!(r.validate().is_err());
}
#[test]
fn explicit_manifest_is_not_a_format_or_identity_bypass() {
    let mut r = request();
    r.observations=Some(vec![serde_json::from_value(serde_json::json!({"id":"LC07_L2SP_016035_20240629_02_T1","datetime":"2024-06-29T15:52:43Z","collection":"landsat-c2-l2","platform":"landsat-7","processing":"L2SP","assets":{"SR_B4":{"href":"/missing.tif","etag":null}}})).unwrap()]);
    assert!(products::discover(&r).is_err());
    r.observations.as_mut().unwrap()[0].id = "LC08_L2SP_016035_20240629_02_T1".into();
    r.observations.as_mut().unwrap()[0].platform = Some("landsat-8".into());
    r.observations.as_mut().unwrap()[0]
        .assets
        .get_mut("SR_B4")
        .unwrap()
        .href = "s3://usgs-landsat/paid.tif".into();
    assert!(products::discover(&r).is_err());
    r.observations.as_mut().unwrap()[0]
        .assets
        .get_mut("SR_B4")
        .unwrap()
        .href = "/missing.tif".into();
    r.observations.as_mut().unwrap()[0].datetime = "2024-07-01T00:00:00Z".into();
    assert!(products::discover(&r).is_err());
}
#[test]
fn empty_explicit_selection_is_complete_not_an_implicit_remote_search() {
    let report = products::discover(&request()).unwrap();
    assert!(report.complete);
    assert!(report.observations.is_empty());
}

#[test]
fn observation_time_and_metadata_bounds_are_enforced_before_acquisition() {
    let mut r = request();
    r.observations = Some(vec![serde_json::from_value(serde_json::json!({
        "id":"LC08_L2SP_016035_20240629_02_T1",
        "datetime":"2024-06-28T15:52:43Z",
        "collection":"landsat-c2-l2",
        "platform":"landsat-8",
        "processing":"L2SP",
        "assets":{"SR_B4":{"href":"/missing.tif","etag":null}}
    }))
    .unwrap()]);
    assert!(products::discover(&r).is_err());

    r.observations.as_mut().unwrap()[0].datetime = "2024-06-29T15:52:43Z".into();
    r.observations.as_mut().unwrap()[0].properties =
        serde_json::json!({"oversized":"x".repeat(17 * 1024)});
    assert!(products::discover(&r).is_err());
}

#[cfg(feature = "native-cog")]
fn local_product_request(
    product: &str,
    id: &str,
    datetime: &str,
    collection: &str,
    band: &str,
    fixture: &str,
    metadata: serde_json::Value,
) -> ProductRequest {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/native_cog")
        .join(fixture);
    serde_json::from_value(serde_json::json!({
        "product":product,
        "aoi":{"kind":"bounds","crs":"EPSG:4326","bbox":[-81.001,36.14,-80.99,36.15]},
        "start":datetime,
        "end":datetime,
        "bands":[band],
        "observations":[{
            "id":id,
            "datetime":datetime,
            "collection":collection,
            "platform":null,
            "processing":null,
            "assets":{(band):{"href":path,"etag":null,"metadata":metadata}}
        }]
    }))
    .unwrap()
}

#[cfg(feature = "native-cog")]
#[test]
fn generic_cog_cannot_claim_a_product_identity() {
    let r = local_product_request(
        "hls-l30",
        "HLS.L30.T17SPV.2022231T172609.v2.0",
        "2022-08-19T17:26:09Z",
        "HLSL30",
        "B04",
        "hls_int16.tif",
        serde_json::json!({}),
    );
    assert!(products::prepare(&r, &mut Default::default()).is_err());
}

#[cfg(feature = "native-cog")]
#[test]
fn catalog_nodata_conflict_is_rejected() {
    let canonical = "https://landsateuwest.blob.core.windows.net/landsat-c2/level-2/standard/oli-tirs/2024/016/035/LC08_L2SP_016035_20240629_20240701_02_T1/LC08_L2SP_016035_20240629_20240701_02_T1_QA_PIXEL.TIF";
    let mut r = local_product_request(
        "landsat-c2-l2-sr",
        "LC08_L2SP_016035_20240629_02_T1",
        "2024-06-29T15:52:43Z",
        "landsat-c2-l2",
        "QA_PIXEL",
        "qa_uint16_no_nodata.tif",
        serde_json::json!({"href":canonical,"raster:bands":[{"nodata":0}]}),
    );
    r.observations.as_mut().unwrap()[0].platform = Some("landsat-8".into());
    r.observations.as_mut().unwrap()[0].processing = Some("L2SP".into());
    assert!(products::prepare(&r, &mut Default::default()).is_err());
}
