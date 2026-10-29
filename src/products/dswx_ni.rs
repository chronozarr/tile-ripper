//! Pinned, public DSWx-NI engineering-output prototype.
//!
//! This is intentionally not a production OPERA collection profile.  It binds
//! exactly one successful public engineering job, its four public TIFF objects,
//! and the v0.1/0.3.0 metadata observed on those objects.
use super::{profile, Observation, ProductAsset, ProductRequest};
use crate::{fail, Layout, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ID: &str = "opera-dswx-ni-prototype";
pub const CANONICAL_COLLECTION: &str = "OPERA_DSWX_NI_V0_1_PUBLIC_PROTOTYPE";
pub const GRANULE: &str = "OPERA_L3_DSWx-NI_T42RVP_20260112T004524Z_20260417T094901Z_LSAR_30_v0.1";
pub const PREFIX: &str = "https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/transfer/rileykk/dswx_ni/job-WF-SCIFLO_L3_DSWx_NI-MS_41_15-10-20260416T152808.731058Z/pge_output_dir/";

const BANDS: [(&str, &str, &str); 4] = [
    ("WTR", "B01_WTR", "\"4c24ac7234e0d63d71ade7f8269b455d\""),
    ("BWTR", "B02_BWTR", "\"63c17a56666857346c31dd92df615ffd\""),
    ("CONF", "B03_CONF", "\"5bcf61a8f64146e91a7d50984b4162d7\""),
    ("DIAG", "B04_DIAG", "\"f67e4e123f22f440267381d6bd2ae57d\""),
];

/// Explicit masks on raw NI classes. These never replace source validity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    WtrOpenWater,
    WtrWater,
    WtrEdgeWater,
    BwtrWater,
}

impl Policy {
    pub const fn name(self) -> &'static str {
        match self {
            Self::WtrOpenWater => "dswx_ni_wtr_openwater",
            Self::WtrWater => "dswx_ni_wtr_water",
            Self::WtrEdgeWater => "dswx_ni_wtr_edgewater",
            Self::BwtrWater => "dswx_ni_bwtr_water",
        }
    }

    pub const fn required_bands(self) -> &'static [&'static str] {
        match self {
            Self::WtrOpenWater | Self::WtrWater | Self::WtrEdgeWater => &["WTR"],
            Self::BwtrWater => &["BWTR"],
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::WtrOpenWater => {
                "NI WTR class 1 only (open water); contextual masks remain raw-valid source values"
            }
            Self::WtrWater => {
                "NI WTR classes 1 (open water) and 3 (inundated vegetation); partial-water class 8 excluded"
            }
            Self::WtrEdgeWater => {
                "NI WTR classes 1 (open water), 3 (inundated vegetation), and 8 (partial/water-edge); this is a delivered class mask, not a fractional-water estimate"
            }
            Self::BwtrWater => {
                "NI BWTR class 1 only; HAND, layover/shadow, ocean, and fill are not selected"
            }
        }
    }

    pub fn accepts(self, sample: profile::QualitySample) -> Result<bool> {
        let profile::QualitySample::UInt8(value) = sample else {
            return fail("DSWx-NI quality policy requires UInt8 raw classes");
        };
        let domain = match self {
            Self::WtrOpenWater | Self::WtrWater | Self::WtrEdgeWater => {
                &[0, 1, 3, 8, 250, 251, 254, 255][..]
            }
            Self::BwtrWater => &[0, 1, 250, 251, 254, 255][..],
        };
        if !domain.contains(&value) {
            return fail("undefined DSWx-NI class for the selected layer");
        }
        Ok(match self {
            Self::WtrOpenWater => value == 1,
            Self::WtrWater => matches!(value, 1 | 3),
            Self::WtrEdgeWater => matches!(value, 1 | 3 | 8),
            Self::BwtrWater => value == 1,
        })
    }
}

