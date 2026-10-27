use std::collections::BTreeMap;

use tile_ripper::{
    products::{
        dist_ann::{self, Policy},
        profile::QualitySample,
        Observation, ProductAsset, ProductRequest,
    },
    Layout, Result, Spatial, Variable,
};

const ID: &str = "OPERA_L3_DIST-ANN-HLS_T17SNU_2025_20260109T155422Z_30_v1";
const START: &str = "2025-01-02T16:13:44.046Z";
const END: &str = "2025-12-26T16:23:45.583Z";
const BANDS: [&str; 6] = [
    "VEG-DIST-STATUS",
    "GEN-DIST-STATUS",
    "VEG-DIST-DATE",
    "GEN-DIST-DATE",
    "VEG-LAST-DATE",
    "GEN-LAST-DATE",
];

fn href(band: &str) -> String {
    format!(
        "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/{}/{}/{}_{}.tif",
        dist_ann::COLLECTION,
        ID,
        ID,
        band,
    )
}

fn observation() -> Observation {
    let assets = BANDS
        .iter()
        .map(|band| {
            (
                (*band).into(),
                ProductAsset {
                    href: href(band),
                    etag: None,
                    metadata: serde_json::json!({}),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    Observation {
        id: ID.into(),
        datetime: START.into(),
        collection: dist_ann::COLLECTION.into(),
        platform: None,
        processing: Some("L3".into()),
        assets,
        properties: serde_json::json!({"temporal_extent":{
            "coverage_start": START,
            "coverage_end": END,
            "summary_year": 2025
        }}),
    }
}

fn request() -> ProductRequest {
    ProductRequest {
        product: dist_ann::ID.into(),
        provider: Default::default(),
        aoi: Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [-80., 35., -79., 36.],
        },
        start: "2025-06-01T00:00:00Z".into(),
        end: "2025-06-02T00:00:00Z".into(),
        bands: BANDS.iter().map(|value| (*value).into()).collect(),
        output: None,
        observations: Some(vec![observation()]),
        max_observations: 1,
        buffer_bytes: 16 * 1024 * 1024,
        chunk_edge: 64,
        quality_policies: vec![
            Policy::VegConfirmed.name().into(),
            Policy::GenPreviousYearConfirmed.name().into(),
        ],
    }
}

#[test]
fn annual_layers_have_explicit_type_fill_and_date_duration_units() -> Result<()> {
    use tile_ripper::products::profile::{DataType, NoDataValue};
    let profile = dist_ann::profile();
    assert_eq!(profile.bands.len(), 21);
    for name in [
        "VEG-DIST-DATE",
        "GEN-DIST-DATE",
        "VEG-LAST-DATE",
        "GEN-LAST-DATE",
    ] {
        let band = profile.band(name)?;
        assert_eq!(band.dtype, DataType::Int16);
        assert_eq!(band.nodata, Some(NoDataValue::Int16(-1)));
        assert_eq!(band.unit, "days since 2020-12-31");
    }
    for name in ["VEG-DIST-DUR", "GEN-DIST-DUR"] {
        assert_eq!(profile.band(name)?.unit, "days");
    }
    assert_eq!(profile.band("VEG-ANOM-MAX")?.dtype, DataType::UInt8);
    assert_eq!(profile.band("GEN-ANOM-MAX")?.dtype, DataType::Int16);
    Ok(())
}

#[test]
fn annual_interval_is_real_coverage_not_a_summary_year_timestamp() -> Result<()> {
    let annual_observation = observation();
    let extent = dist_ann::temporal_extent(&annual_observation)?;
    assert_eq!(
        extent.coverage_start.to_rfc3339(),
        "2025-01-02T16:13:44.046+00:00"
    );
    assert_eq!(
        extent.coverage_end.to_rfc3339(),
        "2025-12-26T16:23:45.583+00:00"
    );
    assert_eq!(extent.summary_year, 2025);
    assert_eq!(dist_ann::summary_year(ID)?, 2025);
    assert!(dist_ann::validate_observation(&request(), &annual_observation).is_ok());

    let mut wrong = annual_observation.clone();
    wrong.datetime = "2025-01-01T00:00:00Z".into();
    assert!(dist_ann::temporal_extent(&wrong).is_err());
    wrong = annual_observation.clone();
    wrong.properties["temporal_extent"]["summary_year"] = serde_json::json!(2024);
    assert!(dist_ann::validate_observation(&request(), &wrong).is_err());
    wrong = annual_observation;
    wrong.properties["temporal_extent"]["coverage_end"] = serde_json::json!("2025-01-02T16:13:43Z");
    assert!(dist_ann::temporal_extent(&wrong).is_err());
    let mut nonoverlap = request();
    nonoverlap.start = "2026-01-01T00:00:00Z".into();
    nonoverlap.end = "2026-01-02T00:00:00Z".into();
    assert!(dist_ann::validate_observation(&nonoverlap, &observation()).is_err());
    Ok(())
}

#[test]
fn annual_status_policies_keep_previous_year_semantics_explicit() -> Result<()> {
    for code in [3, 6, 7, 8, 9, 10] {
        assert!(Policy::VegConfirmed.accepts(QualitySample::UInt8(code))?);
    }
    for code in [3, 6, 7, 8] {
        assert!(Policy::VegCurrentYearConfirmed.accepts(QualitySample::UInt8(code))?);
        assert!(!Policy::VegPreviousYearConfirmed.accepts(QualitySample::UInt8(code))?);
    }
    for code in [9, 10] {
        assert!(!Policy::VegCurrentYearConfirmed.accepts(QualitySample::UInt8(code))?);
        assert!(Policy::VegPreviousYearConfirmed.accepts(QualitySample::UInt8(code))?);
    }
    for code in [0, 255] {
        assert!(!Policy::GenConfirmed.accepts(QualitySample::UInt8(code))?);
    }
    assert!(Policy::VegConfirmed
        .accepts(QualitySample::UInt8(1))
        .is_err());
    assert!(Policy::VegConfirmed
        .accepts(QualitySample::Int16(3))
        .is_err());
    Ok(())
}

fn layout(dtype: &str, nodata: Option<f64>) -> Layout {
    Layout {
        width: 3660,
        height: 3660,
        transform: [499980., 30., 0., 3900000., 0., -30.],
        crs: "EPSG:32617".into(),
        variable: Variable::Band { index: 1 },
        block: [256, 256],
        dtype: dtype.into(),
        nodata,
        scale: Some(1.),
        offset: Some(0.),
        tiles: vec![],
    }
}

fn source(band: &str, nodata: &str) -> serde_json::Value {
    let tags = if band.ends_with("STATUS") {
        "<Item name=\"flag_values\">0,3,6,7,8,9,10,255</Item><Item name=\"flag_meanings\">no_disturbance,confirmed_previous_year,no_data</Item>"
    } else {
        "<Item name=\"Units\">days</Item><Item name=\"Valid_min\">0</Item><Item name=\"Valid_max\">1821</Item>"
    };
    serde_json::json!({"object_version":"\"fixture-etag\"","object_identities":{"native_cog":{
        "gdal_nodata_text":nodata,
        "gdal_metadata_xml":format!("<GDALMetadata>{tags}</GDALMetadata>")
    }}})
}

#[test]
fn annual_physical_gate_binds_native_metadata_to_layer_and_interval() -> Result<()> {
    let observation = observation();
    let profile = dist_ann::profile();
    let band = profile.band("VEG-DIST-STATUS")?;
    let asset = observation.assets.get(&band.name).unwrap();
    dist_ann::validate_physical(
        &observation,
        band,
        asset,
        &source(&band.name, "255"),
        &layout("Byte", Some(255.)),
    )?;
    // The provider does not repeat layer identity in status TIFF metadata.
    // Bind a named layer through its exact canonical object, not invented tags.
    let mut wrong_asset = asset.clone();
    wrong_asset.href = href("GEN-DIST-STATUS");
    assert!(dist_ann::validate_physical(
        &observation,
        band,
        &wrong_asset,
        &source("GEN-DIST-STATUS", "255"),
        &layout("Byte", Some(255.)),
    )
    .is_err());
    assert!(dist_ann::validate_physical(
        &observation,
        band,
        asset,
        &source(&band.name, "255"),
        &layout("Byte", Some(255.)),
    )
    .is_ok());
    let date = profile.band("GEN-DIST-DATE")?;
    let asset = observation.assets.get(&date.name).unwrap();
    assert!(dist_ann::validate_physical(
        &observation,
        date,
        asset,
        &source(&date.name, "-1"),
        &layout("Int16", Some(-1.)),
    )
    .is_ok());
    let mut bad_layout = layout("Byte", Some(255.));
    bad_layout.block = [512, 512];
    assert!(dist_ann::validate_physical(
        &observation,
        band,
        observation.assets.get(&band.name).unwrap(),
        &source(&band.name, "255"),
        &bad_layout,
    )
    .is_err());
    Ok(())
}
