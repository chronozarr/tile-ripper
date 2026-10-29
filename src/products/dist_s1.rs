//! Narrow identity and physical-source contract for OPERA DIST-ALERT-S1 v1.0.
//!
//! This is intentionally separate from DIST-ALERT-HLS: its ten Sentinel-1
//! layers, status classes, floating metrics, and no-data conventions come from
//! the DIST-S1 producer, not the optical DIST product.
use super::{
    profile::{
        BandProfile, BandRole, CatalogIdentity, DataType, NativeGrid, NoDataValue, ProductFamily,
        ProductProfile, QualityPolicy, QualitySample,
    },
    Observation, ProductAsset, ProductRequest,
};
use crate::{fail, Layout, Result};
use serde_json::Value;

pub const ID: &str = "opera-dist-alert-s1";
const COLLECTION: &str = "OPERA_L3_DIST-ALERT-S1_V1";
const VERSION: &str = "1";
const RELEASE: &str = "1.0";

/// A closed mask over the generic DIST-S1 status layer. These policies do not
/// threshold `GEN-DIST-CONF`, which is a source metric rather than a calibrated
/// probability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Confirmed,
    ConfirmedOngoing,
}

impl Policy {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Confirmed => "dist_s1_confirmed",
            Self::ConfirmedOngoing => "dist_s1_confirmed_ongoing",
        }
    }

    pub const fn required_bands(self) -> &'static [&'static str] {
        &["GEN-DIST-STATUS"]
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Confirmed => {
                "DIST-S1 generic disturbance statuses confirmed low/high (3,6) or confirmed finished (7,8); raw status is retained"
            }
            Self::ConfirmedOngoing => {
                "DIST-S1 generic disturbance statuses confirmed and ongoing (3,6); no confidence threshold is applied"
            }
        }
    }

    pub fn accepts(self, sample: QualitySample) -> Result<bool> {
        let QualitySample::UInt8(value) = sample else {
            return fail("DIST-S1 status policy requires a UInt8 GEN-DIST-STATUS sample");
        };
        if !matches!(value, 0..=8 | 255) {
            return fail("undefined DIST-S1 status class");
        }
        Ok(match self {
            Self::Confirmed => matches!(value, 3 | 6 | 7 | 8),
            Self::ConfirmedOngoing => matches!(value, 3 | 6),
        })
    }
}

