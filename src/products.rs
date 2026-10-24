//! Scientific product selection over the existing bounded acquisition lifecycle.
//! Profiles describe expectations; the physical reader still inspects every source.
pub mod profile;
use crate::{
    acquisition, cog::LayoutCache, fail, Asset, Budget, Format, Grid, Request, Result, Sink,
    Source, Spatial, Time, Variable,
};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    time::Duration,
};

const MAX_JSON: u64 = 4 * 1024 * 1024;
const PC: &str = "https://planetarycomputer.microsoft.com/api/stac/v1";
const CMR: &str = "https://cmr.earthdata.nasa.gov/search";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductRequest {
    pub product: String,
    pub aoi: Spatial,
    pub start: String,
    pub end: String,
    pub bands: Vec<String>,
    #[serde(default)]
    pub output: Option<String>,
    /// Explicit selected/frozen observations. Absence invokes bounded discovery.
    #[serde(default)]
    pub observations: Option<Vec<Observation>>,
    #[serde(default = "default_limit")]
    pub max_observations: usize,
    #[serde(default = "default_buffer")]
    pub buffer_bytes: usize,
    #[serde(default = "default_edge")]
    pub chunk_edge: usize,
    #[serde(default)]
    pub quality_policies: Vec<String>,
}
fn default_limit() -> usize {
    8
}
fn default_buffer() -> usize {
    32 * 1024 * 1024
}
fn default_edge() -> usize {
    256
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub id: String,
    pub datetime: String,
    pub collection: String,
    pub platform: Option<String>,
    pub processing: Option<String>,
    pub assets: BTreeMap<String, ProductAsset>,
    #[serde(default)]
    pub properties: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductAsset {
    pub href: String,
    pub etag: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}
#[derive(Serialize)]
pub struct Catalog {
    pub schema: &'static str,
    pub product: String,
    pub provider: &'static str,
    pub complete: bool,
    pub observations: Vec<Observation>,
    /// Retained product metadata charged outside the core reader working budget.
    pub metadata_reservation_bytes: usize,
    pub seconds: f64,
}
fn client() -> Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(10))
        .redirect(Policy::none())
        .build()?)
}
fn read_json(response: reqwest::blocking::Response) -> Result<Value> {
    if !response.status().is_success() {
        return fail(&format!(
            "product catalog/signing HTTP status {}",
            response.status().as_u16()
        ));
    }
    let mut b = Vec::new();
    response.take(MAX_JSON + 1).read_to_end(&mut b)?;
    if b.len() as u64 > MAX_JSON {
        return fail("product response exceeds 4 MiB limit");
    }
    Ok(serde_json::from_slice(&b)?)
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| format!("catalog missing {key}").into())
}
impl ProductRequest {
    pub fn validate(&self) -> Result<()> {
        let p = profile::profile(&self.product)?;
        let start = chrono::DateTime::parse_from_rfc3339(&self.start)?;
        let end = chrono::DateTime::parse_from_rfc3339(&self.end)?;
        if end < start || (end - start).num_seconds() > 31 * 86400 {
            return fail("product date interval must be ordered and at most 31 days");
        }
        if !(1..=32).contains(&self.max_observations)
            || !(1..=16).contains(&self.bands.len())
            || self.buffer_bytes < 4 * 1024 * 1024
            || self.buffer_bytes > 1024 * 1024 * 1024
            || self.chunk_edge == 0
            || self.chunk_edge > 1024
        {
            return fail("invalid product limits: 1..32 observations, 1..16 bands, 4..1024 MiB buffer, chunk edge 1..1024");
        }
        let mut names = BTreeSet::new();
        for name in &self.bands {
            if !names.insert(p.band(name)?.name.clone()) {
                return fail("duplicate product band/alias");
            }
        }
        let bounds = geographic_bounds(&self.aoi)?;
        if bounds[2] - bounds[0] > 30.
            || bounds[3] - bounds[1] > 30.
            || (bounds[2] - bounds[0]) * (bounds[3] - bounds[1]) > 400.
        {
            return fail("product AOI exceeds bounded 30 degree / 400 square degree limit");
        }
        if let Some(observations) = &self.observations {
            if observations.len() > self.max_observations {
                return fail("explicit observations exceed max_observations");
            }
        }
        let mut policies = BTreeSet::new();
        for name in &self.quality_policies {
            let policy = p.quality_policy(name)?;
            if !policies.insert(policy.name()) {
                return fail("duplicate quality policy");
            }
            let required = policy.required_bands();
            if !self
                .bands
                .iter()
                .any(|b| p.band(b).is_ok_and(|b| required.contains(&b.name.as_str())))
            {
                return fail("quality policy requires its exact raw quality/classification band to be selected");
            }
        }
        Ok(())
    }
}
fn geographic_bounds(aoi: &Spatial) -> Result<[f64; 4]> {
    match aoi {
        Spatial::Bounds { crs, bbox }
            if crs == "EPSG:4326"
                && bbox.iter().all(|x| x.is_finite())
                && bbox[0] >= -180.
                && bbox[2] <= 180.
                && bbox[1] >= -90.
                && bbox[3] <= 90.
                && bbox[0] < bbox[2]
                && bbox[1] < bbox[3] =>
        {
            Ok(*bbox)
        }
        Spatial::Polygon { crs, .. } if crs == "EPSG:4326" => {
            let geometry = crate::spatial::geometry(aoi)?;
            Ok(crate::spatial::bounds(&geometry))
        }
        _ => fail(
            "product selection requires finite WGS84 bounds or Polygon; no antimeridian wrapping",
        ),
    }
}
/// Bounded, credential-free catalog discovery. A cap is an error, never truncation.
pub fn discover(request: &ProductRequest) -> Result<Catalog> {
    request.validate()?;
    // Reject attacker-sized explicit metadata before cloning the request or
    // any observation. Remote observations are bounded after parsing.
    if let Some(observations) = &request.observations {
        metadata_reservation(request, observations)?;
    }
    let canonical_id = profile::profile(&request.product)?.id;
    let canonical = if request.product == canonical_id {
        None
    } else {
        let mut value = request.clone();
        value.product = canonical_id.into();
        Some(value)
    };
    let request = canonical.as_ref().unwrap_or(request);
    let started = std::time::Instant::now();
    let observations = if let Some(obs) = &request.observations {
        obs.clone()
    } else if request.product == "landsat-c2-l2-sr" {
        discover_landsat(request)?
    } else {
        discover_cmr(request)?
    };
    for obs in &observations {
        validate_observation(request, obs)?;
    }
    let metadata_reservation_bytes = metadata_reservation(request, &observations)?;
    Ok(Catalog {
        schema: "tile-ripper-products-v1",
        product: request.product.clone(),
        provider: if request.product == "landsat-c2-l2-sr" {
            "planetary_computer"
        } else {
            "cmr"
        },
        complete: true,
        observations,
        metadata_reservation_bytes,
        seconds: started.elapsed().as_secs_f64(),
    })
}
fn discover_landsat(r: &ProductRequest) -> Result<Vec<Observation>> {
    let mut url = Url::parse(&format!("{PC}/search"))?;
    url.query_pairs_mut()
        .append_pair("collections", "landsat-c2-l2")
        .append_pair(
            "bbox",
            &geographic_bounds(&r.aoi)?
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .append_pair("datetime", &format!("{}/{}", r.start, r.end))
        .append_pair("query", r#"{"platform":{"in":["landsat-8","landsat-9"]}}"#)
        .append_pair("limit", &(r.max_observations + 1).to_string());
    let v = read_json(
        client()?
            .get(url)
            .send()
            .map_err(|_| "STAC product discovery transport failed")?,
    )?;
    let items = v["features"].as_array().ok_or("STAC missing features")?;
    if items.len() > r.max_observations
        || v["links"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v["rel"] == "next"))
    {
        return fail("product discovery exceeds max_observations or one bounded page; narrow AOI/date interval");
    }
    let p = profile::profile(&r.product)?;
    let mut out = vec![];
    for item in items {
        // This collection also contains Landsat 4–7, which this profile excludes.
        if !["landsat-8", "landsat-9"]
            .contains(&item["properties"]["platform"].as_str().unwrap_or(""))
        {
            continue;
        }
        let mut assets = BTreeMap::new();
        for requested in &r.bands {
            let band = p.band(requested)?;
            let asset = &item["assets"][&band.asset_key];
            assets.insert(
                band.name.to_string(),
                ProductAsset {
                    href: text(asset, "href")?.into(),
                    etag: None,
                    metadata: asset.clone(),
                },
            );
        }
        out.push(Observation {
            id: text(item, "id")?.into(),
            datetime: text(&item["properties"], "datetime")?.into(),
            collection: text(item, "collection")?.into(),
            platform: item["properties"]["platform"].as_str().map(str::to_owned),
            processing: item["properties"]["landsat:correction"]
                .as_str()
                .map(str::to_owned),
            assets,
            properties: item["properties"].clone(),
        });
    }
    Ok(out)
}
fn collection(product: &str) -> Result<(&'static str, &'static str)> {
    match product {
        "hls-l30" => Ok(("HLSL30", "2.0")),
        "hls-s30" => Ok(("HLSS30", "2.0")),
        "opera-dswx-hls" => Ok(("OPERA_L3_DSWX-HLS_V1", "1.0")),
        "opera-rtc-s1" => Ok(("OPERA_L2_RTC-S1_V1", "1")),
        _ => fail("no CMR route for product"),
    }
}
fn discover_cmr(r: &ProductRequest) -> Result<Vec<Observation>> {
    let (short, version) = collection(&r.product)?;
    let c = client()?;
    let mut url = Url::parse(&format!("{CMR}/collections.json"))?;
    url.query_pairs_mut()
        .append_pair("short_name", short)
        .append_pair("version", version)
        .append_pair("page_size", "2");
    let v = read_json(
        c.get(url)
            .send()
            .map_err(|_| "CMR collection transport failed")?,
    )?;
    let entries = v["feed"]["entry"]
        .as_array()
        .ok_or("CMR missing collections")?;
    if entries.len() != 1 {
        return fail(
            "CMR collection is missing or ambiguous; profile requires exact short-name/version",
        );
    }
    let concept = text(&entries[0], "id")?;
    let mut url = Url::parse(&format!("{CMR}/granules.json"))?;
    url.query_pairs_mut()
        .append_pair("collection_concept_id", concept)
        .append_pair(
            "bounding_box",
            &geographic_bounds(&r.aoi)?
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .append_pair("temporal", &format!("{},{}", r.start, r.end))
        .append_pair("page_size", &(r.max_observations + 1).to_string())
        .append_pair("sort_key", "start_date");
    let response = c
        .get(url)
        .send()
        .map_err(|_| "CMR granule transport failed")?;
    let hits = response
        .headers()
        .get("CMR-Hits")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .ok_or("CMR missing hit count; cannot establish complete search")?;
    if hits > r.max_observations {
        return fail("CMR query exceeds max_observations; narrow AOI/date interval");
    }
    let v = read_json(response)?;
    let entries = v["feed"]["entry"]
        .as_array()
        .ok_or("CMR missing granules")?;
    if entries.len() != hits {
        return fail("CMR search incomplete");
    }
    let p = profile::profile(&r.product)?;
    let mut out = vec![];
    for e in entries {
        let id = e["producer_granule_id"]
            .as_str()
            .or_else(|| e["title"].as_str())
            .ok_or("CMR missing granule identity")?;
        let links = e["links"].as_array().ok_or("CMR missing asset links")?;
        let mut assets = BTreeMap::new();
        for requested in &r.bands {
            let band = p.band(requested)?;
            let matches: Vec<_> = links
                .iter()
                .filter(|v| {
                    v["inherited"] != true
                        && v["rel"].as_str().is_some_and(|s| s.ends_with("/data#"))
                })
                .filter_map(|v| v["href"].as_str())
                .filter(|href| {
                    href.starts_with("https://") && band_href(&r.product, &band.name, href)
                })
                .collect();
            if matches.len() != 1 {
                return fail(&format!(
                    "CMR granule {id}: expected one HTTPS asset for {}, found {}",
                    band.name,
                    matches.len()
                ));
            }
            assets.insert(
                band.name.to_string(),
                ProductAsset {
                    href: matches[0].into(),
                    etag: None,
                    metadata: json!({"cmr_concept_id":e["id"],"revision_id":e["revision_id"]}),
                },
            );
        }
        out.push(Observation{id:id.into(),datetime:text(e,"time_start")?.into(),collection:short.into(),platform:None,processing:None,assets,properties:json!({"cmr_concept_id":e["id"],"collection_concept_id":concept,"updated":e["updated"]})});
    }
    Ok(out)
}
fn band_href(product: &str, band: &str, href: &str) -> bool {
    let path = href.split('?').next().unwrap_or("").to_ascii_lowercase();
    let band = band.to_ascii_lowercase();
    if product.starts_with("hls-") {
        path.ends_with(&format!(".{band}.tif"))
    } else {
        path.ends_with(&format!("_{band}.tif"))
    }
}
fn validate_observation(r: &ProductRequest, o: &Observation) -> Result<()> {
    let p = profile::profile(&r.product)?;
    p.validate_identity(&o.id, o.platform.as_deref(), o.processing.as_deref())?;
    p.validate_datetime(&o.id, &o.datetime)?;
    let collection_ok = if r.product == "landsat-c2-l2-sr" {
        o.collection == "landsat-c2-l2"
    } else {
        let (short, version) = collection(&r.product)?;
        o.collection == short
            || o.collection == format!("{short}_{version}")
            || (r.product.starts_with("hls-") && o.collection == format!("{short}.020"))
    };
    if !collection_ok {
        return fail("observation collection conflicts with product identity");
    }
    let time = chrono::DateTime::parse_from_rfc3339(&o.datetime)?;
    if time < chrono::DateTime::parse_from_rfc3339(&r.start)?
        || time > chrono::DateTime::parse_from_rfc3339(&r.end)?
    {
        return fail("observation outside requested inclusive interval");
    }
    let mut names = BTreeSet::new();
    for name in &r.bands {
        let b = p.band(name)?;
        let a = o
            .assets
            .get(&b.name)
            .ok_or("selected observation missing requested canonical band asset")?;
        if !names.insert(&b.name) || a.href.len() > 8192 || a.href.is_empty() {
            return fail("invalid product asset");
        }
        if a.href.starts_with("s3:") {
            return fail("S3 requester-pays/credential behavior is not implicit; choose an approved HTTPS or local mirror route");
        }
    }
    Ok(())
}

fn json_bytes(value: &Value, limit: usize) -> Result<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self
                .bytes
                .checked_add(buf.len())
                .is_none_or(|bytes| bytes > self.limit)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "JSON field exceeds product metadata limit",
                ));
            }
            self.bytes += buf.len();
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value)
        .map_err(|_| "JSON field exceeds product metadata limit")?;
    Ok(counter.bytes)
}