pub fn profile() -> profile::ProductProfile {
    let band =
        |name: &str, role: profile::BandRole, description: &'static str| profile::BandProfile {
            name: name.into(),
            asset_key: name.into(),
            aliases: &[],
            role,
            description,
            dtype: profile::DataType::UInt8,
            nodata: Some(profile::NoDataValue::UInt8(255)),
            scale: Some(1.),
            offset: Some(0.),
            unit: "1",
            optional: false,
        };
    profile::ProductProfile {
        id: ID,
        family: profile::ProductFamily::OperaDswxNiPrototype,
        mission: "NASA OPERA NISAR engineering prototype",
        instrument: "NISAR L-band SAR GCOV input",
        title: "OPERA DSWx-NI v0.1 public engineering-output prototype",
        version: "0.1",
        processing_versions: &["0.1"],
        native_grid: profile::NativeGrid {
            posting_m: 30,
            projection: "EPSG:32642 / WGS 84 UTM zone 42N",
            organization: "MGRS tile; exactly pinned T42RVP prototype object set",
        },
        catalog: profile::CatalogIdentity {
            provider: "OPERA public-reference S3 transfer object",
            catalog: "explicit pinned observation only; no catalog discovery",
            collection: CANONICAL_COLLECTION,
            version: "prototype 0.1",
            access: "anonymous HTTPS only for the four version-pinned public engineering COGs",
            platforms: &["NISAR"],
            processing_levels: &["L3", "DSWx-NI"],
        },
        bands: vec![
            band(
                "WTR",
                profile::BandRole::Classification,
                "NI interpreted water classes: 0 not water, 1 open water, 3 inundated vegetation, 8 partial/water-edge, 250 HAND, 251 layover/shadow, 254 ocean, 255 fill",
            ),
            band(
                "BWTR",
                profile::BandRole::Classification,
                "NI binary water class 1 with distinct contextual HAND, layover/shadow, ocean, and fill codes",
            ),
            band(
                "CONF",
                profile::BandRole::Confidence,
                "NI categorical algorithm and wetland classes, not a probability; source-native codes are retained",
            ),
            band(
                "DIAG",
                profile::BandRole::Diagnostic,
                "NI rounded fuzzy water score 0 through 100, with contextual masks and fill retained",
            ),
        ],
        quality_policies: vec![
            profile::QualityPolicy::DswxNi(Policy::WtrOpenWater),
            profile::QualityPolicy::DswxNi(Policy::WtrWater),
            profile::QualityPolicy::DswxNi(Policy::WtrEdgeWater),
            profile::QualityPolicy::DswxNi(Policy::BwtrWater),
        ],
        capability_caveat: "This profile declaration grants no acquisition by itself. Pinned public engineering-output prototype only: no production collection/discovery, protected routing, final DSWx-NI v1.0, cross-sensor equivalence, calibration, or algorithm rerun is qualified.",
    }
}

pub fn canonical_collection() -> &'static str {
    CANONICAL_COLLECTION
}

pub fn filename_for_band(band: &str) -> Result<String> {
    let (_, suffix, _) = band_spec(band)?;
    Ok(format!("{GRANULE}_{suffix}.tif"))
}

pub fn band_href(band: &str) -> Result<String> {
    Ok(format!("{PREFIX}{}", filename_for_band(band)?))
}

pub fn validate_identity(
    granule: &str,
    platform: Option<&str>,
    processing: Option<&str>,
) -> Result<()> {
    let parts = granule
        .strip_suffix(".tif")
        .unwrap_or(granule)
        .split('_')
        .collect::<Vec<_>>();
    if parts.len() < 9
        || parts[..3] != ["OPERA", "L3", "DSWx-NI"]
        || parts[3] != "T42RVP"
        || parts[4] != "20260112T004524Z"
        || parts[5] != "20260417T094901Z"
        || parts[6] != "LSAR"
        || parts[7] != "30"
        || parts[8] != "v0.1"
    {
        return fail("granule is not the exact public DSWx-NI v0.1 prototype identity");
    }
    if parts.len() > 9 {
        let layer = parts[9..].join("_");
        if !BANDS.iter().any(|(_, expected, _)| *expected == layer) {
            return fail("DSWx-NI asset layer is not one of the four pinned prototype layers");
        }
    }
    if platform.is_some_and(|value| !value.eq_ignore_ascii_case("NISAR")) {
        return fail("DSWx-NI prototype platform must be NISAR");
    }
    if processing.is_some_and(|value| value != "L3" && value != "DSWx-NI") {
        return fail("DSWx-NI prototype processing must be L3 or DSWx-NI");
    }
    Ok(())
}

pub fn validate_datetime(granule: &str, datetime: &str) -> Result<()> {
    validate_identity(granule, None, None)?;
    let observed = chrono::DateTime::parse_from_rfc3339(datetime)?;
    let nominal = chrono::DateTime::parse_from_rfc3339("2026-01-12T00:45:24Z")?;
    if observed.timestamp() != nominal.timestamp() {
        return fail(
            "DSWx-NI observation datetime must retain the nominal filename acquisition timestamp",
        );
    }
    Ok(())
}

pub fn validate_observation(request: &ProductRequest, observation: &Observation) -> Result<()> {
    validate_identity(
        &observation.id,
        observation.platform.as_deref(),
        observation.processing.as_deref(),
    )?;
    validate_datetime(&observation.id, &observation.datetime)?;
    if observation.collection != CANONICAL_COLLECTION {
        return fail("DSWx-NI prototype accepts only its explicit pinned pseudo-collection");
    }
    if request.provider != super::routing::Provider::Pinned {
        return fail("DSWx-NI prototype requires explicit pinned routing; auto and production routes are refused");
    }
    for (name, _, etag) in BANDS {
        let asset = observation
            .assets
            .get(name)
            .ok_or("DSWx-NI pinned observation must retain all four source layers")?;
        validate_asset(name, asset, etag)?;
    }
    let profile = profile();
    for requested in &request.bands {
        let band = profile.band(requested)?;
        if !observation.assets.contains_key(&band.name) {
            return fail("DSWx-NI requested band is absent from the pinned observation");
        }
    }
    validate_time_fields(&observation.properties)
}