pub fn profile() -> ProductProfile {
    let categorical = |name, aliases, role, nodata, unit| {
        super::profile::band(
            name,
            aliases,
            role,
            DataType::UInt8,
            Some(NoDataValue::UInt8(nodata)),
            1.0,
            0.0,
            unit,
        )
    };
    let floating = |name, role, description| {
        let mut band = super::profile::band(
            name,
            &[],
            role,
            DataType::Float32,
            Some(NoDataValue::Float32NaN),
            1.0,
            0.0,
            "1",
        );
        band.description = description;
        band
    };
    let days = |name, role, unit, description| {
        let mut band = super::profile::band(
            name,
            &[],
            role,
            DataType::Int16,
            Some(NoDataValue::Int16(-1)),
            1.0,
            0.0,
            unit,
        );
        band.description = description;
        band
    };
    let mut status = categorical(
        "GEN-DIST-STATUS",
        &["status", "generic_disturbance_status"],
        BandRole::Classification,
        255,
        "1",
    );
    status.description = "Generic Sentinel-1 disturbance status: 0 none; 1/4 first low/high; 2/5 provisional low/high; 3/6 confirmed low/high; 7/8 confirmed finished; 255 missing";
    let mut status_acq = categorical(
        "GEN-DIST-STATUS-ACQ",
        &["status_acq", "generic_disturbance_status_acquisition"],
        BandRole::Classification,
        255,
        "1",
    );
    status_acq.description = "Generic Sentinel-1 disturbance status for the latest acquisition; uses the same closed 0..8 and 255 class domain as GEN-DIST-STATUS";
    let mut count = categorical(
        "GEN-DIST-COUNT",
        &["count", "generic_disturbance_count"],
        BandRole::Count,
        255,
        "count",
    );
    count.description = "Number of generic disturbances since first detection; values are non-negative integers and 255 is source fill";
    let mut percent = categorical(
        "GEN-DIST-PERC",
        &["percent", "percentage"],
        BandRole::Metric,
        255,
        "%",
    );
    percent.description =
        "Percentage of generic disturbance since first detection; 255 is source fill";
    ProductProfile {
        id: ID,
        family: ProductFamily::OperaDistS1,
        mission: "NASA OPERA Sentinel-1",
        instrument: "Sentinel-1 C-SAR RTC-S1 input time series",
        title: "OPERA Level-3 DIST-ALERT-S1 V1.0 accumulated generic disturbance observation",
        version: RELEASE,
        processing_versions: &[RELEASE],
        native_grid: NativeGrid {
            posting_m: 30,
            projection: "WGS84 UTM",
            organization: "MGRS tile; one 2-D COG per named layer",
        },
        catalog: CatalogIdentity {
            provider: "NASA ASF DAAC",
            catalog: "NASA CMR / ASF Cumulus",
            collection: COLLECTION,
            version: VERSION,
            access: "Earthdata-authenticated ASF Cumulus HTTPS COG assets; S3 direct access requires temporary credentials",
            platforms: &["SENTINEL_1A", "SENTINEL_1B", "SENTINEL_1C", "SENTINEL_1D"],
            processing_levels: &["L3", "DIST-ALERT-S1"],
        },
        bands: vec![
            status,
            floating(
                "GEN-METRIC",
                BandRole::Metric,
                "Generic disturbance metric: non-negative Float32 deviations from the mean; NaN is source fill and no confidence threshold is applied",
            ),
            status_acq,
            floating(
                "GEN-METRIC-MAX",
                BandRole::Metric,
                "Maximum non-negative generic disturbance metric since first disturbance, reset to zero for a new disturbance; NaN is source fill",
            ),
            floating(
                "GEN-DIST-CONF",
                BandRole::Confidence,
                "Non-negative generic disturbance confidence metric, reset to zero for a new disturbance; NaN is source fill and it is not a probability",
            ),
            days(
                "GEN-DIST-DATE",
                BandRole::Date,
                "days since 2020-12-31",
                "Generic disturbance date as days since 2020-12-31; -1 is source fill or no prior acquisition",
            ),
            count,
            percent,
            days(
                "GEN-DIST-DUR",
                BandRole::Duration,
                "days",
                "Generic disturbance duration in days since first detection; -1 is source fill",
            ),
            days(
                "GEN-DIST-LAST-DATE",
                BandRole::Date,
                "days since 2020-12-31",
                "Latest generic disturbance detection as days since 2020-12-31; -1 is source fill",
            ),
        ],
        quality_policies: vec![
            QualityPolicy::DistS1(Policy::Confirmed),
            QualityPolicy::DistS1(Policy::ConfirmedOngoing),
        ],
        capability_caveat: "This descriptor grants no acquisition beyond the authenticated ASF Cumulus v1.0 contract: all ten named physical COG layers are admitted only when their canonical locator, conditional object version, native dtype/fill/grid, and retained Sentinel-1 metadata agree. No implicit multi-observation compositing or confidence threshold is performed.",
    }
}

struct Granule<'a> {
    tile: &'a str,
    acquisition: chrono::NaiveDateTime,
    platform: &'a str,
}

fn granule(value: &str) -> Result<Granule<'_>> {
    let parts = value.split('_').collect::<Vec<_>>();
    if parts.len() != 9
        || parts[..3] != ["OPERA", "L3", "DIST-ALERT-S1"]
        || !valid_mgrs(parts[3])
        || chrono::NaiveDateTime::parse_from_str(parts[4], "%Y%m%dT%H%M%SZ").is_err()
        || chrono::NaiveDateTime::parse_from_str(parts[5], "%Y%m%dT%H%M%SZ").is_err()
        || !matches!(parts[6], "S1A" | "S1B" | "S1C" | "S1D")
        || parts[7] != "30"
        || parts[8] != "v1.0"
    {
        return fail("granule is not an exact OPERA DIST-ALERT-S1 v1.0 30 m identifier");
    }
    Ok(Granule {
        tile: parts[3],
        acquisition: chrono::NaiveDateTime::parse_from_str(parts[4], "%Y%m%dT%H%M%SZ")?,
        platform: parts[6],
    })
}

