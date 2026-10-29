//! Narrow annual OPERA DIST-ANN-HLS product contract.
//!
//! DIST-ANN is an annual summary, not an instantaneous acquisition.  Its
//! catalog interval is therefore carried explicitly and is never reconstructed
//! from the summary year or the product filename.
use super::{
    profile::{
        self, BandProfile, BandRole, CatalogIdentity, DataType, NativeGrid, NoDataValue,
        ProductFamily, ProductProfile, QualityPolicy, QualitySample,
    },
    Observation, ProductAsset, ProductRequest,
};
use crate::{fail, Layout, Result};
use serde::{ser::SerializeStruct, Serialize};
use serde_json::Value;

pub const ID: &str = "opera-dist-ann-hls";
pub const COLLECTION: &str = "OPERA_L3_DIST-ANN-HLS_V1";
const VERSION: &str = "1";

const BANDS: [&str; 21] = [
    "VEG-DIST-STATUS",
    "GEN-DIST-STATUS",
    "VEG-DIST-DATE",
    "GEN-DIST-DATE",
    "VEG-LAST-DATE",
    "GEN-LAST-DATE",
    "VEG-ANOM-MAX",
    "GEN-ANOM-MAX",
    "VEG-DIST-CONF",
    "GEN-DIST-CONF",
    "VEG-DIST-COUNT",
    "GEN-DIST-COUNT",
    "VEG-DIST-DUR",
    "GEN-DIST-DUR",
    "VEG-CONF-PREV",
    "GEN-CONF-PREV",
    "VEG-CONF-COUNT",
    "GEN-CONF-COUNT",
    "VEG-HIST",
    "VEG-IND-MAX",
    "VEG-IND-3YR-MIN",
];

/// Explicit masks over annual raw status classes.  `Confirmed` includes
/// confirmed classes retained from the preceding year (9 and 10); callers that
/// need only that history, or only the current annual result, select the more
/// specific policies instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    VegConfirmed,
    GenConfirmed,
    VegCurrentYearConfirmed,
    GenCurrentYearConfirmed,
    VegPreviousYearConfirmed,
    GenPreviousYearConfirmed,
}

impl Policy {
    pub const fn name(self) -> &'static str {
        match self {
            Self::VegConfirmed => "dist_ann_veg_confirmed",
            Self::GenConfirmed => "dist_ann_gen_confirmed",
            Self::VegCurrentYearConfirmed => "dist_ann_veg_current_year_confirmed",
            Self::GenCurrentYearConfirmed => "dist_ann_gen_current_year_confirmed",
            Self::VegPreviousYearConfirmed => "dist_ann_veg_previous_year_confirmed",
            Self::GenPreviousYearConfirmed => "dist_ann_gen_previous_year_confirmed",
        }
    }

    pub const fn required_bands(self) -> &'static [&'static str] {
        match self {
            Self::VegConfirmed | Self::VegCurrentYearConfirmed | Self::VegPreviousYearConfirmed => {
                &["VEG-DIST-STATUS"]
            }
            Self::GenConfirmed | Self::GenCurrentYearConfirmed | Self::GenPreviousYearConfirmed => {
                &["GEN-DIST-STATUS"]
            }
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::VegConfirmed => "annual vegetation status confirmed in the summary year (3,6,7,8) or retained confirmed previous-year state (9,10); raw status is retained",
            Self::GenConfirmed => "annual generic status confirmed in the summary year (3,6,7,8) or retained confirmed previous-year state (9,10); raw status is retained",
            Self::VegCurrentYearConfirmed => "annual vegetation status confirmed in the summary year (3,6,7,8); previous-year classes 9 and 10 are excluded",
            Self::GenCurrentYearConfirmed => "annual generic status confirmed in the summary year (3,6,7,8); previous-year classes 9 and 10 are excluded",
            Self::VegPreviousYearConfirmed => "annual vegetation status retained as confirmed from the previous year (9,10) only",
            Self::GenPreviousYearConfirmed => "annual generic status retained as confirmed from the previous year (9,10) only",
        }
    }

    pub fn accepts(self, sample: QualitySample) -> Result<bool> {
        let QualitySample::UInt8(value) = sample else {
            return fail("DIST-ANN status policy requires a UInt8 raw status sample");
        };
        if !matches!(value, 0 | 3 | 6 | 7 | 8 | 9 | 10 | 255) {
            return fail("undefined DIST-ANN status class");
        }
        Ok(match self {
            Self::VegConfirmed | Self::GenConfirmed => matches!(value, 3 | 6 | 7 | 8 | 9 | 10),
            Self::VegCurrentYearConfirmed | Self::GenCurrentYearConfirmed => {
                matches!(value, 3 | 6 | 7 | 8)
            }
            Self::VegPreviousYearConfirmed | Self::GenPreviousYearConfirmed => {
                matches!(value, 9 | 10)
            }
        })
    }
}