fn metadata_reservation(r: &ProductRequest, observations: &[Observation]) -> Result<usize> {
    const FIELD_LIMIT: usize = 16 * 1024;
    const ABSOLUTE_LIMIT: usize = 4 * 1024 * 1024;
    let p = profile::profile(&r.product)?;
    let mut total = 0usize;
    for observation in observations {
        let properties = json_bytes(&observation.properties, FIELD_LIMIT)
            .map_err(|_| "observation properties exceed 16 KiB product metadata limit")?;
        // One retained catalog copy, one conservative request/canonical copy,
        // and one acquisition-metadata copy per selected band.
        total = total
            .checked_add(
                properties
                    .checked_mul(r.bands.len() + 2)
                    .ok_or("product metadata reservation overflow")?,
            )
            .ok_or("product metadata reservation overflow")?;
        for asset in observation.assets.values() {
            let bytes = json_bytes(&asset.metadata, FIELD_LIMIT)
                .map_err(|_| "asset metadata exceed 16 KiB product metadata limit")?;
            total = total
                .checked_add(
                    bytes
                        .checked_mul(2)
                        .ok_or("product metadata reservation overflow")?,
                )
                .ok_or("product metadata reservation overflow")?;
        }
        for requested in &r.bands {
            let band = p.band(requested)?;
            let asset = observation
                .assets
                .get(&band.name)
                .ok_or("selected observation missing requested canonical band asset")?;
            total = total
                .checked_add(json_bytes(&asset.metadata, FIELD_LIMIT)?)
                .ok_or("product metadata reservation overflow")?;
        }
    }
    let limit = (r.buffer_bytes / 4).min(ABSOLUTE_LIMIT);
    if total > limit {
        return fail("retained product metadata exceed one quarter of buffer_bytes or 4 MiB");
    }
    Ok(total)
}

