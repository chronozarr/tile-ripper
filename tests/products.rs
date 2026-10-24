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
fn select_native_backend() {
    static SELECT: std::sync::Once = std::sync::Once::new();
    SELECT.call_once(|| std::env::set_var("TILE_RIPPER_COG_BACKEND", "native"));
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
    select_native_backend();
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

fn hls_observation(href: &str) -> products::Observation {
    serde_json::from_value(serde_json::json!({
        "id":"HLS.L30.T17SPV.2024168T155243.v2.0", "datetime":"2024-06-16T15:52:43Z",
        "collection":"HLSL30", "platform":null,"processing":null,
        "assets":{"B04":{"href":href,"etag":null}}
    }))
    .unwrap()
}
#[test]
fn provider_override_preserves_frozen_observations_and_rejects_mixed_sources() {
    use products::routing::Provider;
    let mut r = request();
    r.product = "hls-l30".into();
    r.bands = vec!["B04".into()];
    r.observations = Some(vec![hls_observation(
        "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/HLSL30.020/HLS.L30.T17SPV.2024168T155243.v2.0/HLS.L30.T17SPV.2024168T155243.v2.0.B04.tif",
    )]);
    let catalog = products::discover(&r).unwrap();
    assert_eq!(catalog.provider, Provider::Earthdata);
    assert!(catalog.routing.observations_preserved);
    r.provider = Provider::PlanetaryComputer;
    let error = products::discover(&r).err().unwrap().to_string();
    assert!(error.contains("conflicts with supplied"));
    r.provider = Provider::Auto;
    r.observations
        .as_mut()
        .unwrap()
        .push(hls_observation("/tmp/mirror.tif"));
    assert!(products::discover(&r)
        .err()
        .unwrap()
        .to_string()
        .contains("mix physical providers"));
}
#[test]
fn supplied_provider_cannot_claim_legacy_or_wrong_science() {
    let mut r = request();
    r.product = "hls-l30".into();
    r.bands = vec!["B04".into()];
    let id = "HLS.L30.T17SPV.2024168T155243.v2.0";
    r.observations = Some(vec![hls_observation(&format!(
        "https://hls2euwest.blob.core.windows.net/hls2/L30/17/S/PV/2024/06/16/{id}/{id}.B04.tif"
    ))]);
    r.observations.as_mut().unwrap()[0].collection = "hls2-l30".into();
    assert_eq!(
        products::discover(&r).unwrap().provider,
        products::routing::Provider::PlanetaryComputer
    );
    r.observations.as_mut().unwrap()[0].id = id.replace("v2.0", "v1.4");
    assert!(products::discover(&r).is_err());
    r.observations.as_mut().unwrap()[0].id = id.into();
    r.observations.as_mut().unwrap()[0]
        .assets
        .get_mut("B04")
        .unwrap()
        .href = format!(
        "https://hls2euwest.blob.core.windows.net/hls2/L30/17/S/PV/2024/06/16/{id}/{id}.B03.tif"
    );
    assert!(products::discover(&r).is_err());
}

#[test]
fn sample_equivalence_requires_the_actual_attested_locator_and_etag() {
    let product = "hls-l30";
    let id = "HLS.L30.T15VUE.2022231T172609.v2.0";
    let href = format!(
        "https://hls2euwest.blob.core.windows.net/hls2/L30/15/V/UE/2022/08/19/{id}/{id}.B04.tif"
    );
    let known =
        products::routing::attestation(product, id, "B04", &href, Some("\"0x8DDD4C2657FAAAD\""));
    assert!(!known["group"].is_null());
    assert_eq!(known["exact_byte_replica_proven"], false);
    for (p, g, b, h, e) in [
        (product, id, "B04", href.clone(), Some("\"changed\"")),
        (product, id, "B04", href.clone(), None),
        (
            product,
            id,
            "Fmask",
            href.clone(),
            Some("\"0x8DDD4C2657FAAAD\""),
        ),
        (
            product,
            id,
            "B04",
            href.replace("L30/15", "L30/16"),
            Some("\"0x8DDD4C2657FAAAD\""),
        ),
        (
            "hls-s30",
            id,
            "B04",
            href.clone(),
            Some("\"0x8DDD4C2657FAAAD\""),
        ),
    ] {
        assert!(products::routing::attestation(p, g, b, &h, e)["group"].is_null());
    }
}

#[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
#[test]
fn unused_authorization_is_not_opened_by_a_credential_free_preparation() {
    select_native_backend();
    let mut r = request();
    r.provider = products::routing::Provider::PlanetaryComputer;
    let p = products::prepare_earthdata_authenticated(
        &r,
        &mut Default::default(),
        std::path::Path::new("/nonexistent/private-netrc"),
    )
    .unwrap();
    let v = p.explain();
    assert_eq!(v["product"]["routing"]["selected"], "planetary_computer");
    assert_eq!(v["product"]["routing"]["credentials_required"], false);
    assert_eq!(v["product"]["routing"]["authorization_configured"], true);
}

#[test]
fn coverage_precedes_convenience_and_unknown_coverage_is_never_empty() {
    use products::routing::{choose_coverage, Provider};
    use std::collections::BTreeSet;
    let set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<BTreeSet<_>>();
    let a = set(&["a"]);
    let ab = set(&["a", "b"]);
    let b = set(&["b"]);
    let empty = set(&[]);
    assert_eq!(
        choose_coverage(Some(&a), Some(&a), true).unwrap().0,
        Provider::PlanetaryComputer
    );
    assert_eq!(
        choose_coverage(Some(&a), Some(&ab), true).unwrap().0,
        Provider::Earthdata
    );
    assert_eq!(
        choose_coverage(Some(&ab), Some(&a), true).unwrap().0,
        Provider::PlanetaryComputer
    );
    assert!(choose_coverage(Some(&a), Some(&b), true).is_err());
    assert_eq!(
        choose_coverage(Some(&a), Some(&ab), false).unwrap().0,
        Provider::PlanetaryComputer
    );
    assert_eq!(
        choose_coverage(Some(&empty), Some(&ab), false).unwrap().0,
        Provider::Earthdata
    );
    assert_eq!(
        choose_coverage(None, Some(&a), true).unwrap().0,
        Provider::Earthdata
    );
    assert!(choose_coverage(None, None, true).is_err());
}

#[cfg(feature = "native-cog")]
#[test]
fn protected_supplied_source_fails_before_io_without_explicit_authorization() {
    select_native_backend();
    let mut r = request();
    r.product = "hls-l30".into();
    r.bands = vec!["B04".into()];
    r.observations=Some(vec![hls_observation("https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/HLSL30.020/HLS.L30.T17SPV.2024168T155243.v2.0/HLS.L30.T17SPV.2024168T155243.v2.0.B04.tif")]);
    let error = products::prepare(&r, &mut Default::default())
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("requires explicit --auth earthdata --netrc"),
        "{error}"
    );
}