/// A catalog-asserted coverage interval and the distinct annual summary year.
/// The timestamps are parsed values rather than a date inferred from the year.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemporalExtent {
    pub coverage_start: chrono::DateTime<chrono::FixedOffset>,
    pub coverage_end: chrono::DateTime<chrono::FixedOffset>,
    pub summary_year: i32,
}

impl Serialize for TemporalExtent {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("TemporalExtent", 3)?;
        state.serialize_field(
            "coverage_start",
            &self
                .coverage_start
                .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        )?;
        state.serialize_field(
            "coverage_end",
            &self
                .coverage_end
                .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        )?;
        state.serialize_field("summary_year", &self.summary_year)?;
        state.end()
    }
}

/// Parse the retained annual interval without manufacturing an instantaneous
/// timestamp.  `Observation.datetime` has the explicit role `coverage_start`.
pub fn temporal_extent(observation: &Observation) -> Result<TemporalExtent> {
    let value = observation
        .properties
        .get("temporal_extent")
        .and_then(Value::as_object)
        .ok_or("annual DIST observation requires properties.temporal_extent object")?;
    if value.len() != 3
        || !value.contains_key("coverage_start")
        || !value.contains_key("coverage_end")
        || !value.contains_key("summary_year")
    {
        return fail("annual DIST temporal_extent must contain only coverage_start, coverage_end, and summary_year");
    }
    let coverage_start = chrono::DateTime::parse_from_rfc3339(
        value["coverage_start"]
            .as_str()
            .ok_or("annual DIST coverage_start must be an RFC3339 string")?,
    )?;
    let coverage_end = chrono::DateTime::parse_from_rfc3339(
        value["coverage_end"]
            .as_str()
            .ok_or("annual DIST coverage_end must be an RFC3339 string")?,
    )?;
    let summary_year = value["summary_year"]
        .as_i64()
        .and_then(|year| i32::try_from(year).ok())
        .ok_or("annual DIST summary_year must be an integer")?;
    if coverage_end < coverage_start {
        return fail("annual DIST coverage_end precedes coverage_start");
    }
    let datetime = chrono::DateTime::parse_from_rfc3339(&observation.datetime)?;
    if datetime != coverage_start {
        return fail("annual DIST observation datetime must equal temporal_extent.coverage_start");
    }
    Ok(TemporalExtent {
        coverage_start,
        coverage_end,
        summary_year,
    })
}