fn valid_mgrs(value: &str) -> bool {
    let bytes = value.as_bytes();
    let zone = bytes
        .get(1..3)
        .filter(|digits| digits.iter().all(u8::is_ascii_digit))
        .map(|digits| (digits[0] - b'0') * 10 + (digits[1] - b'0'));
    bytes.len() == 6
        && bytes[0] == b'T'
        && zone.is_some_and(|zone| (1..=60).contains(&zone))
        && matches!(bytes[3], b'C'..=b'H' | b'J'..=b'N' | b'P'..=b'X')
        && matches!(bytes[4], b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'Z')
        && matches!(bytes[5], b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'Z')
}

fn normal(value: &str) -> String {
    value.to_ascii_uppercase().replace('-', "_")
}

pub fn validate_identity(
    value: &str,
    platform: Option<&str>,
    processing: Option<&str>,
) -> Result<()> {
    let parsed = granule(value)?;
    if let Some(platform) = platform {
        let expected = format!("SENTINEL_1{}", &parsed.platform[2..]);
        let found = normal(platform);
        if found != expected && found != parsed.platform {
            return fail("DIST-S1 catalog platform conflicts with the granule sensor token");
        }
    }
    if let Some(processing) = processing {
        let processing = normal(processing);
        if !matches!(
            processing.as_str(),
            "L3" | "DIST_ALERT_S1" | "L3_DIST_ALERT_S1"
        ) {
            return fail("DIST-S1 catalog processing conflicts with the L3 DIST-ALERT-S1 profile");
        }
    }
    Ok(())
}

pub fn validate_datetime(value: &str, datetime: &str) -> Result<()> {
    let expected = granule(value)?.acquisition.and_utc();
    let observed = chrono::DateTime::parse_from_rfc3339(datetime)?.to_utc();
    if observed.timestamp() != expected.timestamp() {
        return fail("DIST-S1 observation datetime contradicts granule acquisition identity");
    }
    Ok(())
}

/// Exact canonical ASF HTTPS asset naming for this collection. CMR's S3 URL is
/// deliberately not accepted as an implicit requester-pays/temporary-credential route.
pub fn band_href(band: &str, href: &str) -> bool {
    let filename = href
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let Some(id) = filename.strip_suffix(&format!("_{band}.tif")) else {
        return false;
    };
    granule(id).is_ok()
}

pub fn validate_canonical_href(
    observation: &Observation,
    band: &BandProfile,
    asset: &ProductAsset,
) -> Result<()> {
    let url = reqwest::Url::parse(&asset.href)?;
    let expected_file = format!("{}_{}.tif", observation.id, band.name);
    let expected_path = format!("/OPERA/{COLLECTION}/{}/{}", observation.id, expected_file);
    if url.scheme() != "https"
        || url.host_str() != Some("cumulus.asf.earthdatacloud.nasa.gov")
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != expected_path
        || !band_href(&band.name, &asset.href)
    {
        return fail(
            "DIST-S1 canonical ASF HTTPS asset conflicts with exact granule/layer identity",
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
    if observation.collection != COLLECTION
        && observation.collection != format!("{COLLECTION}_{VERSION}")
    {
        return fail("DIST-S1 observation collection conflicts with exact ASF collection identity");
    }
    let profile = profile();
    for requested in &request.bands {
        let band = profile.band(requested)?;
        let asset = observation
            .assets
            .get(&band.name)
            .ok_or("DIST-S1 observation is missing a requested layer")?;
        validate_canonical_href(observation, band, asset)?;
    }
    Ok(())
}

fn xml_item<'a>(xml: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!(r#"<Item name="{key}""#);
    let tail = xml.get(xml.find(&marker)? + marker.len()..)?;
    let value = tail.get(tail.find('>')? + 1..)?;
    value.get(..value.find("</Item>")?).map(str::trim)
}

fn dtype(layout: &Layout) -> &'static str {
    match layout.dtype.as_str() {
        "Byte" | "UInt8" | "|u1" => "uint8",
        "Int16" | "<i2" => "int16",
        "Float32" | "<f4" => "float32",
        _ => "unsupported",
    }
}