#[test]
fn current_opera_frozen_sources_preserve_their_qualified_earthdata_routes() {
    use products::routing::Provider;
    for text in [
        include_str!("../docs/products/dswx-authenticated-request.json"),
        include_str!("../docs/products/rtc-authenticated-request.json"),
    ] {
        let mut r: ProductRequest = serde_json::from_str(text).unwrap();
        let catalog = products::discover(&r).unwrap();
        assert_eq!(catalog.provider, Provider::Earthdata);
        assert!(catalog.routing.credentials_required);
        assert!(catalog.routing.observations_preserved);
        r.provider = Provider::Earthdata;
        assert_eq!(
            products::discover(&r).unwrap().provider,
            Provider::Earthdata
        );
        #[cfg(feature = "native-cog")]
        {
            select_native_backend();
            let error = products::prepare(&r, &mut Default::default())
                .err()
                .unwrap()
                .to_string();
            assert!(
                error.contains("requires explicit --auth earthdata --netrc"),
                "{error}"
            );
        }
        r.provider = Provider::PlanetaryComputer;
        let error = products::discover(&r).err().unwrap().to_string();
        assert!(
            error.contains("conflicts with supplied physical sources"),
            "{error}"
        );
    }
}

#[test]
fn legacy_asf_origins_do_not_claim_current_qualified_authentication() {
    let mut r: ProductRequest = serde_json::from_str(include_str!(
        "../docs/products/rtc-authenticated-request.json"
    ))
    .unwrap();
    for observation in r.observations.as_mut().unwrap() {
        for asset in observation.assets.values_mut() {
            asset.href = asset.href.replace(
                "cumulus.asf.earthdatacloud.nasa.gov",
                "datapool.asf.alaska.edu",
            );
        }
    }
    let error = products::discover(&r).err().unwrap().to_string();
    assert!(
        error.contains("legacy ASF origin has no qualified"),
        "{error}"
    );
}