pub fn profile() -> ProductProfile {
    let status = |name, aliases| {
        let mut band = profile::band(
            name,
            aliases,
            BandRole::Classification,
            DataType::UInt8,
            Some(NoDataValue::UInt8(255)),
            1.,
            0.,
            "1",
        );
        band.description = "Annual DIST accumulated status: 0 no disturbance; 3/6 confirmed low/high; 7/8 confirmed finished; 9/10 confirmed previous-year low/high; 255 missing";
        band
    };
    let date = |name, aliases, description| {
        let mut band = profile::band(
            name,
            aliases,
            BandRole::Date,
            DataType::Int16,
            Some(NoDataValue::Int16(-1)),
            1.,
            0.,
            "days since 2020-12-31",
        );
        band.description = description;
        band
    };
    let native = |name, role, dtype, nodata, unit, description| {
        let mut band = profile::band(name, &[], role, dtype, nodata, 1., 0., unit);
        band.description = description;
        band
    };
    ProductProfile {
        id: ID,
        family: ProductFamily::OperaDistAnn,
        mission: "NASA OPERA",
        instrument: "Harmonized Landsat and Sentinel-2",
        title: "OPERA Level-3 DIST-ANN-HLS V1 annual disturbance summary",
        version: VERSION,
        processing_versions: &[VERSION],
        native_grid: NativeGrid {
            posting_m: 30,
            projection: "WGS84 UTM",
            organization: "MGRS tile; one 2-D COG per named annual layer",
        },
        catalog: CatalogIdentity {
            provider: "NASA LP DAAC",
            catalog: "NASA CMR / LP DAAC",
            collection: COLLECTION,
            version: VERSION,
            access: "Earthdata-authenticated LP DAAC HTTPS COG assets",
            platforms: &[],
            processing_levels: &["L3", "DIST-ANN-HLS"],
        },
        // All producer layers have explicit native type/nodata contracts; raw
        // source and durable-output qualification remains separately recorded.
        bands: vec![
            status("VEG-DIST-STATUS", &["vegetation_disturbance_status"]),
            status("GEN-DIST-STATUS", &["generic_disturbance_status"]),
            date("VEG-DIST-DATE", &["vegetation_disturbance_date"], "Date of initial annual vegetation disturbance as days since 2020-12-31; -1 is source fill"),
            date("GEN-DIST-DATE", &["generic_disturbance_date"], "Date of initial annual generic disturbance as days since 2020-12-31; -1 is source fill"),
            date("VEG-LAST-DATE", &["vegetation_last_date"], "Last observation assessed for annual vegetation disturbance as days since 2020-12-31; -1 is source fill"),
            date("GEN-LAST-DATE", &["generic_last_date"], "Last observation assessed for annual generic disturbance as days since 2020-12-31; -1 is source fill"),
            native("VEG-ANOM-MAX", BandRole::Metric, DataType::UInt8, Some(NoDataValue::UInt8(255)), "%", "Maximum annual vegetation anomaly percentage; 255 is source fill"),
            native("GEN-ANOM-MAX", BandRole::Metric, DataType::Int16, Some(NoDataValue::Int16(-1)), "1", "Maximum annual generic anomaly metric; -1 is source fill"),
            native("VEG-DIST-CONF", BandRole::Confidence, DataType::Int16, Some(NoDataValue::Int16(-1)), "1", "Vegetation disturbance confidence metric; -1 is source fill and values are not probabilities"),
            native("GEN-DIST-CONF", BandRole::Confidence, DataType::Int16, Some(NoDataValue::Int16(-1)), "1", "Generic disturbance confidence metric; -1 is source fill and values are not probabilities"),
            native("VEG-DIST-COUNT", BandRole::Count, DataType::UInt8, Some(NoDataValue::UInt8(255)), "count", "Number of vegetation disturbance anomalies; 255 is source fill"),
            native("GEN-DIST-COUNT", BandRole::Count, DataType::UInt8, Some(NoDataValue::UInt8(255)), "count", "Number of generic disturbance observations; 255 is source fill"),
            native("VEG-DIST-DUR", BandRole::Duration, DataType::Int16, Some(NoDataValue::Int16(-1)), "days", "Vegetation disturbance duration in days; -1 is source fill"),
            native("GEN-DIST-DUR", BandRole::Duration, DataType::Int16, Some(NoDataValue::Int16(-1)), "days", "Generic disturbance duration in days; -1 is source fill"),
            native("VEG-CONF-PREV", BandRole::Classification, DataType::UInt8, Some(NoDataValue::UInt8(255)), "1", "Confirmed vegetation disturbance class retained from the previous year; 255 is source fill"),
            native("GEN-CONF-PREV", BandRole::Classification, DataType::UInt8, Some(NoDataValue::UInt8(255)), "1", "Confirmed generic disturbance class retained from the previous year; 255 is source fill"),
            native("VEG-CONF-COUNT", BandRole::Count, DataType::UInt8, Some(NoDataValue::UInt8(255)), "count", "Count of vegetation disturbance alerts; 255 is source fill"),
            native("GEN-CONF-COUNT", BandRole::Count, DataType::UInt8, Some(NoDataValue::UInt8(255)), "count", "Count of generic disturbance alerts; 255 is source fill"),
            native("VEG-HIST", BandRole::Metric, DataType::UInt8, Some(NoDataValue::UInt8(255)), "%", "Historical vegetation cover percentage; 255 is source fill"),
            native("VEG-IND-MAX", BandRole::Metric, DataType::UInt8, Some(NoDataValue::UInt8(255)), "%", "Maximum annual vegetation index percentage; 255 is source fill"),
            native("VEG-IND-3YR-MIN", BandRole::Metric, DataType::UInt8, Some(NoDataValue::UInt8(255)), "%", "Minimum three-year vegetation index percentage; 255 is source fill"),
        ],
        quality_policies: vec![
            QualityPolicy::DistAnn(Policy::VegConfirmed),
            QualityPolicy::DistAnn(Policy::GenConfirmed),
            QualityPolicy::DistAnn(Policy::VegCurrentYearConfirmed),
            QualityPolicy::DistAnn(Policy::GenCurrentYearConfirmed),
            QualityPolicy::DistAnn(Policy::VegPreviousYearConfirmed),
            QualityPolicy::DistAnn(Policy::GenPreviousYearConfirmed),
        ],
        capability_caveat: "This profile grants no acquisition without an independently inspected, Earthdata-authorized source. Qualification currently covers conventional northern-hemisphere WGS84 UTM specimens; provider extended-northern-zone southern-tile conventions are not admitted. A fabricated instantaneous acquisition time, implicit annual compositing, and confidence thresholds are not supported.",
    }
}