fn gdal_item<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!(r#"<Item name="{name}""#);
    let tail = xml.get(xml.find(&marker)? + marker.len()..)?;
    let value = tail.get(tail.find('>')? + 1..)?;
    value.get(..value.find("</Item>")?)
}

fn native_physical(source: &Value) -> Option<&Value> {
    source
        .pointer("/object_identities/native_cog")
        .or_else(|| source.get("physical_metadata"))
}

fn landsat_core(value: &str) -> Option<Vec<&str>> {
    let filename = value.split('?').next()?.rsplit('/').next()?;
    let parts = filename.split('_').collect::<Vec<_>>();
    if parts.len() < 6 {
        return None;
    }
    let collection = if parts.get(4).is_some_and(|part| part.len() == 8) {
        5
    } else {
        4
    };
    Some(vec![
        parts[0],
        parts[1],
        parts[2],
        parts[3],
        *parts.get(collection)?,
        parts.get(collection + 1)?.split('.').next()?,
    ])
}

fn landsat_band_suffix(href: &str, band: &str) -> bool {
    let filename = href
        .split('?')
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    filename.ends_with(&format!("_{band}.TIF"))
}

fn physical_identity(
    product: &str,
    observation: &Observation,
    band: &profile::BandProfile,
    asset: &ProductAsset,
    source: &Value,
) -> Result<()> {
    let p = profile::profile(product)?;
    if product == "landsat-c2-l2-sr" {
        let canonical = if asset.href.starts_with("https://") {
            Some(asset.href.as_str())
        } else {
            asset.metadata["href"].as_str()
        }
        .ok_or("Landsat product identity requires the canonical catalog asset href")?;
        let url = Url::parse(canonical)?;
        if url.scheme() != "https"
            || !url.host_str().is_some_and(|host| {
                host.starts_with("landsat") && host.ends_with(".blob.core.windows.net")
            })
            || !url.path().starts_with("/landsat-c2/")
            || !landsat_band_suffix(canonical, &band.name)
        {
            return fail("Landsat asset reference conflicts with product/band identity");
        }
        let filename = url.path().rsplit('/').next().unwrap_or_default();
        p.validate_identity(
            filename,
            observation.platform.as_deref(),
            observation.processing.as_deref(),
        )?;
        p.validate_datetime(filename, &observation.datetime)?;
        if landsat_core(filename) != landsat_core(&observation.id) {
            return fail("Landsat asset identifier conflicts with observation identifier");
        }
        return Ok(());
    }

    let physical =
        native_physical(source).ok_or("product source lacks retained native COG metadata")?;
    let xml = physical["gdal_metadata_xml"]
        .as_str()
        .ok_or("product source lacks retained GDAL metadata")?;
    let id_parts = observation.id.split('_').collect::<Vec<_>>();
    match product {
        "hls-l30" | "hls-s30" => {
            let hls = observation.id.split('.').collect::<Vec<_>>();
            let tile = hls
                .get(2)
                .and_then(|value| value.strip_prefix('T'))
                .ok_or("invalid HLS tile identity")?;
            let sensing_day = chrono::NaiveDateTime::parse_from_str(
                hls.get(3).ok_or("invalid HLS acquisition identity")?,
                "%Y%jT%H%M%S",
            )?
            .format("%Y-%m-%d")
            .to_string();
            if gdal_item(xml, "HLS_PROCESSING_TIME").is_none()
                || !gdal_item(xml, "SENSING_TIME").is_some_and(|value| value.contains(&sensing_day))
                || if product == "hls-l30" {
                    !gdal_item(xml, "LANDSAT_PRODUCT_ID").is_some_and(|value| {
                        value.starts_with("LC08_") || value.starts_with("LC09_")
                    }) || gdal_item(xml, "SENTINEL2_TILEID") != Some(tile)
                } else {
                    !gdal_item(xml, "PRODUCT_URI").is_some_and(|value| value.starts_with("S2"))
                        || !gdal_item(xml, "TILE_ID")
                            .is_some_and(|value| value.contains(&format!("_T{tile}_")))
                }
            {
                return fail("physical HLS metadata conflicts with product/granule identity");
            }
            if band.name == "Fmask" && !xml.contains("name=\"Fmask") {
                return fail("physical HLS metadata does not identify the Fmask layer");
            }
        }
        "opera-dswx-hls" => {
            let tile = id_parts.get(3).ok_or("invalid DSWx identity")?;
            let acquisition = chrono::NaiveDateTime::parse_from_str(
                id_parts.get(4).ok_or("invalid DSWx identity")?,
                "%Y%m%dT%H%M%SZ",
            )?;
            let platform = id_parts.get(6).ok_or("invalid DSWx identity")?;
            let (source_kind, sensor_prefix) = match *platform {
                "L8" => ("L30", "LC08_"),
                "L9" => ("L30", "LC09_"),
                "S2A" => ("S30", "S2A_"),
                "S2B" => ("S30", "S2B_"),
                "S2C" => ("S30", "S2C_"),
                _ => return fail("invalid DSWx platform identity"),
            };
            let expected_hls = format!(
                "HLS.{source_kind}.{tile}.{}.v2.0",
                acquisition.format("%Y%jT%H%M%S")
            );
            let sensor = gdal_item(xml, "SENSOR_PRODUCT_ID").unwrap_or_default();
            if gdal_item(xml, "PROJECT") != Some("OPERA")
                || gdal_item(xml, "PRODUCT_TYPE") != Some("DSWx-HLS")
                || gdal_item(xml, "PRODUCT_VERSION") != Some("1.0")
                || gdal_item(xml, "HLS_DATASET") != Some(expected_hls.as_str())
                || !sensor.starts_with(sensor_prefix)
                || !gdal_item(xml, "DESCRIPTION").is_some_and(|value| value.contains(&band.name))
            {
                return fail("physical DSWx metadata conflicts with product/granule/band identity");
            }
        }
        "opera-rtc-s1" => {
            let burst = id_parts
                .get(3)
                .ok_or("invalid RTC identity")?
                .replace('-', "_")
                .to_ascii_lowercase();
            let acquisition = id_parts.get(4).ok_or("invalid RTC identity")?;
            let platform = id_parts.get(6).ok_or("invalid RTC identity")?;
            let expected_platform = format!("Sentinel-1{}", &platform[2..]);
            let layer = gdal_item(xml, "LAYER_NAME").unwrap_or_default();
            let input = gdal_item(xml, "INPUT_L1_SLC_GRANULES").unwrap_or_default();
            let zero_doppler = gdal_item(xml, "ZERO_DOPPLER_START_TIME")
                .or_else(|| gdal_item(xml, "SOURCE_DATA_ZERO_DOPPLER_START_TIME"))
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.format("%Y%m%dT%H%M%SZ").to_string());
            // Prefer the explicit burst time. Older assets that omit it must
            // instead carry the exact product timestamp in their input SLC ID.
            let acquisition_matches = zero_doppler.as_deref().map_or_else(
                || input.contains(acquisition.trim_end_matches('Z')),
                |value| value == *acquisition,
            );
            if !gdal_item(xml, "BURST_ID").is_some_and(|value| value.eq_ignore_ascii_case(&burst)) {
                return fail("physical RTC burst metadata conflicts with product identity");
            }
            if gdal_item(xml, "PLATFORM") != Some(expected_platform.as_str())
                || !input.contains(platform)
            {
                return fail("physical RTC platform metadata conflicts with product identity");
            }
            if !acquisition_matches {
                return fail(
                    "physical RTC source acquisition time conflicts with product identity",
                );
            }
            let layer_matches = if band.name == "mask" {
                layer == "Mask Layer"
            } else {
                layer.contains(&format!(" {} Backscatter", band.name))
                    && gdal_item(
                        xml,
                        "PROCESSING_INFORMATION_OUTPUT_BACKSCATTER_EXPRESSION_CONVENTION",
                    ) == Some("linear backscatter intensity")
                    && gdal_item(
                        xml,
                        "PROCESSING_INFORMATION_OUTPUT_BACKSCATTER_NORMALIZATION_CONVENTION",
                    ) == Some("gamma0")
            };
            if !layer_matches {
                return fail("physical RTC layer metadata conflicts with requested band");
            }
            if gdal_item(xml, "PRODUCT_VERSION").is_some_and(|version| version != "1.0") {
                return fail("physical RTC version metadata conflicts with profile");
            }
        }
        _ => return fail("unsupported physical product identity gate"),
    }
    Ok(())
}
fn signed_landsat(c: &Client, href: &str) -> Result<String> {
    let url = Url::parse(href)?;
    if url.query().is_some() {
        return fail("Landsat source manifest must contain unsigned canonical URLs; signing stays runtime-only");
    }
    if url.scheme() != "https"
        || !url
            .host_str()
            .is_some_and(|h| h.starts_with("landsat") && h.ends_with(".blob.core.windows.net"))
        || !url.path().starts_with("/landsat-c2/")
    {
        return fail("Planetary Computer signer is confined to Landsat C2 Azure blob objects");
    }
    let mut signer = Url::parse("https://planetarycomputer.microsoft.com/api/sas/v1/sign")?;
    signer.query_pairs_mut().append_pair("href", href);
    let v = read_json(c.get(signer).send().map_err(|_| "Landsat signing failed")?)?;
    let signed = Url::parse(text(&v, "href")?)?;
    if signed.scheme() != url.scheme()
        || signed.host_str() != url.host_str()
        || signed.path() != url.path()
        || signed.username() != ""
        || signed.password().is_some()
    {
        return fail("signing changed the canonical source object");
    }
    Ok(signed.into())
}
/// Translate product selection to ordinary source assets, inspect, then check
/// scientific expectations. No descriptor can override a physical format gate.
pub fn prepare(r: &ProductRequest, cache: &mut LayoutCache) -> Result<acquisition::Prepared> {
    prepare_inner(r, cache, None)
}
/// Explicit Earthdata authorization for the separately qualified HLS COG route.
/// Credentials stay in exact-object sessions and are never manifest fields.
#[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
pub fn prepare_hls_authenticated(
    r: &ProductRequest,
    cache: &mut LayoutCache,
    netrc: &std::path::Path,
) -> Result<acquisition::Prepared> {
    if !["hls-l30", "hls-s30"].contains(&profile::profile(&r.product)?.id) {
        return fail("authenticated product COG route is confined to HLS L30/S30 v2");
    }
    prepare_inner(r, cache, Some(netrc))
}
fn prepare_inner(
    r: &ProductRequest,
    cache: &mut LayoutCache,
    netrc: Option<&std::path::Path>,
) -> Result<acquisition::Prepared> {
    r.validate()?;
    if let Some(observations) = &r.observations {
        metadata_reservation(r, observations)?;
    }
    let canonical_id = profile::profile(&r.product)?.id;
    let canonical = if r.product == canonical_id {
        None
    } else {
        let mut value = r.clone();
        value.product = canonical_id.into();
        Some(value)
    };
    let r = canonical.as_ref().unwrap_or(r);
    if !crate::cog::native_selected() {
        return fail("product profiles require native COG selection; build native-cog and select --backend native");
    }
    let catalog = discover(r)?;
    let p = profile::profile(&r.product)?;
    let acquisition_buffer_bytes = r
        .buffer_bytes
        .checked_sub(catalog.metadata_reservation_bytes)
        .filter(|bytes| *bytes >= 4 * 1024 * 1024)
        .ok_or("product metadata reservation leaves less than 4 MiB for native acquisition; raise buffer_bytes")?;
    let c = client()?;
    let mut assets = vec![];
    let mut metadata = vec![];
    let mut ids = BTreeSet::new();
    for (i, o) in catalog.observations.iter().enumerate() {
        if !ids.insert(&o.id) {
            return fail("duplicate product observation identity");
        }
        for requested in &r.bands {
            let b = p.band(requested)?;
            let a = &o.assets[&b.name];
            let id = format!("p{i:03}_{}", b.name);
            let href = if r.product == "landsat-c2-l2-sr" && a.href.starts_with("https://") {
                signed_landsat(&c, &a.href)?
            } else {
                a.href.clone()
            };
            assets.push(Asset {
                id: id.clone(),
                locator: href,
                format: Format::Cog,
                datetime: Some(o.datetime.clone()),
                etag: a.etag.clone(),
            });
            metadata.push(json!({"asset_id":id,"item_id":o.id,"collection":o.collection,"requested_band":b.name,"product":r.product,"product_version":p.version,"platform":o.platform,"processing":o.processing,"product_band":b,"acquisition":o.properties,"asset_metadata":a.metadata,"href":crate::stac_workflow::safe_reference(&a.href),"raster_bands":[{"scale":b.scale,"offset":b.offset,"unit":b.unit}],"quality_policies":r.quality_policies.iter().filter(|name|name.parse::<profile::QualityPolicy>().is_ok_and(|q|q.required_bands().contains(&b.name.as_str()))).collect::<Vec<_>>() }));
        }
    }
    let request = Request {
        source: Source::Assets { assets },
        spatial: r.aoi.clone(),
        time: Some(Time::Interval {
            start: r.start.clone(),
            end: r.end.clone(),
        }),
        variables: vec![Variable::Band { index: 1 }],
        quality: vec![],
        grid: Grid::Native,
        operations: vec![],
        sink: match &r.output {
            Some(path) => Sink::ZarrDirectory { path: path.clone() },
            None => Sink::Callback,
        },
        budget: Budget {
            buffer_bytes: acquisition_buffer_bytes,
            max_nodes: 100_000,
            chunk_edge: r.chunk_edge,
        },
    };
    let prepared = if let Some(netrc) = netrc {
        #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
        {
            let Source::Assets { assets } = &request.source else {
                unreachable!()
            };
            let mut sessions = BTreeMap::new();
            for asset in assets {
                sessions.insert(
                    asset.id.clone(),
                    std::sync::Arc::new(
                        crate::authenticated_http::AuthenticatedHttp::earthdata_hls(
                            &asset.locator,
                            netrc,
                        )?,
                    ),
                );
            }
            acquisition::prepare_authenticated_cogs(&request, cache, sessions)?
        }
        #[cfg(not(all(feature = "native-cog", feature = "authenticated-http")))]
        {
            let _ = netrc;
            return fail("authenticated HLS COG acquisition requires authenticated-http feature");
        }
    } else {
        acquisition::prepare(&request, cache)?
    };
    let physical_inspection = prepared.explain();
    for selected in &prepared.plan().selected {
        let l = selected
            .layout
            .as_ref()
            .ok_or("product source missing inspected layout")?;
        let source = metadata
            .iter()
            .find(|m| m["asset_id"] == selected.asset_id)
            .ok_or("product source metadata missing")?;
        let band = p.band(source["requested_band"].as_str().unwrap())?;
        let observation = catalog
            .observations
            .iter()
            .find(|observation| source["item_id"].as_str() == Some(observation.id.as_str()))
            .ok_or("product observation metadata missing")?;
        let asset = observation
            .assets
            .get(&band.name)
            .ok_or("product asset metadata missing")?;
        let physical_source = physical_inspection["sources"]
            .as_array()
            .and_then(|sources| {
                sources
                    .iter()
                    .find(|candidate| candidate["asset_id"] == selected.asset_id)
            })
            .ok_or("inspected physical product source metadata missing")?;
        physical_identity(&r.product, observation, band, asset, physical_source)?;
        let dtype = match l.dtype.as_str() {
            "UInt16" | "<u2" => "uint16",
            "Int16" | "<i2" => "int16",
            "Byte" | "UInt8" | "|u1" => "uint8",
            "Float32" | "<f4" => "float32",
            _ => "unsupported",
        };
        if dtype != band.dtype.name() {
            return fail(&format!(
                "{}: actual TIFF dtype {dtype} conflicts with product band {}",
                r.product, band.name
            ));
        }
        let expected_nodata = band.nodata.map(profile::NoDataValue::as_f64);
        let catalog_nodata = asset.metadata["raster:bands"]
            .as_array()
            .and_then(|bands| bands.first())
            .and_then(|raster| raster["nodata"].as_f64());
        let nodata_matches = match (expected_nodata, l.nodata, catalog_nodata) {
            (Some(expected), Some(actual), declared) => {
                actual.is_finite()
                    && actual == expected
                    && declared.is_none_or(|value| value == expected)
            }
            (Some(expected), None, Some(declared)) => declared == expected,
            (Some(_), None, None) => true,
            (None, None, None) => true,
            // Native RTC normalizes a non-finite GDAL_NODATA token to no finite
            // replacement while retaining the exact token in physical metadata.
            (None, Some(actual), None) => actual.is_nan(),
            _ => false,
        };
        if !nodata_matches {
            return fail("TIFF nodata conflicts with authoritative product profile");
        }
        if l.transform[1] != 30.
            || l.transform[5] != -30.
            || l.transform[2] != 0.
            || l.transform[4] != 0.
            || !l
                .crs
                .strip_prefix("EPSG:")
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|epsg| {
                    (32601..=32660).contains(&epsg) || (32701..=32760).contains(&epsg)
                })
        {
            return fail("product profile requires actual north-up 30 m WGS84 UTM source grid");
        }
        if l.scale
            .zip(band.scale)
            .is_some_and(|(a, b)| (a - b).abs() > 1e-12)
            || l.offset
                .zip(band.offset)
                .is_some_and(|(a, b)| (a - b).abs() > 1e-12)
        {
            return fail("TIFF calibration conflicts with authoritative product profile");
        }
    }
    Ok(prepared.with_product_metadata(metadata,json!({"profile":p,"quality_policies":r.quality_policies,"mask_scope":"derived masks belong to their raw QA/classification observation; no implicit cross-band alignment","discovery_seconds":catalog.seconds,"metadata_reservation_bytes":catalog.metadata_reservation_bytes,"requested_total_buffer_bytes":r.buffer_bytes,"acquisition_buffer_bytes":acquisition_buffer_bytes})))
}

