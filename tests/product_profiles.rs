use tile_ripper::{
    products::{
        self,
        profile::{
            profile, profiles, DataType, NoDataValue, QualityPolicy, QualitySample, HLS_L30_V2,
            HLS_S30_V2, LANDSAT_C2_L2_SR, OPERA_DSWX_HLS_V1, OPERA_RTC_S1_V1,
        },
    },
    Pixels, Result,
};

#[test]
fn profiles_are_serializable_declarations_not_capabilities() -> Result<()> {
    let profiles = profiles();
    assert_eq!(profiles.len(), 12);
    assert_eq!(
        profiles.iter().map(|p| p.id).collect::<Vec<_>>(),
        [
            LANDSAT_C2_L2_SR,
            "landsat-tm-c2-l2-sr",
            "landsat-etm-c2-l2-sr",
            HLS_L30_V2,
            HLS_S30_V2,
            OPERA_DSWX_HLS_V1,
            OPERA_RTC_S1_V1,
            "opera-dswx-s1",
            "opera-dist-alert",
            "opera-dswx-ni-prototype",
            "opera-dist-alert-s1",
            "opera-dist-ann-hls",
        ]
    );
    for profile in profiles {
        let value = serde_json::to_value(&profile)?;
        assert_eq!(value["native_grid"]["posting_m"], 30);
        assert!(value["capability_caveat"]
            .as_str()
            .unwrap()
            .contains("grants no acquisition"));
        assert!(value["bands"].as_array().is_some_and(|v| !v.is_empty()));
        assert!(value["mission"].as_str().is_some_and(|v| !v.is_empty()));
        assert!(value["instrument"].as_str().is_some_and(|v| !v.is_empty()));
        assert!(value["bands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|band| band["description"]
                .as_str()
                .is_some_and(|description| !description.is_empty())));
        assert!(profile.bands.iter().all(|band| [
            "1",
            "m",
            "degree",
            "degree_Celsius",
            "days since 2020-12-31",
            "days",
            "count",
            "%"
        ]
        .contains(&band.unit)));
    }
    Ok(())
}

#[test]
fn aliases_resolve_to_exact_source_bands_and_catalog_assets() -> Result<()> {
    let landsat = profile(LANDSAT_C2_L2_SR)?;
    assert_eq!(landsat.band("red")?.name, "SR_B4");
    assert_eq!(landsat.band("red")?.asset_key, "red");
    assert_eq!(landsat.band("qa_pixel")?.name, "QA_PIXEL");
    assert_eq!(landsat.band("QA_PIXEL")?.asset_key, "qa_pixel");
    assert_eq!(
        landsat.band("QA_PIXEL")?.nodata,
        Some(NoDataValue::UInt16(1))
    );

    let l30 = profile(HLS_L30_V2)?;
    assert_eq!(l30.band("nir")?.name, "B05");
    assert_eq!(l30.band("B05")?.dtype, DataType::Int16);
    assert_eq!(serde_json::to_value(DataType::Int16)?, "int16");
    assert_eq!(l30.band("B05")?.scale, Some(0.0001));
    assert_eq!(l30.band("B05")?.nodata, Some(NoDataValue::Int16(-9999)));

    let dswx = profile(OPERA_DSWX_HLS_V1)?;
    assert_eq!(dswx.catalog.version, "1.0");
    assert_eq!(dswx.processing_versions, ["1.0", "1.1"]);
    let old = "OPERA_L3_DSWx-HLS_T10TCT_20211211T191811Z_20241025T180315Z_S2A_30_v1.0";
    let current = "OPERA_L3_DSWx-HLS_T17SPU_20261018T154740Z_20261022T014315Z_L8_30_v1.1";
    assert_eq!(dswx.processing_version(old)?, "1.0");
    assert_eq!(dswx.processing_version(current)?, "1.1");
    dswx.validate_datetime(current, "2026-10-18T15:47:40.202Z")?;
    assert!(dswx
        .processing_version(&current.replace("v1.1", "v1.2"))
        .is_err());

    let s30 = profile(HLS_S30_V2)?;
    assert_eq!(s30.band("nir")?.name, "B8A");
    assert_eq!(s30.band("nir_narrow")?.name, "B8A");
    assert_eq!(s30.band("nir_broad")?.name, "B08");
    assert_ne!(s30.band("B8A")?.name, s30.band("B08")?.name);
    assert!(s30.band("B8").is_err());
    assert!(s30.band("not_a_band").is_err());
    Ok(())
}