struct Granule {
    summary_year: i32,
}

fn granule(value: &str) -> Result<Granule> {
    let value = value.trim_end_matches(".tif");
    let parts = value.split('_').collect::<Vec<_>>();
    if parts.len() != 8
        || parts[..3] != ["OPERA", "L3", "DIST-ANN-HLS"]
        || !valid_mgrs(parts[3])
        || chrono::NaiveDateTime::parse_from_str(parts[5], "%Y%m%dT%H%M%SZ").is_err()
        || parts[6] != "30"
        || parts[7] != "v1"
    {
        return fail("granule is not an exact OPERA DIST-ANN-HLS v1 30 m identifier");
    }
    let summary_year = parts[4]
        .parse::<i32>()
        .ok()
        .filter(|year| (2023..=9999).contains(year))
        .ok_or("annual DIST granule must carry a supported four-digit summary year")?;
    Ok(Granule { summary_year })
}

/// Return the filename's annual summary year without assigning it a date or
/// treating it as an acquisition time.
pub fn summary_year(granule_id: &str) -> Result<i32> {
    Ok(granule(granule_id)?.summary_year)
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
    let filename = value
        .split(['?', '#'])
        .next()
        .unwrap_or(value)
        .rsplit('/')
        .next()
        .unwrap_or(value);
    let bare = filename.strip_suffix(".tif").unwrap_or(filename);
    let base = BANDS
        .iter()
        .find_map(|band| bare.strip_suffix(&format!("_{band}")))
        .unwrap_or(bare);
    granule(base)?;
    if platform.is_some() {
        return fail("annual DIST catalog platform is not an acquisition-sensor identity");
    }
    if let Some(processing) = processing {
        if !matches!(
            normal(processing).as_str(),
            "L3" | "DIST_ANN_HLS" | "L3_DIST_ANN_HLS"
        ) {
            return fail("annual DIST processing conflicts with the L3 DIST-ANN-HLS profile");
        }
    }
    if base != bare && !BANDS.iter().any(|band| bare.ends_with(&format!("_{band}"))) {
        return fail("annual DIST asset layer is not in the admitted layer set");
    }
    Ok(())
}

/// There is no granule acquisition timestamp to bind.  Validate syntax here;
/// `validate_observation` binds it to the explicit coverage interval instead.
pub fn validate_datetime(value: &str, datetime: &str) -> Result<()> {
    validate_identity(value, None, None)?;
    chrono::DateTime::parse_from_rfc3339(datetime)?;
    Ok(())
}