/// Evaluate a named policy against its raw layer without mutating the source.
/// The caller combines this mask with explicit AOI inclusion and source validity.
pub fn derive_quality_mask(
    policy: profile::QualityPolicy,
    pixels: &crate::Pixels,
) -> Result<Vec<u8>> {
    use profile::QualitySample;
    (0..pixels.len())
        .map(|i| {
            let sample = match pixels {
                crate::Pixels::UInt8(v) => QualitySample::UInt8(v[i]),
                crate::Pixels::UInt16(v) => QualitySample::UInt16(v[i]),
                crate::Pixels::Float32(v) => QualitySample::Float32(v[i]),
                _ => return fail("quality policy received incompatible source dtype"),
            };
            Ok(u8::from(policy.accepts(sample)?))
        })
        .collect()
}

/// Product-semantic source validity, kept separate from physical TIFF validity
/// and AOI inclusion. This never mutates a stored sample.
pub fn source_validity<'a>(
    profile: &'a profile::ProductProfile,
    band: &'a profile::BandProfile,
    pixels: &'a crate::Pixels,
) -> impl Iterator<Item = bool> + 'a {
    let nodata = band.nodata.map(profile::NoDataValue::as_f64);
    (0..pixels.len()).map(move |i| {
        if !pixels.valid(i, nodata) {
            return false;
        }
        if profile.family == profile::ProductFamily::Landsat {
            match (band.name.as_str(), pixels) {
                ("QA_PIXEL", crate::Pixels::UInt16(values)) => values[i] & 1 == 0,
                ("SR_QA_AEROSOL", crate::Pixels::UInt8(values)) => values[i] & 1 == 0,
                ("QA_PIXEL" | "SR_QA_AEROSOL", _) => false,
                _ => true,
            }
        } else {
            true
        }
    })
}