#[test]
fn landsat_product_validity_uses_fill_bit_without_reclassifying_zero() -> Result<()> {
    let landsat = profile(LANDSAT_C2_L2_SR)?;

    let qa_pixel = Pixels::UInt16(vec![0, 1, 21_824, 3]);
    assert_eq!(
        products::source_validity(&landsat, landsat.band("QA_PIXEL")?, &qa_pixel)
            .collect::<Vec<_>>(),
        [true, false, true, false]
    );

    let aerosol = Pixels::UInt8(vec![0, 1, 2, 3]);
    assert_eq!(
        products::source_validity(&landsat, landsat.band("SR_QA_AEROSOL")?, &aerosol,)
            .collect::<Vec<_>>(),
        [true, false, true, false]
    );

    let radsat = Pixels::UInt16(vec![0, 1]);
    assert_eq!(
        products::source_validity(&landsat, landsat.band("QA_RADSAT")?, &radsat)
            .collect::<Vec<_>>(),
        [true, true]
    );
    Ok(())
}

#[test]
fn granule_schemas_and_observation_times_are_bound() -> Result<()> {
    let landsat = profile(LANDSAT_C2_L2_SR)?;
    let landsat_id = "LC08_L2SP_016035_20240629_02_T1";
    landsat.validate_datetime(landsat_id, "2024-06-29T15:52:43Z")?;
    assert!(landsat
        .validate_datetime(landsat_id, "2024-06-28T15:52:43Z")
        .is_err());

    let hls = profile(HLS_S30_V2)?;
    let hls_id = "HLS.S30.T17SPV.2022023T160551.v2.0";
    // The S30 nominal identifier token and tile sensing time need not have the
    // same clock time, but they must identify the same UTC calendar day.
    hls.validate_datetime(hls_id, "2022-01-23T16:13:26.247240Z")?;
    assert!(hls
        .validate_identity("HLS.S30.garbage.V2.0", None, None)
        .is_err());
    assert!(hls
        .validate_datetime(hls_id, "2022-01-24T16:13:26Z")
        .is_err());

    let dswx = profile(OPERA_DSWX_HLS_V1)?;
    let dswx_id = "OPERA_L3_DSWx-HLS_T10TCT_20211211T191811Z_20241025T180315Z_S2A_30_v1.0";
    dswx.validate_datetime(dswx_id, "2021-12-11T19:18:11Z")?;
    dswx.validate_datetime(dswx_id, "2021-12-11T19:18:11.202827Z")?;
    assert!(dswx
        .validate_datetime(dswx_id, "2021-12-11T19:18:12Z")
        .is_err());
    assert!(dswx
        .validate_identity("OPERA_L3_DSWx-HLS_x_30_v1.0", None, None)
        .is_err());

    let rtc = profile(OPERA_RTC_S1_V1)?;
    assert!(rtc
        .validate_identity(
            "OPERA_L2_RTC-S1_T001-x-IW3_20240110T181405Z_20240410T013737Z_S1A_30_v1.0",
            None,
            None,
        )
        .is_err());
    Ok(())
}

#[test]
fn landsat_identity_binds_platform_collection_and_processing() -> Result<()> {
    let p = profile(LANDSAT_C2_L2_SR)?;
    let l8 = "LC08_L2SP_042034_20200906_20200919_02_T1";
    p.validate_identity(l8, Some("landsat-8"), Some("L2SP"))?;
    p.validate_identity(
        "LC08_L2SP_016035_20240629_02_T1",
        Some("landsat-8"),
        Some("L2SP"),
    )?;
    p.validate_identity(
        "LC09_L2SR_015033_20240715_20240717_02_T2_SR_B4.TIF",
        Some("LANDSAT_9"),
        Some("L2SR"),
    )?;
    assert!(p
        .validate_identity(l8, Some("LANDSAT_9"), Some("L2SP"))
        .is_err());
    assert!(p
        .validate_identity(l8, Some("LANDSAT_8"), Some("L1TP"))
        .is_err());
    assert!(p
        .validate_identity("LC07_L2SP_042034_20200906_20200919_02_T1", None, None)
        .is_err());
    assert!(p
        .validate_identity("LC08_L2SP_042034_20200906_20200919_01_T1", None, None)
        .is_err());
    Ok(())
}

#[test]
fn release_specific_non_landsat_identities_are_narrow() -> Result<()> {
    profile(HLS_L30_V2)?.validate_identity(
        "HLS.L30.T17SLU.2020209T155956.V2.0",
        Some("LANDSAT_8"),
        Some("L30"),
    )?;
    assert!(profile(HLS_S30_V2)?
        .validate_identity(
            "HLS.S30.T17SLU.2020117T160901.V1.5",
            Some("SENTINEL_2A"),
            Some("S30")
        )
        .is_err());
    assert!(profile(HLS_L30_V2)?
        .validate_identity("HLS.L30.T17SLU.2020209T155956.V2.01", None, None)
        .is_err());
    profile(OPERA_DSWX_HLS_V1)?.validate_identity(
        "OPERA_L3_DSWx-HLS_T11SLT_20230701T184921Z_20230703T010101Z_S2B_30_v1.0_WTR.tif",
        Some("SENTINEL_2B"),
        Some("L3"),
    )?;
    profile(OPERA_RTC_S1_V1)?.validate_identity(
        "OPERA_L2_RTC-S1_T069-147170-IW1_20210205T163901Z_20220101T140222Z_S1A_30_v1.0_VV.tif",
        Some("SENTINEL_1A"),
        Some("L2"),
    )?;
    assert!(profile(OPERA_RTC_S1_V1)?
        .validate_identity(
            "OPERA_L2_RTC-S1_T069-147170-IW1_20210205T163901Z_20220101T140222Z_S1A_30_v1.01_VV.tif",
            None,
            None,
        )
        .is_err());
    Ok(())
}