/// Verify an exact LP DAAC native-C0G href for one admitted annual layer.
pub fn band_href(band: &str, href: &str) -> bool {
    if !BANDS.contains(&band) {
        return false;
    }
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

fn validate_canonical_href(
    observation: &Observation,
    band: &BandProfile,
    asset: &ProductAsset,
) -> Result<()> {
    let url = reqwest::Url::parse(&asset.href)?;
    let expected_file = format!("{}_{}.tif", observation.id, band.name);
    let expected_path = format!(
        "/lp-prod-protected/{COLLECTION}/{}/{}",
        observation.id, expected_file
    );
    if url.scheme() != "https"
        || url.host_str() != Some("data.lpdaac.earthdatacloud.nasa.gov")
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != expected_path
        || !band_href(&band.name, &asset.href)
    {
        return fail(
            "annual DIST canonical LP DAAC asset conflicts with exact granule/layer identity",
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
    let extent = temporal_extent(observation)?;
    if granule(&observation.id)?.summary_year != extent.summary_year {
        return fail("annual DIST temporal_extent summary_year conflicts with granule identity");
    }
    let requested_start = chrono::DateTime::parse_from_rfc3339(&request.start)?;
    let requested_end = chrono::DateTime::parse_from_rfc3339(&request.end)?;
    if extent.coverage_end < requested_start || extent.coverage_start > requested_end {
        return fail("annual DIST coverage interval does not overlap the requested interval");
    }
    if observation.collection != COLLECTION
        && observation.collection != format!("{COLLECTION}_{VERSION}")
    {
        return fail(
            "annual DIST observation collection conflicts with exact LP DAAC collection identity",
        );
    }
    let profile = profile();
    for requested in &request.bands {
        let band = profile.band(requested)?;
        let asset = observation
            .assets
            .get(&band.name)
            .ok_or("annual DIST observation is missing a requested admitted layer")?;
        validate_canonical_href(observation, band, asset)?;
    }
    Ok(())
}

fn xml_item<'a>(xml: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!(r#"<Item name="{key}""#);
    let tail = xml.get(xml.find(&marker)? + marker.len()..)?;
    let value = tail.get(tail.find('>')? + 1..)?;
    value.get(..value.find("</Item>")?)
}

fn strong_etag(value: &str) -> bool {
    value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && !value[1..value.len() - 1]
            .bytes()
            .any(|byte| byte < 33 || byte == 127 || byte == b'"')
}

fn dtype(layout: &Layout) -> &'static str {
    match layout.dtype.as_str() {
        "Byte" | "UInt8" | "|u1" => "uint8",
        "Int16" | "<i2" => "int16",
        _ => "unsupported",
    }
}

/// Bind one inspected COG to its requested annual layer, interval, and grid.
/// The whole prepared-source value is required: its native COG evidence lives
/// under `object_identities.native_cog` and must not be replaced by request
/// metadata.
pub fn validate_physical(
    observation: &Observation,
    band: &BandProfile,
    asset: &ProductAsset,
    source: &Value,
    layout: &Layout,
) -> Result<()> {
    validate_canonical_href(observation, band, asset)?;
    let extent = temporal_extent(observation)?;
    if dtype(layout) != band.dtype.name()
        || layout.nodata != band.nodata.map(NoDataValue::as_f64)
        || layout.width != 3660
        || layout.height != 3660
        || layout.block
            != if band.name.ends_with("LAST-DATE") {
                [512, 512]
            } else {
                [256, 256]
            }
        || layout.transform[1] != 30.
        || layout.transform[5] != -30.
        || layout.transform[2] != 0.
        || layout.transform[4] != 0.
        || !layout
            .crs
            .strip_prefix("EPSG:")
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|epsg| (32601..=32660).contains(&epsg) || (32701..=32760).contains(&epsg))
    {
        return fail(
            "annual DIST physical COG conflicts with the admitted 30 m MGRS layer contract",
        );
    }
    let native = source
        .pointer("/object_identities/native_cog")
        .or_else(|| source.get("physical_metadata"))
        .ok_or("annual DIST source lacks retained native COG evidence")?;
    if !source["object_version"].as_str().is_some_and(strong_etag) {
        return fail("annual DIST source lacks an inspected strong object ETag");
    }
    let nodata = native["gdal_nodata_text"]
        .as_str()
        .and_then(|value| value.parse::<f64>().ok());
    if nodata != band.nodata.map(NoDataValue::as_f64) {
        return fail("annual DIST retained native COG nodata conflicts with named layer contract");
    }
    let xml = native["gdal_metadata_xml"]
        .as_str()
        .ok_or("annual DIST source lacks retained GDAL metadata")?;
    let flags = "0,3,6,7,8,9,10,255";
    let maximum_day = extent
        .coverage_end
        .to_utc()
        .date_naive()
        .signed_duration_since(
            chrono::NaiveDate::from_ymd_opt(2020, 12, 31).expect("fixed DIST epoch is valid"),
        )
        .num_days()
        .to_string();
    if band.name.ends_with("STATUS") {
        if xml_item(xml, "flag_values") != Some(flags)
            || !xml_item(xml, "flag_meanings")
                .is_some_and(|value| value.contains("previous_year") && value.contains("no_data"))
        {
            return fail("annual DIST status source flags conflict with the named layer contract");
        }
    } else if band.name.ends_with("DATE")
        && (xml_item(xml, "Units") != Some("days")
            || xml_item(xml, "Valid_min") != Some("0")
            || xml_item(xml, "Valid_max") != Some(maximum_day.as_str()))
    {
        return fail("annual DIST date source metadata conflicts with the named layer contract");
    } else if let Some(value) = xml_item(xml, "Percent_data") {
        if !value
            .parse::<f64>()
            .is_ok_and(|percent| percent.is_finite() && (0.0..=100.0).contains(&percent))
        {
            return fail("annual DIST source has an invalid native Percent_data metadata value");
        }
    }
    if band.name.ends_with("LAST-DATE") {
        let update = xml_item(xml, "Update_Date")
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .ok_or("annual DIST last-date source lacks an update timestamp")?;
        if (update - extent.coverage_end).num_milliseconds().abs() > 1 {
            return fail(
                "annual DIST last-date source update time conflicts with catalog coverage end",
            );
        }
    }
    Ok(())
}