/// Validate the retained native COG metadata and layout attached to a source.
/// The parent product gate supplies the full source provenance object because
/// its `object_identities.native_cog` child carries the GDAL XML/GeoKeys.
pub fn validate_physical(
    observation: &Observation,
    band: &profile::BandProfile,
    asset: &ProductAsset,
    source: &Value,
    layout: &Layout,
) -> Result<()> {
    validate_observation_identity_only(observation)?;
    let (_, _, etag) = band_spec(&band.name)?;
    validate_asset(&band.name, asset, etag)?;
    if !matches!(layout.dtype.as_str(), "Byte" | "UInt8")
        || layout.nodata != Some(255.)
        || layout.width != 3660
        || layout.height != 3660
        || layout.block != [512, 512]
        || layout.crs != "EPSG:32642"
        || layout.transform != [399960., 30., 0., 2900040., 0., -30.]
    {
        return fail(
            "DSWx-NI prototype physical grid/type/nodata differs from the pinned specimen",
        );
    }
    let physical = source
        .pointer("/object_identities/native_cog")
        .or_else(|| source.get("physical_metadata"))
        .ok_or("DSWx-NI source lacks retained native COG physical metadata")?;
    let xml = physical["gdal_metadata_xml"]
        .as_str()
        .ok_or("DSWx-NI source lacks retained GDAL metadata XML")?;
    let layer_description = match band.name.as_str() {
        "WTR" => "Water classification (WTR)",
        "BWTR" => "Binary Water classification (BWTR)",
        "CONF" => "Confidence values (CONF)",
        "DIAG" => "Diagnostic layer (DIAG)",
        _ => return fail("invalid DSWx-NI band"),
    };
    if item(xml, "PROJECT") != Some("OPERA")
        || item(xml, "PRODUCT_TYPE") != Some("DSWx-NI")
        || item(xml, "DSWX_PRODUCT_VERSION") != Some("0.1")
        || item(xml, "SOFTWARE_VERSION") != Some("0.3.0")
        || item(xml, "PRODUCT_SOURCE") != Some("NISAR_GCOV")
        || item(xml, "SENSOR") != Some("LSAR")
        || item(xml, "SPACECRAFT_NAME") != Some("NISAR")
        || item(xml, "DESCRIPTION") != Some(layer_description)
        || item(xml, "ZERO_DOPPLER_START_TIME") != Some("2026-01-12T00:44:50Z")
        || item(xml, "ZERO_DOPPLER_END_TIME") != Some("2026-01-12T00:45:58Z")
        || item(xml, "PROCESSING_DATETIME") != Some("2026-04-17T09:49:12Z")
    {
        return fail("DSWx-NI prototype physical identity, layer, or retained temporal lineage differs from the pinned specimen");
    }
    Ok(())
}

fn validate_observation_identity_only(observation: &Observation) -> Result<()> {
    validate_identity(
        &observation.id,
        observation.platform.as_deref(),
        observation.processing.as_deref(),
    )?;
    validate_datetime(&observation.id, &observation.datetime)?;
    if observation.collection != CANONICAL_COLLECTION {
        return fail("DSWx-NI prototype collection identity changed");
    }
    Ok(())
}

fn validate_asset(band: &str, asset: &ProductAsset, expected_etag: &str) -> Result<()> {
    let expected = band_href(band)?;
    if asset.href != expected || asset.etag.as_deref() != Some(expected_etag) {
        return fail("DSWx-NI prototype asset locator or ETag differs from the pinned specimen");
    }
    Ok(())
}

fn band_spec(band: &str) -> Result<(&'static str, &'static str, &'static str)> {
    BANDS
        .iter()
        .copied()
        .find(|(name, _, _)| name.eq_ignore_ascii_case(band))
        .ok_or_else(|| "unknown DSWx-NI prototype band".into())
}

fn validate_time_fields(properties: &Value) -> Result<()> {
    let expected = [
        ("mosaic_start_datetime", "2026-01-12T00:44:50Z"),
        ("mosaic_end_datetime", "2026-01-12T00:45:58Z"),
        ("filename_processing_datetime", "2026-04-17T09:49:01Z"),
        ("source_processing_datetime", "2026-04-17T09:49:12Z"),
    ];
    for (key, value) in expected {
        if properties[key] != value {
            return fail("DSWx-NI prototype observation must retain distinct nominal, mosaic, and processing time fields");
        }
    }
    Ok(())
}

fn item<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!(r#"<Item name="{name}""#);
    let tail = xml.get(xml.find(&marker)? + marker.len()..)?;
    let value = tail.get(tail.find('>')? + 1..)?;
    value.get(..value.find("</Item>")?)
}