#[test]
fn landsat_pixel_policy_rejects_reserved_confidence_states() -> Result<()> {
    let policy = QualityPolicy::LandsatPixelClear;
    let documented_clear_land = 21_824_u16;
    assert!(policy.accepts(QualitySample::UInt16(documented_clear_land))?);

    for shift in [10, 12, 14] {
        let reserved = (documented_clear_land & !(3 << shift)) | (2 << shift);
        assert!(policy.accepts(QualitySample::UInt16(reserved)).is_err());
    }

    let cloud = (documented_clear_land & !(1 << 6) & !(3 << 8)) | (1 << 3) | (3 << 8);
    assert!(!policy.accepts(QualitySample::UInt16(cloud))?);
    assert!(!policy.accepts(QualitySample::UInt16(1))?);
    Ok(())
}

#[test]
fn landsat_radsat_and_aerosol_use_only_documented_codes() -> Result<()> {
    let radsat = QualityPolicy::LandsatRadiometricallyUnsaturated;
    assert!(radsat.accepts(QualitySample::UInt16(0))?);
    assert!(!radsat.accepts(QualitySample::UInt16(1 << 4))?);
    assert!(radsat.accepts(QualitySample::UInt16(1 << 7)).is_err());

    let valid = QualityPolicy::LandsatAerosolValid;
    let recommended = QualityPolicy::LandsatAerosolRecommended;
    assert!(valid.accepts(QualitySample::UInt8(194))?);
    assert!(valid.accepts(QualitySample::UInt16(194))?);
    assert!(!recommended.accepts(QualitySample::UInt16(194))?);
    assert!(recommended.accepts(QualitySample::UInt16(130))?);
    assert!(!valid.accepts(QualitySample::UInt16(160))?);
    assert!(valid.accepts(QualitySample::UInt16(34)).is_err());
    Ok(())
}

#[test]
fn hls_dswx_and_rtc_policies_preserve_separate_source_semantics() -> Result<()> {
    let hls: QualityPolicy = "hls_fmask_clear_low_aerosol".parse()?;
    assert!(hls.accepts(QualitySample::UInt8(0))?);
    assert!(hls.accepts(QualitySample::UInt8(64))?);
    assert!(!hls.accepts(QualitySample::UInt8(128))?);
    assert!(!hls.accepts(QualitySample::UInt8(255))?);
    assert!(hls.accepts(QualitySample::UInt8(1)).is_err());

    assert!(QualityPolicy::DswxWtrWater.accepts(QualitySample::UInt8(2))?);
    assert!(!QualityPolicy::DswxWtrWater.accepts(QualitySample::UInt8(253))?);
    assert!(QualityPolicy::DswxWtrWater
        .accepts(QualitySample::UInt8(3))
        .is_err());
    assert!(!QualityPolicy::DswxConfUnmaskedWater.accepts(QualitySample::UInt8(11))?);
    assert!(QualityPolicy::DswxConfAnyWater.accepts(QualitySample::UInt8(11))?);
    assert!(QualityPolicy::DswxConfAnyWater
        .accepts(QualitySample::UInt8(9))
        .is_err());
    assert!(QualityPolicy::DswxShadowFree.accepts(QualitySample::UInt8(1))?);
    assert!(!QualityPolicy::DswxShadowFree.accepts(QualitySample::UInt8(0))?);
    for code in [0, 8] {
        assert!(QualityPolicy::DswxCloudObstructionFree.accepts(QualitySample::UInt8(code))?);
    }
    for code in [1, 4, 10, 254, 255] {
        assert!(!QualityPolicy::DswxCloudObstructionFree.accepts(QualitySample::UInt8(code))?);
    }
    assert!(QualityPolicy::DswxCloudObstructionFree
        .accepts(QualitySample::UInt8(16))
        .is_err());

    assert!(QualityPolicy::OperaRtcFinite.accepts(QualitySample::Float32(0.0))?);
    assert!(!QualityPolicy::OperaRtcFinite.accepts(QualitySample::Float32(f32::NAN))?);
    assert!(QualityPolicy::OperaRtcMaskValid.accepts(QualitySample::UInt8(3))?);
    assert!(!QualityPolicy::OperaRtcMaskUnaffected.accepts(QualitySample::UInt8(3))?);
    assert!(QualityPolicy::OperaRtcMaskValid
        .accepts(QualitySample::UInt8(4))
        .is_err());
    assert!(QualityPolicy::OperaRtcFinite
        .accepts(QualitySample::UInt8(0))
        .is_err());
    Ok(())
}
