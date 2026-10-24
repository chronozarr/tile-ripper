use tile_ripper::{
    products::{
        self,
        profile::{profile, QualityPolicy as Q, QualitySample as S},
    },
    Pixels, Result,
};

#[test]
fn historical_sensor_band_and_quality_contracts_remain_distinct() -> Result<()> {
    let tm = profile("landsat-tm-c2-l2-sr")?;
    let etm = profile("landsat-etm-c2-l2-sr")?;
    let oli = profile("landsat-c2-l2-sr")?;
    for (p, id, platform) in [
        (&tm, "LT04_L2SP_016035_19881109_02_T1", "landsat-4"),
        (&tm, "LT05_L2SP_016035_20100810_02_T1", "landsat-5"),
        (&etm, "LE07_L2SP_016035_20040716_02_T1", "landsat-7"),
    ] {
        p.validate_identity(id, Some(platform), Some("L2SP"))?;
        assert!(oli
            .validate_identity(id, Some(platform), Some("L2SP"))
            .is_err());
        assert_eq!(p.band("red")?.name, "SR_B3");
        assert_eq!(p.band("nir")?.name, "SR_B4");
        assert_eq!(p.band("SR_B3")?.scale, Some(0.0000275));
        assert_eq!(p.band("SR_B3")?.offset, Some(-0.2));
        assert!(p.band("SR_B6").is_err());
        assert!(p.band("SR_QA_AEROSOL").is_err());
        assert!(p.quality_policy("landsat_pixel_clear").is_err());
    }
    assert!(tm
        .validate_identity("LE07_L2SP_016035_20040716_02_T1", None, None)
        .is_err());
    assert!(etm
        .validate_identity("LT05_L2SP_016035_20100810_02_T1", None, None)
        .is_err());
    // The USGS clear-bit issue over historical water must not invalidate a raw QA word.
    for word in [128, 128 | 64] {
        assert!(Q::LandsatTmEtmPixelClear.accepts(S::UInt16(word))?);
    }
    for bit in [0, 1, 3, 4, 5] {
        assert!(!Q::LandsatTmEtmPixelClear.accepts(S::UInt16(1 << bit))?);
    }
    assert!(!Q::LandsatEtmRadiometricallyUnsaturated.accepts(S::UInt16(1 << 9))?);
    assert!(!Q::LandsatEtmRadiometricallyUnsaturated.accepts(S::UInt16(1 << 8))?);
    assert!(Q::LandsatTmRadiometricallyUnsaturated
        .accepts(S::UInt16(1 << 8))
        .is_err());
    assert!(Q::LandsatEtmRadiometricallyUnsaturated
        .accepts(S::UInt16(1 << 11))
        .is_err());
    let gap = Pixels::UInt16(vec![0, 10000]);
    assert_eq!(
        products::source_validity(&etm, etm.band("red")?, &gap).collect::<Vec<_>>(),
        [false, true]
    );
    // LEDAPS cloud QA zero is product fill even though the TIFF has no marker.
    let cloud = Pixels::UInt8(vec![0, 1, 32, 255]);
    assert!(Q::LandsatTmEtmCloudClear.accepts(S::UInt8(3)).is_err());
    assert!(!Q::LandsatTmEtmCloudClear.accepts(S::UInt8(0))?);
    assert_eq!(
        products::source_validity(&tm, tm.band("SR_CLOUD_QA")?, &cloud).collect::<Vec<_>>(),
        [false, true, true, true]
    );
    Ok(())
}

#[test]
fn water_policies_do_not_conflate_radar_and_optical_classes() -> Result<()> {
    let s1 = profile("opera-dswx-s1")?;
    assert_eq!(s1.band("DIAG")?.nodata.unwrap().as_f64(), 255.);
    assert!(s1.quality_policy("dswx_wtr_water").is_err());
    for code in [1, 3] {
        assert!(Q::DswxS1WtrWater.accepts(S::UInt8(code))?);
    }
    for code in [0, 250, 251, 254, 255] {
        assert!(!Q::DswxS1WtrWater.accepts(S::UInt8(code))?);
    }
    assert!(Q::DswxS1WtrWater.accepts(S::UInt8(2)).is_err());
    assert!(Q::DswxWtrWater.accepts(S::UInt8(3)).is_err());
    let id = "OPERA_L3_DSWx-S1_T17SQV_20261021T230515Z_20261023T070839Z_S1D_30_v1.0";
    s1.validate_identity(id, Some("sentinel-1d"), Some("L3"))?;
    s1.validate_datetime(id, "2026-10-21T23:05:34Z")?;
    assert!(s1.validate_datetime(id, "2026-10-22T23:05:34Z").is_err());
    assert!(s1
        .validate_identity(&id.replace("v1.0", "v1.1"), None, None)
        .is_err());
    assert!(s1
        .validate_identity(&format!("{id}_B04_WTR.tif"), None, None)
        .is_err());
    Ok(())
}

#[test]
fn disturbance_status_quality_confidence_and_missingness_are_separate() -> Result<()> {
    let p = profile("opera-dist-alert")?;
    assert!(p
        .band("VEG-DIST-CONF")?
        .description
        .contains("not a probability"));
    assert!(p.band("VEG-DIST-DATE")?.description.contains("2020-12-31"));
    let status = Pixels::UInt8(vec![0, 1, 3, 6, 7, 8, 255]);
    assert_eq!(
        products::source_validity(&p, p.band("VEG-DIST-STATUS")?, &status).collect::<Vec<_>>(),
        [true, true, true, true, true, true, false]
    );
    assert_eq!(
        products::derive_quality_mask(Q::DistVegConfirmed, &status)?,
        [0, 0, 1, 1, 1, 1, 0]
    );
    assert_eq!(
        products::derive_quality_mask(Q::DistVegConfirmedOngoing, &status)?,
        [0, 0, 1, 1, 0, 0, 0]
    );
    assert!(Q::DistGenConfirmed.accepts(S::UInt8(9)).is_err());
    let conf = Pixels::Int16(vec![-1, 0, 1000]);
    assert_eq!(
        products::source_validity(&p, p.band("VEG-DIST-CONF")?, &conf).collect::<Vec<_>>(),
        [false, true, true]
    );
    let mut request: products::ProductRequest = serde_json::from_value(
        serde_json::json!({"product":p.id,"aoi":{"kind":"bounds","crs":"EPSG:4326","bbox":[-79.,35.,-78.,36.]},"start":"2026-10-21T00:00:00Z","end":"2026-10-21T23:59:59Z","bands":["DATA-MASK"],"observations":[],"quality_policies":["dist_veg_confirmed"]}),
    )?;
    assert!(request.validate().is_err());
    request.quality_policies = vec!["dist_current_assessed".into()];
    request.validate()?;
    Ok(())
}

#[test]
fn wave2_canonical_sources_never_change_provider_after_selection() -> Result<()> {
    use products::routing::Provider;
    for path in [
        "docs/products/wave2/dswx-s1-request.json",
        "docs/products/wave2/dist-alert-request.json",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let mut r: products::ProductRequest = serde_json::from_slice(&std::fs::read(path)?)?;
        let c = products::discover(&r)?;
        assert_eq!(c.provider, Provider::Earthdata);
        assert!(c.routing.credentials_required);
        r.provider = Provider::PlanetaryComputer;
        assert!(products::discover(&r).is_err());
        r.provider = Provider::Auto;
        let asset = r.observations.as_mut().unwrap()[0]
            .assets
            .values_mut()
            .next()
            .unwrap();
        asset.href = asset.href.replace("_V1/", "_V2/");
        assert!(products::discover(&r).is_err());
    }
    Ok(())
}