pub fn validate_physical(
    observation: &Observation,
    band: &BandProfile,
    asset: &ProductAsset,
    source: &Value,
    layout: &Layout,
) -> Result<()> {
    validate_canonical_href(observation, band, asset)?;
    let physical = source
        .pointer("/object_identities/native_cog")
        .or_else(|| source.get("physical_metadata"))
        .ok_or("DIST-S1 source lacks retained native COG metadata")?;
    if dtype(layout) != band.dtype.name()
        || layout.width != 3660
        || layout.height != 3660
        || layout.block != [512, 512]
        || layout.transform[1] != 30.
        || layout.transform[5] != -30.
        || layout.transform[2] != 0.
        || layout.transform[4] != 0.
        || layout
            .crs
            .strip_prefix("EPSG:")
            .and_then(|value| value.parse::<u32>().ok())
            .is_none_or(|epsg| parsed_epsg(&observation.id) != Some(epsg))
    {
        return fail(
            "DIST-S1 physical COG conflicts with required 30 m north-up UTM layer contract",
        );
    }
    match band.nodata {
        Some(NoDataValue::Float32NaN) => {
            if layout.nodata.is_some()
                || !physical["gdal_nodata_text"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case("nan"))
            {
                return fail("DIST-S1 floating layer must retain a NaN source nodata token and no finite replacement");
            }
        }
        Some(expected) => {
            let expected = expected.as_f64();
            if layout.nodata != Some(expected)
                || physical["gdal_nodata_text"]
                    .as_str()
                    .and_then(|value| value.parse::<f64>().ok())
                    != Some(expected)
            {
                return fail(
                    "DIST-S1 physical source nodata conflicts with the named layer contract",
                );
            }
        }
        None => return fail("DIST-S1 layers require an explicit source nodata convention"),
    }
    let xml = physical["gdal_metadata_xml"]
        .as_str()
        .ok_or("DIST-S1 source lacks retained GDAL metadata")?;
    let parsed = granule(&observation.id)?;
    let source_etag = source["object_version"]
        .as_str()
        .ok_or("DIST-S1 source lacks its conditional object version")?;
    if !strong_etag(source_etag) {
        return fail("DIST-S1 inspected source object version is not a strong ETag");
    }
    if asset
        .etag
        .as_deref()
        .is_some_and(|expected| expected != source_etag)
    {
        return fail("DIST-S1 inspected source object version conflicts with the selected asset");
    }
    let tile = parsed.tile.strip_prefix('T').unwrap_or(parsed.tile);
    let acquisition = parsed.acquisition.format("%Y%m%dT%H%M%SZ").to_string();
    let post_rtc = xml_item(xml, "post_rtc_opera_ids");
    if xml_item(xml, "mgrs_tile_id") != Some(tile)
        || xml_item(xml, "sensor") != Some(parsed.platform)
        || !post_rtc.is_some_and(|ids| {
            ids.contains(&acquisition)
                && ids.contains(&format!("_{}_30_v{RELEASE}", parsed.platform))
        })
        || xml_item(xml, "version").is_none_or(str::is_empty)
    {
        return fail("DIST-S1 GDAL metadata conflicts with tile, Sentinel-1 acquisition, or producer provenance");
    }
    Ok(())
}

fn strong_etag(value: &str) -> bool {
    value.len() >= 3
        && value.starts_with('"')
        && value.ends_with('"')
        && !value.starts_with("W/")
        && value[1..value.len() - 1]
            .bytes()
            .all(|byte| (33..=126).contains(&byte) && byte != b'"')
}

fn parsed_epsg(id: &str) -> Option<u32> {
    let tile = granule(id).ok()?.tile.as_bytes();
    let zone = std::str::from_utf8(tile.get(1..3)?)
        .ok()?
        .parse::<u32>()
        .ok()?;
    let north = tile.get(3).is_some_and(|letter| *letter >= b'N');
    Some(if north { 32600 } else { 32700 } + zone)
}
