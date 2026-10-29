//! Scientific product selection over the existing bounded acquisition lifecycle.
//! Profiles describe expectations; the physical reader still inspects every source.
#[cfg(feature = "native-hdf5")]
pub mod array_grid;
#[cfg(feature = "native-hdf5")]
pub mod arrays;
pub mod dist_ann;
pub mod dist_s1;
pub mod dswx_ni;
pub mod profile;
pub mod routing;
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
    #[serde(default)]
    pub provider: routing::Provider,
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
    pub provider: routing::Provider,
    pub routing: routing::Decision,
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
        let max_days = if p.family == profile::ProductFamily::OperaDistAnn {
            366
        } else {
            31
        };
        if end < start || end - start > chrono::Duration::days(max_days) {
            return fail("product date interval must be ordered and within its product limit (31 days, or 366 days for annual DIST)");
        }
        if !(1..=32).contains(&self.max_observations)
            || self.bands.is_empty()
            || self.bands.len()
                > if p.family == profile::ProductFamily::OperaDistAnn {
                    21
                } else {
                    16
                }
            || self.buffer_bytes < 4 * 1024 * 1024
            || self.buffer_bytes > 1024 * 1024 * 1024
            || self.chunk_edge == 0
            || self.chunk_edge > 1024
        {
            return fail("invalid product limits: 1..32 observations, 1..16 bands (21 for annual DIST), 4..1024 MiB buffer, chunk edge 1..1024");
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
    discover_routed(request, false)
}
/// Catalog-only policy with explicit authorization availability. This never
/// discovers or reads credentials; callers must pass the selected session at preparation.
pub fn discover_with_authorization(
    request: &ProductRequest,
    explicitly_configured: bool,
) -> Result<Catalog> {
    discover_routed(request, explicitly_configured)
}
fn discover_routed(request: &ProductRequest, authorized: bool) -> Result<Catalog> {
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
    let (selected, observations, catalog_comparison) = if request.observations.is_none()
        && request.provider == routing::Provider::Auto
        && request.product.starts_with("hls-")
    {
        discover_hls_alternatives(request, authorized)?
    } else {
        let selected = routing::select(request, authorized)?;
        let observations = if let Some(obs) = &request.observations {
            obs.clone()
        } else if selected == routing::Provider::PlanetaryComputer {
            discover_planetary(request)?
        } else {
            discover_cmr(request)?
        };
        let audit = json!({"mode":"explicit_or_pinned","queried_providers":if request.observations.is_none() {vec![selected]} else {vec![]},"other_catalogs":"not queried; supplied sources and explicit overrides never trigger substitution"});
        (selected, observations, audit)
    };
    let mut observation_ids = BTreeSet::new();
    for obs in &observations {
        if !observation_ids.insert(&obs.id) {
            return fail("duplicate or ambiguous product observation identity");
        }
        validate_observation(request, obs)?;
        routing::validate_source(request, selected, obs)?;
    }
    let mut routing = routing::decision(request, selected, authorized, &observations)?;
    routing.eligibility = catalog_comparison;
    let metadata_reservation_bytes = metadata_reservation(request, &observations)?;
    Ok(Catalog {
        schema: "tile-ripper-products-v1",
        product: request.product.clone(),
        provider: selected,
        routing,
        complete: true,
        observations,
        metadata_reservation_bytes,
        seconds: started.elapsed().as_secs_f64(),
    })
}
/// Compare only bounded catalog metadata. Never fetch science to select a route.
fn discover_hls_alternatives(
    r: &ProductRequest,
    authorized: bool,
) -> Result<(routing::Provider, Vec<Observation>, Value)> {
    let mut pc = discover_planetary(r);
    let mut nasa = discover_cmr(r);
    // Admit each catalog independently before using its coverage in the policy.
    for (provider, result) in [
        (routing::Provider::PlanetaryComputer, &mut pc),
        (routing::Provider::Earthdata, &mut nasa),
    ] {
        if let Ok(observations) = result {
            let checked = (|| -> Result<()> {
                metadata_reservation(r, observations)?;
                let mut ids = BTreeSet::new();
                for o in observations {
                    if !ids.insert(o.id.clone()) {
                        return fail("catalog contains duplicate observation identities");
                    }
                    validate_observation(r, o)?;
                    routing::validate_source(r, provider, o)?;
                }
                Ok(())
            })();
            if let Err(error) = checked {
                *result = Err(error);
            }
        }
    }
    let pc_ids = pc
        .as_ref()
        .ok()
        .map(|obs| obs.iter().map(|o| o.id.clone()).collect::<BTreeSet<_>>());
    let nasa_ids = nasa
        .as_ref()
        .ok()
        .map(|obs| obs.iter().map(|o| o.id.clone()).collect::<BTreeSet<_>>());
    let (selected, reason) = routing::choose_coverage(
        pc_ids.as_ref(),
        nasa_ids.as_ref(),
        authorized,
    )
    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
        format!(
            "{error}; Planetary Computer discovery: {}; Earthdata discovery: {}",
            pc.as_ref()
                .map(|observations| format!("complete, {} observations", observations.len()))
                .unwrap_or_else(|error| error.to_string()),
            nasa.as_ref()
                .map(|observations| format!("complete, {} observations", observations.len()))
                .unwrap_or_else(|error| error.to_string())
        )
        .into()
    })?;
    let audit = |provider, result: &Result<Vec<Observation>>| match result {
        Ok(observations) => {
            json!({"provider":provider,"status":"complete","count":observations.len(),"observation_ids":observations.iter().map(|o|&o.id).collect::<Vec<_>>(),"credentials_required":provider==routing::Provider::Earthdata})
        }
        // Library errors contain closed transport/status/validation labels, not bearer URLs.
        Err(error) => {
            json!({"provider":provider,"status":"unavailable_or_incomplete","error":error.to_string(),"coverage_known":false})
        }
    };
    let comparison = json!({"mode":"bounded_metadata_only","decision":reason,"catalogs":[audit(routing::Provider::PlanetaryComputer,&pc),audit(routing::Provider::Earthdata,&nasa)],"earthdata_only_ids":nasa_ids.as_ref().map(|n|n.difference(&pc_ids.clone().unwrap_or_default()).cloned().collect::<Vec<_>>()),"planetary_only_ids":pc_ids.as_ref().map(|p|p.difference(&nasa_ids.clone().unwrap_or_default()).cloned().collect::<Vec<_>>()),"equivalence":"coverage comparisons use exact granule identities within the v2 profile; cross-provider sample equivalence remains separately bound to inspected objects","no_science_download_for_routing":true});
    let observations = if selected == routing::Provider::PlanetaryComputer {
        pc?
    } else {
        nasa?
    };
    Ok((selected, observations, comparison))
}
fn discover_planetary(r: &ProductRequest) -> Result<Vec<Observation>> {
    let collection = match r.product.as_str() {
        "landsat-c2-l2-sr" | "landsat-tm-c2-l2-sr" | "landsat-etm-c2-l2-sr" => "landsat-c2-l2",
        "hls-l30" => "hls2-l30",
        "hls-s30" => "hls2-s30",
        _ => return fail("no qualified Planetary Computer route for this product"),
    };
    let mut url = Url::parse(&format!("{PC}/search"))?;
    url.query_pairs_mut()
        .append_pair("collections", collection)
        .append_pair(
            "bbox",
            &geographic_bounds(&r.aoi)?
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .append_pair("datetime", &format!("{}/{}", r.start, r.end))
        .append_pair("limit", &(r.max_observations + 1).to_string());
    let platforms: &[&str] = match r.product.as_str() {
        "landsat-c2-l2-sr" => &["landsat-8", "landsat-9"],
        "landsat-tm-c2-l2-sr" => &["landsat-4", "landsat-5"],
        "landsat-etm-c2-l2-sr" => &["landsat-7"],
        _ => &[],
    };
    if !platforms.is_empty() {
        url.query_pairs_mut()
            .append_pair("query", &json!({"platform":{"in":platforms}}).to_string());
    }
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
        if !platforms.is_empty()
            && !platforms.contains(&item["properties"]["platform"].as_str().unwrap_or(""))
        {
            return fail("STAC search returned a sensor outside the requested Landsat profile");
        }
        let mut assets = BTreeMap::new();
        for requested in &r.bands {
            let band = p.band(requested)?;
            let key = if r.product.starts_with("hls-") {
                &band.name
            } else {
                &band.asset_key
            };
            let asset = &item["assets"][key];
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
        "opera-dswx-s1" => Ok(("OPERA_L3_DSWX-S1_V1", "1.0")),
        "opera-dist-alert" => Ok(("OPERA_L3_DIST-ALERT-HLS_V1", "1")),
        "opera-dist-alert-s1" => Ok(("OPERA_L3_DIST-ALERT-S1_V1", "1")),
        "opera-dist-ann-hls" => Ok(("OPERA_L3_DIST-ANN-HLS_V1", "1")),
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
        let mut properties = json!({"cmr_concept_id":e["id"],"collection_concept_id":concept,"updated":e["updated"],"source_time_start":e["time_start"],"source_time_end":e["time_end"]});
        if r.product == dist_ann::ID {
            properties["temporal_extent"] = json!({
                "coverage_start":text(e,"time_start")?,
                "coverage_end":text(e,"time_end")?,
                "summary_year":dist_ann::summary_year(id)?,
            });
        }
        out.push(Observation {
            id: id.into(),
            datetime: text(e, "time_start")?.into(),
            collection: short.into(),
            platform: None,
            processing: None,
            assets,
            properties,
        });
    }
    Ok(out)
}
/// Annual TIFFs do not encode their coverage interval. Bind supplied intervals
/// to a fresh exact CMR record, rather than trusting caller-authored properties.
fn verify_annual_catalog_extent(observation: &Observation, entry: &Value) -> Result<Value> {
    let id = entry["producer_granule_id"]
        .as_str()
        .or_else(|| entry["title"].as_str())
        .ok_or("CMR annual granule identity missing")?;
    if id != observation.id {
        return fail("CMR annual granule identity mismatch");
    }
    let extent = dist_ann::temporal_extent(observation)?;
    let start = chrono::DateTime::parse_from_rfc3339(text(entry, "time_start")?)?;
    let end = chrono::DateTime::parse_from_rfc3339(text(entry, "time_end")?)?;
    if extent.coverage_start != start || extent.coverage_end != end {
        return fail(
            "annual supplied coverage interval conflicts with the exact live CMR granule record",
        );
    }
    let links = entry["links"]
        .as_array()
        .ok_or("CMR annual data links missing")?;
    for asset in observation.assets.values() {
        if !links.iter().any(|link| {
            link["inherited"] != true
                && link["rel"]
                    .as_str()
                    .is_some_and(|rel| rel.ends_with("/data#"))
                && link["href"] == asset.href
        }) {
            return fail("annual supplied asset is absent from exact live CMR granule record");
        }
    }
    Ok(
        json!({"authority":"NASA CMR exact granule record, not TIFF metadata","collection_concept_id":"C2519119034-LPCLOUD","granule_concept_id":entry["id"],"revision_id":entry["revision_id"],"updated":entry["updated"],"coverage_start":entry["time_start"],"coverage_end":entry["time_end"]}),
    )
}
fn verify_annual_supplied_observations(observations: &mut [Observation]) -> Result<()> {
    let client = client()?;
    for observation in observations {
        let mut url = Url::parse(&format!("{CMR}/granules.json"))?;
        url.query_pairs_mut()
            .append_pair("collection_concept_id", "C2519119034-LPCLOUD")
            .append_pair("granule_ur", &observation.id)
            .append_pair("page_size", "2");
        let value = read_json(
            client
                .get(url)
                .send()
                .map_err(|_| "annual exact CMR verification transport failed")?,
        )?;
        let entries = value["feed"]["entry"]
            .as_array()
            .ok_or("CMR annual verification entries missing")?;
        if entries.len() != 1 {
            return fail("annual exact CMR verification is missing or ambiguous");
        }
        observation.properties["verified_catalog_extent"] =
            verify_annual_catalog_extent(observation, &entries[0])?;
    }
    Ok(())
}
fn band_href(product: &str, band: &str, href: &str) -> bool {
    match product {
        dswx_ni::ID => return dswx_ni::band_href(band).is_ok_and(|canonical| canonical == href),
        dist_s1::ID => return dist_s1::band_href(band, href),
        dist_ann::ID => return dist_ann::band_href(band, href),
        _ => {}
    }
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
    match p.family {
        profile::ProductFamily::OperaDswxNiPrototype => dswx_ni::validate_observation(r, o)?,
        profile::ProductFamily::OperaDistS1 => dist_s1::validate_observation(r, o)?,
        profile::ProductFamily::OperaDistAnn => dist_ann::validate_observation(r, o)?,
        _ => {}
    }
    let collection_ok = if p.family == profile::ProductFamily::OperaDswxNiPrototype {
        o.collection == p.catalog.collection
    } else if p.family == profile::ProductFamily::Landsat {
        o.collection == "landsat-c2-l2"
    } else {
        let (short, version) = collection(&r.product)?;
        o.collection == short
            || o.collection == format!("{short}_{version}")
            || (r.product.starts_with("hls-")
                && (o.collection == format!("{short}.020")
                    || o.collection
                        == if r.product == "hls-l30" {
                            "hls2-l30"
                        } else {
                            "hls2-s30"
                        }))
    };
    if !collection_ok {
        return fail("observation collection conflicts with product identity");
    }
    let time = chrono::DateTime::parse_from_rfc3339(&o.datetime)?;
    if p.family != profile::ProductFamily::OperaDistAnn
        && (time < chrono::DateTime::parse_from_rfc3339(&r.start)?
            || time > chrono::DateTime::parse_from_rfc3339(&r.end)?)
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

fn inspected_product_identity(
    product: &str,
    observation: &Observation,
    band: &profile::BandProfile,
    asset: &ProductAsset,
    source: &Value,
    layout: &crate::Layout,
) -> Result<()> {
    match product {
        dswx_ni::ID => return dswx_ni::validate_physical(observation, band, asset, source, layout),
        dist_s1::ID => return dist_s1::validate_physical(observation, band, asset, source, layout),
        dist_ann::ID => {
            return dist_ann::validate_physical(observation, band, asset, source, layout)
        }
        _ => {}
    }
    physical_identity(product, observation, band, asset, source)
}

fn physical_identity(
    product: &str,
    observation: &Observation,
    band: &profile::BandProfile,
    asset: &ProductAsset,
    source: &Value,
) -> Result<()> {
    let p = profile::profile(product)?;
    if p.family == profile::ProductFamily::Landsat {
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
        "opera-dswx-s1" | "opera-dist-alert" => {
            let dist = product == "opera-dist-alert";
            let canonical = if asset.href.starts_with("https://") {
                Some(asset.href.as_str())
            } else {
                asset.metadata["href"]
                    .as_str()
                    .or_else(|| asset.metadata["canonical_href"].as_str())
            }
            .ok_or("Wave 2 OPERA requires retained canonical catalog href")?;
            let url = Url::parse(canonical)?;
            let expected_prefix = if dist {
                "/lp-prod-protected/OPERA_L3_DIST-ALERT-HLS_V1/"
            } else {
                "/podaac-ops-cumulus-protected/OPERA_L3_DSWX-S1_V1/"
            };
            let expected_host = if dist {
                "data.lpdaac.earthdatacloud.nasa.gov"
            } else {
                "archive.podaac.earthdata.nasa.gov"
            };
            let suffix = if dist {
                band.name.clone()
            } else {
                format!(
                    "B{:02}_{}",
                    match band.name.as_str() {
                        "WTR" => 1,
                        "BWTR" => 2,
                        "CONF" => 3,
                        "DIAG" => 4,
                        _ => return fail("invalid DSWx-S1 band"),
                    },
                    band.name
                )
            };
            let filename = format!("{}_{}.tif", observation.id, suffix);
            let expected_path = if dist {
                format!("{expected_prefix}{}/{}", observation.id, filename)
            } else {
                format!("{expected_prefix}{filename}")
            };
            if url.scheme() != "https"
                || url.host_str() != Some(expected_host)
                || url.port().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != expected_path
            {
                return fail("Wave 2 OPERA canonical asset conflicts with exact granule/layer");
            }
            if !dist || band.name != "DATA-MASK" {
                let observed = chrono::DateTime::parse_from_rfc3339(&observation.datetime)?;
                let time = gdal_item(
                    xml,
                    if dist {
                        "Update_Date"
                    } else {
                        "RTC_SENSING_START_TIME"
                    },
                )
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                .ok_or("Wave 2 OPERA physical source lacks sensing timestamp")?;
                if (time - observed).num_milliseconds().abs() > 1 {
                    return fail(
                        "Wave 2 OPERA physical sensing time conflicts with catalog observation",
                    );
                }
            }
            if dist {
                let description = match band.name.as_str() {
                    "VEG-DIST-STATUS" => "Vegetation_disturbance_status",
                    "GEN-DIST-STATUS" => "Generic_disturbance_status",
                    "VEG-DIST-CONF" => "Confidence_of_vegetation_disturbance",
                    "GEN-DIST-CONF" => "Confidence_of_generic_disturbance",
                    "VEG-DIST-DATE" => "Day_of_vegetation_disturbance",
                    "GEN-DIST-DATE" => "Day_of_generic_disturbance",
                    "VEG-LAST-DATE" => {
                        "Day_of_last_land_observation_for_vegetation_disturbance_detection"
                    }
                    "GEN-LAST-DATE" => {
                        "Day_of_last_land_observation_for_generic_disturbance_detection"
                    }
                    "DATA-MASK" => "Data_mask",
                    _ => return fail("unqualified DIST layer"),
                };
                let units = if band.name.ends_with("DATE") {
                    "days"
                } else {
                    "unitless"
                };
                if gdal_item(xml, "DESCRIPTION") != Some(description)
                    || gdal_item(xml, "Units") != Some(units)
                {
                    return fail("DIST-ALERT physical layer description/units conflict with requested variable");
                }
                if (band.name != "DATA-MASK"
                    && gdal_item(xml, "Input_DIST-ALERT_granule").is_none_or(str::is_empty))
                    || (band.name == "DATA-MASK" && gdal_item(xml, "flag_values") != Some("0,1,2"))
                    || gdal_item(xml, "Units").is_none()
                    || (band.name.ends_with("STATUS")
                        && gdal_item(xml, "flag_values") != Some("0,1,2,3,4,5,6,7,8,255"))
                {
                    return fail("DIST-ALERT source lacks required status/lineage/unit metadata");
                }
            } else if gdal_item(xml, "PROJECT") != Some("OPERA")
                || gdal_item(xml, "PRODUCT_TYPE") != Some("DSWx-S1")
                || gdal_item(xml, "DSWX_PRODUCT_VERSION") != Some("1.0")
                || gdal_item(xml, "SOFTWARE_VERSION") != Some("1.2")
                || gdal_item(xml, "PRODUCT_SOURCE") != Some("OPERA_RTC_S1")
                || gdal_item(xml, "RTC_PRODUCT_VERSION") != Some("1.0")
                || !gdal_item(xml, "RTC_INPUT_LIST").is_some_and(|v| v.contains("OPERA_L2_RTC-S1_"))
                || gdal_item(xml, "PROCESSING_DATETIME")
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                    .is_none()
                || !gdal_item(xml, "DESCRIPTION")
                    .is_some_and(|v| v.contains(&format!("({})", band.name)))
            {
                return fail("DSWx-S1 physical identity/layer/version/lineage conflicts with qualified profile");
            }
        }
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
                || gdal_item(xml, "PRODUCT_VERSION") != Some(p.processing_version(&observation.id)?)
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
/// Bind the inspected Wave 2 MGRS raster to its named tile. The current
/// products are 109.8 km square, with the 100 km tile at the upper left;
/// DSWx-S1 snaps that corner to its 30 m source grid (within two pixels).
fn wave2_grid_identity(observation: &Observation, l: &crate::Layout) -> Result<()> {
    let tile = observation
        .id
        .split('_')
        .nth(3)
        .ok_or("missing OPERA MGRS tile")?;
    wave2_grid_identity_for_tile(tile, l, false)
}

/// Bind an inspected Wave 2 grid to a named MGRS tile.  `extended_north` is
/// private to the annual DIST preselection gate; it means an actual EPSG:326xx
/// source holds a southern tile with zero false northing, not that its CRS or
/// coordinates should be normalized.
pub(crate) fn wave2_grid_identity_for_tile(
    tile: &str,
    l: &crate::Layout,
    extended_north: bool,
) -> Result<()> {
    let bytes = tile.as_bytes();
    if bytes.len() != 6 || bytes[0] != b'T' || !bytes[1..].iter().all(u8::is_ascii_alphanumeric) {
        return fail("invalid Wave 2 MGRS tile");
    }
    let zone = tile[1..3].parse::<usize>()?;
    let bands = b"CDEFGHJKLMNPQRSTUVWX";
    if !(1..=60).contains(&zone) || !bands.contains(&bytes[3]) {
        return fail("invalid Wave 2 MGRS zone/latitude band");
    }
    if extended_north && bytes[3] >= b'N' {
        return fail("extended-north UTM is only valid for a southern MGRS tile");
    }
    let epsg = if bytes[3] >= b'N' || extended_north {
        32600 + zone
    } else {
        32700 + zone
    };
    if l.width != 3660 || l.height != 3660 || l.crs != format!("EPSG:{epsg}") {
        return fail("Wave 2 OPERA source dimensions/UTM zone/hemisphere conflict with MGRS tile");
    }
    let columns: &[u8] = [
        b"ABCDEFGH".as_slice(),
        b"JKLMNPQR".as_slice(),
        b"STUVWXYZ".as_slice(),
    ][(zone - 1) % 3];
    let column = columns
        .iter()
        .position(|x| *x == bytes[4])
        .ok_or("invalid MGRS column for UTM zone")?
        + 1;
    let row = b"ABCDEFGHJKLMNPQRSTUV"
        .iter()
        .position(|x| *x == bytes[5])
        .ok_or("invalid MGRS row")?;
    let row = (row + 20 - if zone % 2 == 0 { 5 } else { 0 }) % 20;
    let top = (l.transform[3] / 100_000.).round();
    // Row letters repeat every 2,000 km. Choose that cycle from the band's
    // center using the already-qualified UTM projection. Even X's 12 degree
    // span plus one 100 km square fits within half of that period.
    #[cfg(feature = "native-cog")]
    {
        let index = bands
            .iter()
            .position(|b| *b == bytes[3])
            .ok_or("invalid MGRS latitude band")?;
        let latitude = if index == 19 {
            78.
        } else {
            -76. + 8. * index as f64
        };
        let from = proj4rs::Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs")?;
        let to = proj4rs::Proj::from_proj_string(&format!(
            "+proj=utm +zone={zone} {} +datum=WGS84 +units=m +no_defs",
            if bytes[3] < b'N' && !extended_north {
                "+south"
            } else {
                ""
            }
        ))?;
        let mut center = (
            (zone as f64 * 6. - 183.).to_radians(),
            latitude.to_radians(),
            0.,
        );
        proj4rs::transform::transform(&from, &to, &mut center)?;
        if ((top * 100_000. - 50_000.) - center.1).abs() >= 1_000_000. {
            return fail("Wave 2 OPERA source northing cycle conflicts with MGRS latitude band");
        }
    }
    let actual_row = ((top as i64 - 1).rem_euclid(20)) as usize;
    if (l.transform[0] - column as f64 * 100_000.).abs() > 60.
        || (l.transform[3] - top * 100_000.).abs() > 60.
        || actual_row != row
    {
        return fail("Wave 2 OPERA source corner conflicts with MGRS 100 km tile");
    }
    Ok(())
}

/// Candidate identity for the inspected Planetary Computer HLS southern-frame
/// convention. This cannot be enabled by a public request/layout flag.
pub(crate) fn hls_pc_southern_tile(
    product: &str,
    observation: &Observation,
    band: &str,
    asset: &ProductAsset,
) -> Result<Option<String>> {
    let p = profile::profile(product)?;
    if p.family != profile::ProductFamily::Hls {
        return fail("PC HLS southern-grid convention is confined to HLS L30/S30 v2");
    }
    p.validate_identity(
        &observation.id,
        observation.platform.as_deref(),
        observation.processing.as_deref(),
    )?;
    p.validate_datetime(&observation.id, &observation.datetime)?;
    routing::validate_planetary_locator(p.id, &asset.href)?;
    let collection = if p.id == profile::HLS_L30_V2 {
        "hls2-l30"
    } else {
        "hls2-s30"
    };
    if observation.collection != collection
        || !p.bands.iter().any(|b| b.name == band)
        || !Url::parse(&asset.href)?
            .path()
            .ends_with(&format!("/{0}/{0}.{band}.tif", observation.id))
    {
        return fail("PC HLS southern-grid catalog/source/granule/band identity conflicts");
    }
    let tile = observation
        .id
        .split('.')
        .nth(2)
        .ok_or("HLS identity has no MGRS tile")?;
    Ok((tile.as_bytes()[3] < b'N').then(|| tile.to_owned()))
}

/// Bind the actual TIFF grid, retained HLS tags and object pin before allowing
/// WGS84 AOI selection in its EPSG:326xx frame with southern negative northings.
pub(crate) fn hls_pc_extended_northern_grid(
    product: &str,
    observation: &Observation,
    band: &profile::BandProfile,
    asset: &ProductAsset,
    source: &Value,
    layout: &crate::Layout,
) -> Result<u32> {
    let tile = hls_pc_southern_tile(product, observation, &band.name, asset)?
        .ok_or("PC HLS extended-north mapping requires a southern MGRS tile")?;
    physical_identity(product, observation, band, asset, source)?;
    let zone = tile[1..3].parse::<u32>()?;
    if layout.crs != format!("EPSG:{}", 32600 + zone)
        || layout.transform.iter().any(|n| !n.is_finite())
        || [
            layout.transform[1],
            layout.transform[2],
            layout.transform[4],
            layout.transform[5],
        ] != [30., 0., 0., -30.]
        || layout.transform[3] + layout.height as f64 * layout.transform[5] >= 0.
    {
        return fail("PC HLS extended-north source must retain its actual 30 m northern UTM CRS and southern physical frame");
    }
    let physical = native_physical(source)
        .ok_or("PC HLS convention requires inspected native COG metadata")?;
    if physical["normalized_crs"].as_str() != Some(layout.crs.as_str()) {
        return fail("PC HLS inspected CRS conflicts with the native layout");
    }
    let pin = source["object_version"]
        .as_str()
        .filter(|s| s.starts_with('"') && s.ends_with('"'))
        .ok_or("PC HLS convention requires the inspected immutable object ETag")?;
    if asset
        .etag
        .as_deref()
        .is_some_and(|expected| expected != pin)
    {
        return fail("PC HLS inspected object ETag conflicts with the selected source pin");
    }
    let xml = physical["gdal_metadata_xml"]
        .as_str()
        .ok_or("PC HLS convention lacks source TIFF tags")?;
    let expected_south = format!("EPSG:{}", 32700 + zone);
    if gdal_item(xml, "HORIZONTAL_CS_CODE") != Some(expected_south.as_str())
        || gdal_item(xml, "ULX").and_then(|v| v.parse::<f64>().ok()) != Some(layout.transform[0])
        || gdal_item(xml, "ULY").and_then(|v| v.parse::<f64>().ok()) != Some(layout.transform[3])
        || gdal_item(xml, "NCOLS").and_then(|v| v.parse::<usize>().ok()) != Some(layout.width)
        || gdal_item(xml, "NROWS").and_then(|v| v.parse::<usize>().ok()) != Some(layout.height)
        || gdal_item(xml, "SPATIAL_RESOLUTION").and_then(|v| v.parse::<f64>().ok()) != Some(30.)
    {
        return fail("PC HLS southern-grid TIFF tags conflict with the inspected source frame");
    }
    wave2_grid_identity_for_tile(&tile, layout, true)?;
    Ok(zone)
}

fn signed_planetary(
    c: &Client,
    signatures: &mut routing::PlanetarySignatures,
    product: &str,
    href: &str,
) -> Result<String> {
    let result = signatures.sign(product, href, || {
        let mut signer = Url::parse("https://planetarycomputer.microsoft.com/api/sas/v1/sign")?;
        signer.query_pairs_mut().append_pair("href", href);
        let v = read_json(
            c.get(signer)
                .send()
                .map_err(|_| "signing transport failed")?,
        )?;
        let signed = text(&v, "href")?;
        Ok(signed.into())
    });
    result.map_err(|error| {
        let alternative = if product.starts_with("hls-") {
            " For a different qualified route, explicitly create a fresh --provider earthdata plan with --auth earthdata --netrc PRIVATE_FILE; supplied observations must be explicitly rediscovered against that provider."
        } else { "" };
        format!("selected planetary_computer source {} could not be signed: {error}; no provider switch occurred. Explicitly create a fresh plan to retry the canonical object.{alternative}",crate::stac_workflow::safe_reference(href)).into()
    })
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
/// Explicit authorization for the separately qualified HLS, current DSWx-HLS,
/// and current ASF RTC-S1 exact-object COG routes. Each family chooses its own
/// observed redirect policy; credentials never become request metadata.
#[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
pub fn prepare_earthdata_authenticated(
    r: &ProductRequest,
    cache: &mut LayoutCache,
    netrc: &std::path::Path,
) -> Result<acquisition::Prepared> {
    // An explicitly supplied authorization is considered by routing; a free
    // route must never consult the file simply because it was configured.
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
    let mut catalog = discover_routed(r, netrc.is_some())?;
    // Routing is final before source inspection. Never open credentials for a
    // credential-free selection, even when authorization was supplied.
    let netrc = if catalog.provider == routing::Provider::Earthdata {
        netrc
    } else {
        None
    };
    if catalog.provider == routing::Provider::Earthdata
        && netrc.is_none()
        && !catalog.observations.is_empty()
    {
        return fail("selected Earthdata route requires explicit --auth earthdata --netrc PRIVATE_FILE; use a fresh --provider planetary_computer plan where qualified coverage exists");
    }
    let p = profile::profile(&r.product)?;
    if p.family == profile::ProductFamily::OperaDistAnn && r.observations.is_some() {
        verify_annual_supplied_observations(&mut catalog.observations)?;
        catalog.metadata_reservation_bytes = metadata_reservation(r, &catalog.observations)?;
    }
    let acquisition_buffer_bytes = r
        .buffer_bytes
        .checked_sub(catalog.metadata_reservation_bytes)
        .filter(|bytes| *bytes >= 4 * 1024 * 1024)
        .ok_or("product metadata reservation leaves less than 4 MiB for native acquisition; raise buffer_bytes")?;
    let c = client()?;
    let mut signatures = routing::PlanetarySignatures::default();
    let mut assets = vec![];
    let mut metadata = vec![];
    let mut native_selection = crate::planner::NativeSelectionPolicy::default();
    let mut observation_context = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for (i, o) in catalog.observations.iter().enumerate() {
        if !ids.insert(&o.id) {
            return fail("duplicate product observation identity");
        }
        for requested in &r.bands {
            let b = p.band(requested)?;
            let a = &o.assets[&b.name];
            let id = format!("p{i:03}_{}", b.name);
            let href = if catalog.provider == routing::Provider::PlanetaryComputer
                && a.href.starts_with("https://")
            {
                signed_planetary(&c, &mut signatures, &r.product, &a.href)?
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
            if p.family == profile::ProductFamily::OperaDistAnn {
                if let Some(candidate) = dist_ann::extended_northern_candidate(o)? {
                    let observation = observation_context
                        .entry(o.id.clone())
                        .or_insert_with(|| std::sync::Arc::new(o.clone()))
                        .clone();
                    native_selection.admit_annual_dist_extended_north(
                        id.clone(),
                        candidate,
                        observation,
                        b.name.clone(),
                        a.clone(),
                    )?;
                }
            }
            if p.family == profile::ProductFamily::Hls
                && catalog.provider == routing::Provider::PlanetaryComputer
                && hls_pc_southern_tile(p.id, o, &b.name, a)?.is_some()
            {
                let observation = observation_context
                    .entry(o.id.clone())
                    .or_insert_with(|| std::sync::Arc::new(o.clone()))
                    .clone();
                native_selection.admit_hls_pc_southern(
                    id.clone(),
                    p.id.into(),
                    observation,
                    b.name.clone(),
                    a.clone(),
                )?;
            }
            metadata.push(json!({"asset_id":id,"provider":catalog.provider,"canonical_locator":crate::stac_workflow::safe_reference(&a.href),"access_policy":if catalog.provider == routing::Provider::PlanetaryComputer { routing::SIGNING_POLICY } else { routing::POLICY },"item_id":o.id,"collection":o.collection,"requested_band":b.name,"product":r.product,"product_version":p.processing_version(&o.id)?,"platform":o.platform,"processing":o.processing,"product_band":b,"acquisition":o.properties,"asset_metadata":a.metadata,"href":crate::stac_workflow::safe_reference(&a.href),"raster_bands":[{"scale":b.scale,"offset":b.offset,"unit":b.unit}],"quality_policies":r.quality_policies.iter().filter(|name|name.parse::<profile::QualityPolicy>().is_ok_and(|q|q.required_bands().contains(&b.name.as_str()))).collect::<Vec<_>>() }));
            if p.family == profile::ProductFamily::OperaDistAnn {
                let record = metadata.last_mut().expect("source record was appended");
                record["temporal_extent"] = serde_json::to_value(dist_ann::temporal_extent(o)?)?;
                record["time_coordinate_role"] = json!("source_coverage_start");
                record["temporal_selection"] =
                    json!("provider coverage interval intersects requested interval");
            }
            if p.family == profile::ProductFamily::OperaDswxNiPrototype {
                let record = metadata.last_mut().expect("source record was appended");
                record["prototype"] = json!(true);
                record["qualification_scope"] = json!(
                    "pinned v0.1 engineering specimen; not final NI v1.0 or a production archive"
                );
            }
        }
    }
    let request = Request {
        source: Source::Assets { assets },
        spatial: r.aoi.clone(),
        time: if p.family == profile::ProductFamily::OperaDistAnn {
            None
        } else {
            Some(Time::Interval {
                start: r.start.clone(),
                end: r.end.clone(),
            })
        },
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
                use crate::authenticated_http::AuthenticatedHttp;
                let session = match r.product.as_str() {
                    "hls-l30" | "hls-s30" => {
                        AuthenticatedHttp::earthdata_hls(&asset.locator, netrc)?
                    }
                    "opera-dswx-hls" => AuthenticatedHttp::earthdata_dswx(&asset.locator, netrc)?,
                    "opera-rtc-s1" => AuthenticatedHttp::earthdata_rtc(&asset.locator, netrc)?,
                    "opera-dswx-s1" => AuthenticatedHttp::earthdata_dswx_s1(&asset.locator, netrc)?,
                    "opera-dist-alert" => {
                        AuthenticatedHttp::earthdata_dist_alert(&asset.locator, netrc)?
                    }
                    "opera-dist-alert-s1" => {
                        AuthenticatedHttp::earthdata_dist_s1(&asset.locator, netrc)?
                    }
                    "opera-dist-ann-hls" => {
                        AuthenticatedHttp::earthdata_dist_ann(&asset.locator, netrc)?
                    }
                    _ => return fail("no qualified authenticated product COG route"),
                };
                sessions.insert(asset.id.clone(), std::sync::Arc::new(session));
            }
            acquisition::prepare_authenticated_cogs_with_native_selection(
                &request,
                cache,
                sessions,
                &native_selection,
            )?
        }
        #[cfg(not(all(feature = "native-cog", feature = "authenticated-http")))]
        {
            let _ = netrc;
            return fail(
                "authenticated product COG acquisition requires authenticated-http feature",
            );
        }
    } else if native_selection.is_empty() {
        acquisition::prepare(&request, cache)?
    } else {
        acquisition::prepare_with_native_selection(&request, cache, &native_selection)?
    };
    let physical_inspection = prepared.explain();
    let mut extended_north_provenance = BTreeMap::new();
    let mut last_date_provenance = BTreeMap::new();
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
        inspected_product_identity(&r.product, observation, band, asset, physical_source, l)?;
        if p.family == profile::ProductFamily::Hls
            && catalog.provider == routing::Provider::PlanetaryComputer
            && hls_pc_southern_tile(p.id, observation, &band.name, asset)?.is_some()
            && l.crs
                .strip_prefix("EPSG:")
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|epsg| (32601..=32660).contains(&epsg))
        {
            let zone =
                hls_pc_extended_northern_grid(p.id, observation, band, asset, physical_source, l)?;
            extended_north_provenance.insert(selected.asset_id.clone(), json!({
                "source_declared_crs":l.crs,"source_transform":l.transform,
                "mgrs_tile":observation.id.split('.').nth(2),
                "source_horizontal_cs_code":format!("EPSG:{}",32700+zone),
                "selection_mapping":"Planetary Computer HLS southern tile in source-declared northern UTM coordinates; false northing 0; no +south",
                "data_reprojection":false,"crs_relabeling":false,"coordinate_shift_m":0,
                "scope":"exact inspected HLS v2 object, native TIFF grid/tags and MGRS tile identity"}));
        }
        if p.family == profile::ProductFamily::OperaDistAnn {
            if let Some(provenance) =
                dist_ann::last_date_provenance(observation, band, physical_source)?
            {
                last_date_provenance.insert(selected.asset_id.clone(), provenance);
            }
        }
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
        let declared_nodata = asset.metadata["raster:bands"]
            .as_array()
            .and_then(|bands| bands.first())
            .and_then(|raster| raster.get("nodata"))
            .filter(|value| !value.is_null());
        let catalog_nodata = declared_nodata.and_then(Value::as_f64);
        let nodata_matches = if expected_nodata.is_some_and(f64::is_nan) {
            let actual = native_physical(physical_source)
                .and_then(|physical| physical["gdal_nodata_text"].as_str())
                .and_then(|text| text.parse::<f64>().ok());
            actual.is_some_and(f64::is_nan)
                && l.nodata.is_none_or(f64::is_nan)
                && declared_nodata.is_none_or(|value| {
                    value.as_f64().is_some_and(f64::is_nan)
                        || value
                            .as_str()
                            .and_then(|text| text.parse::<f64>().ok())
                            .is_some_and(f64::is_nan)
                })
        } else {
            match (expected_nodata, l.nodata, catalog_nodata) {
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
            }
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
        if p.family == profile::ProductFamily::OperaDistAnn {
            let candidate = dist_ann::extended_northern_candidate(observation)?;
            let extended = candidate.as_ref().is_some_and(|candidate| {
                candidate
                    .tile
                    .get(1..3)
                    .and_then(|zone| zone.parse::<u32>().ok())
                    .is_some_and(|zone| l.crs == format!("EPSG:{}", 32600 + zone))
            });
            if extended {
                let candidate = candidate.expect("extended annual candidate exists");
                dist_ann::extended_northern_grid(&candidate, l)?;
                extended_north_provenance.insert(selected.asset_id.clone(), json!({
                    "source_declared_crs":l.crs,
                    "source_transform":l.transform,
                    "mgrs_tile":candidate.tile,
                    "mgrs_latitude_band":candidate.tile.get(3..4),
                    "selection_mapping":"provider extended northern UTM coordinates (false northing 0; no +south)",
                    "data_reprojection":false,
                    "crs_relabeling":false,
                    "coordinate_shift_m":0,
                }));
            } else {
                wave2_grid_identity(observation, l)?;
            }
        } else if matches!(
            p.family,
            profile::ProductFamily::OperaDswxS1
                | profile::ProductFamily::OperaDist
                | profile::ProductFamily::OperaDswxNiPrototype
                | profile::ProductFamily::OperaDistS1
        ) {
            wave2_grid_identity(observation, l)?;
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
    let mut decision = catalog.routing;
    let mut verified = 0usize;
    for source in &mut metadata {
        if let Some(convention) =
            extended_north_provenance.get(source["asset_id"].as_str().unwrap_or(""))
        {
            source["native_coordinate_convention"] = convention.clone();
        }
        if let Some(provenance) =
            last_date_provenance.get(source["asset_id"].as_str().unwrap_or(""))
        {
            source["annual_last_date_temporal_provenance"] = provenance.clone();
        }
        let physical = physical_inspection["sources"]
            .as_array()
            .and_then(|sources| {
                sources
                    .iter()
                    .find(|candidate| candidate["asset_id"] == source["asset_id"])
            });
        source["equivalence"] = routing::attestation(
            &r.product,
            source["item_id"].as_str().unwrap_or(""),
            source["requested_band"].as_str().unwrap_or(""),
            source["canonical_locator"].as_str().unwrap_or(""),
            physical.and_then(|p| p["object_version"].as_str()),
        );
        if !source["equivalence"]["group"].is_null() {
            verified += 1;
        }
    }
    decision.equivalence = json!({"classification":if verified == metadata.len() && verified > 0 {"scientifically_equivalent_selected_samples"} else {"unknown_or_partial_across_providers"},"verified_physical_assets":verified,"total_assets":metadata.len(),"scope":"see each source's immutable ETag/locator-bound sample evidence; no whole-object byte identity or archive-wide interchangeability claim"});
    Ok(prepared.with_product_metadata(metadata,json!({"profile":p,"routing":decision,"quality_policies":r.quality_policies,"mask_scope":"derived masks belong to their raw QA/classification observation; no implicit cross-band alignment","discovery_seconds":catalog.seconds,"metadata_reservation_bytes":catalog.metadata_reservation_bytes,"requested_total_buffer_bytes":r.buffer_bytes,"acquisition_buffer_bytes":acquisition_buffer_bytes,"selection_time":Time::Interval{start:r.start.clone(),end:r.end.clone()},"temporal_interpretation":if p.family==profile::ProductFamily::OperaDistAnn {"annual coverage interval; coordinate labels actual provider coverage_start"}else{"source observation timestamp; additional source times retained in provenance"}})))
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
                crate::Pixels::Int16(v) => QualitySample::Int16(v[i]),
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

#[cfg(test)]
mod wave2_identity_tests {
    use super::*;

    #[cfg(feature = "native-cog")]
    fn pc_hls_southern_fixture() -> (Observation, ProductAsset, Value, crate::Layout) {
        // Captured metadata identities from the independently read July 13,
        // 2025 Ucayali specimen. No source values or bearer URLs are embedded.
        let id = "HLS.S30.T18MVS.2025194T151729.v2.0";
        let asset = ProductAsset {
            href: format!("https://hls2euwest.blob.core.windows.net/hls2/S30/18/M/VS/2025/07/13/{id}/{id}.B04.tif"),
            etag: Some("\"0x8DDC77CCDFA0AD4\"".into()), metadata: Value::Null,
        };
        let observation = Observation {
            id: id.into(),
            datetime: "2025-07-13T15:25:20.106080Z".into(),
            collection: "hls2-s30".into(),
            platform: Some("SENTINEL_2B".into()),
            processing: None,
            assets: BTreeMap::from([("B04".into(), asset.clone())]),
            properties: Value::Null,
        };
        let xml = r#"<GDALMetadata><Item name="HLS_PROCESSING_TIME">fixture processing timestamp</Item><Item name="SENSING_TIME">2025-07-13T15:25:20.106081Z</Item><Item name="PRODUCT_URI">S2B_MSIL1C_20250713T151729_N0511_R125_T18MVS_20250713T194831.SAFE</Item><Item name="TILE_ID">S2B_OPER_MSI_L1C_TL_2BPS_20250713T194831_A043623_T18MVS_N05.11</Item><Item name="HORIZONTAL_CS_CODE">EPSG:32718</Item><Item name="HORIZONTAL_CS_NAME">WGS84 / UTM zone 18S</Item><Item name="ULX">399960</Item><Item name="ULY">-799980</Item><Item name="NCOLS">3660</Item><Item name="NROWS">3660</Item><Item name="SPATIAL_RESOLUTION">30</Item></GDALMetadata>"#;
        let source = json!({"object_version":asset.etag,"object_identities":{"native_cog":{"normalized_crs":"EPSG:32618","gdal_metadata_xml":xml}}});
        let layout = crate::Layout {
            width: 3660,
            height: 3660,
            transform: [399960., 30., 0., -799980., 0., -30.],
            crs: "EPSG:32618".into(),
            variable: Variable::Band { index: 1 },
            block: [256, 256],
            dtype: "Int16".into(),
            nodata: Some(-9999.),
            scale: Some(0.0001),
            offset: Some(0.),
            tiles: vec![],
        };
        (observation, asset, source, layout)
    }

    #[cfg(feature = "native-cog")]
    #[test]
    fn pc_hls_southern_mapping_preserves_source_frame_and_independent_window() -> Result<()> {
        let (observation, asset, source, layout) = pc_hls_southern_fixture();
        let band = profile::profile("hls-s30")?.band("B04")?.clone();
        let zone = hls_pc_extended_northern_grid(
            "hls-s30",
            &observation,
            &band,
            &asset,
            &source,
            &layout,
        )?;
        assert_eq!(zone, 18);
        let aoi = Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [-75.075, -7.68, -74.985, -7.59],
        };
        // The GDAL-free projection layer enforces this hemisphere guard;
        // GDAL/PROJ itself accepts the source's extended northern coordinates.
        #[cfg(not(feature = "gdal"))]
        assert!(crate::spatial::native_mask_for_selection(
            &aoi,
            &layout,
            crate::spatial::NativeSelection::Standard
        )
        .is_err());
        let geometry = crate::spatial::native_mask_for_selection(
            &aoi,
            &layout,
            crate::spatial::NativeSelection::ProviderExtendedNorthernUtm { zone },
        )?
        .unwrap();
        let bounds = Spatial::Bounds {
            crs: layout.crs.clone(),
            bbox: crate::spatial::bounds(&geometry),
        };
        let window = crate::spatial::window(&layout, &bounds)?.unwrap();
        assert_eq!(
            [window.x, window.y, window.width, window.height],
            [3058, 1299, 332, 333]
        );
        let mask = crate::spatial::mask(&geometry, &layout, &window)?;
        assert_eq!(mask.data().iter().filter(|v| **v != 0).count(), 109561);
        assert_eq!(layout.crs, "EPSG:32618");
        assert_eq!(layout.transform[3], -799980.);
        Ok(())
    }

    #[cfg(feature = "native-cog")]
    #[test]
    fn pc_hls_southern_admission_rejects_source_tile_frame_and_tag_substitution() -> Result<()> {
        let (observation, asset, source, layout) = pc_hls_southern_fixture();
        let band = profile::profile("hls-s30")?.band("B04")?.clone();
        let check = |o: &Observation, a: &ProductAsset, s: &Value, l: &crate::Layout| {
            hls_pc_extended_northern_grid("hls-s30", o, &band, a, s, l)
        };
        for (field, from, to) in [
            ("gdal_metadata_xml", "EPSG:32718", "EPSG:32719"),
            ("gdal_metadata_xml", "399960", "499960"),
            ("gdal_metadata_xml", "T18MVS", "T18MVT"),
            ("normalized_crs", "EPSG:32618", "EPSG:32718"),
        ] {
            let mut wrong = source.clone();
            let value = wrong["object_identities"]["native_cog"][field]
                .as_str()
                .unwrap()
                .replace(from, to);
            wrong["object_identities"]["native_cog"][field] = json!(value);
            assert!(
                check(&observation, &asset, &wrong, &layout).is_err(),
                "{field} {from}->{to}"
            );
        }
        let mut wrong_source = source.clone();
        wrong_source["object_version"] = json!("\"other-object\"");
        assert!(check(&observation, &asset, &wrong_source, &layout).is_err());
        let mut wrong_asset = asset.clone();
        wrong_asset.href = wrong_asset.href.replace(".B04.tif", ".B03.tif");
        assert!(check(&observation, &wrong_asset, &source, &layout).is_err());
        let mut wrong = layout.clone();
        wrong.transform[3] += 10_000_000.;
        assert!(check(&observation, &asset, &source, &wrong).is_err());
        wrong = layout.clone();
        wrong.crs = "EPSG:32619".into();
        assert!(check(&observation, &asset, &source, &wrong).is_err());
        assert!(hls_pc_southern_tile("opera-dist-alert", &observation, "B04", &asset).is_err());
        let mut wrong_observation = observation.clone();
        wrong_observation.collection = "HLSS30_2.0".into();
        assert!(check(&wrong_observation, &asset, &source, &layout).is_err());
        Ok(())
    }

    #[cfg(feature = "native-cog")]
    #[test]
    fn conventional_wave2_products_do_not_inherit_the_annual_extended_frame() -> Result<()> {
        let request: ProductRequest = serde_json::from_str(include_str!(
            "../docs/products/wave2/dist-alert-request.json"
        ))?;
        let mut observation = request.observations.unwrap().remove(0);
        let mut tokens = observation
            .id
            .split('_')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        tokens[3] = "T20LRP".into();
        observation.id = tokens.join("_");
        let mut layout = crate::Layout {
            width: 3660,
            height: 3660,
            transform: [799980., 30., 0., -1099980., 0., -30.],
            crs: "EPSG:32620".into(),
            variable: Variable::Band { index: 1 },
            block: [256, 256],
            dtype: "Byte".into(),
            nodata: Some(255.),
            scale: None,
            offset: None,
            tiles: vec![],
        };
        assert!(wave2_grid_identity(&observation, &layout).is_err());
        // Explicit physical-frame admission is reserved for the annual gate.
        wave2_grid_identity_for_tile("T20LRP", &layout, true)?;
        layout.crs = "EPSG:32720".into();
        layout.transform[3] += 10_000_000.;
        wave2_grid_identity(&observation, &layout)?;
        Ok(())
    }

    #[test]
    fn annual_coverage_is_verified_against_catalog_not_caller_fields() -> Result<()> {
        let request: ProductRequest =
            serde_json::from_str(include_str!("../examples/opera-dist-ann-hls.json"))?;
        let observation = &request.observations.as_ref().unwrap()[0];
        let extent = dist_ann::temporal_extent(observation)?;
        let entry = json!({"producer_granule_id":observation.id,"id":"G-fixture-LPCLOUD","revision_id":7,
            "time_start":extent.coverage_start.to_rfc3339(),"time_end":extent.coverage_end.to_rfc3339(),
            "links":observation.assets.values().map(|asset|json!({"rel":"http://esipfed.org/ns/fedsearch/1.1/data#","href":asset.href})).collect::<Vec<_>>()});
        verify_annual_catalog_extent(observation, &entry)?;
        let mut fake = observation.clone();
        fake.properties["temporal_extent"]["coverage_end"] = json!("2025-12-31T23:59:59Z");
        assert!(verify_annual_catalog_extent(&fake, &entry).is_err());
        let mut missing_asset = entry.clone();
        missing_asset["links"] = json!([]);
        assert!(verify_annual_catalog_extent(observation, &missing_asset).is_err());
        let mut wrong_id = entry;
        wrong_id["producer_granule_id"] = json!("different");
        assert!(verify_annual_catalog_extent(observation, &wrong_id).is_err());
        Ok(())
    }
    #[test]
    fn dist_physical_metadata_cannot_swap_status_confidence_or_date_layers() -> Result<()> {
        let request: ProductRequest = serde_json::from_str(include_str!(
            "../docs/products/wave2/dist-alert-request.json"
        ))?;
        let inspection: Value = serde_json::from_str(include_str!(
            "../docs/products/wave2/dist-alert-inspection.json"
        ))?;
        let observation = &request.observations.as_ref().unwrap()[0];
        let p = profile::profile(&request.product)?;
        let assets = inspection["assets"].as_array().unwrap();
        // Real metadata from independently inspected source TIFFs. Dtype/fill
        // alone cannot distinguish either status family or score from a date.
        for name in [
            "VEG-DIST-STATUS",
            "GEN-DIST-STATUS",
            "VEG-DIST-CONF",
            "GEN-DIST-CONF",
            "VEG-DIST-DATE",
            "GEN-DIST-DATE",
            "VEG-LAST-DATE",
            "GEN-LAST-DATE",
            "DATA-MASK",
        ] {
            let band = p.band(name)?;
            let asset = &observation.assets[name];
            let correct = assets.iter().find(|a| a["name"] == name).unwrap();
            physical_identity(
                p.id,
                observation,
                band,
                asset,
                &json!({"physical_metadata":correct["physical"]}),
            )?;
            for wrong in assets.iter().filter(|a| a["name"] != name) {
                assert!(
                    physical_identity(
                        p.id,
                        observation,
                        band,
                        asset,
                        &json!({"physical_metadata":wrong["physical"]})
                    )
                    .is_err(),
                    "{name} must reject {}",
                    wrong["name"]
                );
            }
        }
        Ok(())
    }
    #[test]
    fn wave2_actual_shape_zone_hemisphere_and_grid_corner_are_bound_to_tile() -> Result<()> {
        for (name, origin) in [
            ("dswx-s1", [699960., 4000020.]),
            ("dist-alert", [600000., 3900000.]),
        ] {
            let request: ProductRequest = serde_json::from_str(if name == "dswx-s1" {
                include_str!("../docs/products/wave2/dswx-s1-request.json")
            } else {
                include_str!("../docs/products/wave2/dist-alert-request.json")
            })?;
            let o = &request.observations.as_ref().unwrap()[0];
            let l = crate::Layout {
                width: 3660,
                height: 3660,
                transform: [origin[0], 30., 0., origin[1], 0., -30.],
                crs: "EPSG:32617".into(),
                variable: Variable::Band { index: 1 },
                block: [512, 512],
                dtype: "UInt8".into(),
                nodata: Some(255.),
                scale: None,
                offset: None,
                tiles: vec![],
            };
            wave2_grid_identity(o, &l)?;
            let mut wrong = l.clone();
            wrong.crs = "EPSG:32618".into();
            assert!(wave2_grid_identity(o, &wrong).is_err());
            wrong = l.clone();
            wrong.crs = "EPSG:32717".into();
            assert!(wave2_grid_identity(o, &wrong).is_err());
            wrong = l.clone();
            wrong.width = 100;
            assert!(wave2_grid_identity(o, &wrong).is_err());
            wrong = l.clone();
            wrong.transform[0] += 100_000.;
            assert!(wave2_grid_identity(o, &wrong).is_err());
            wrong = l.clone();
            wrong.transform[3] += 100_000.;
            assert!(wave2_grid_identity(o, &wrong).is_err());
            #[cfg(feature = "native-cog")]
            for shift in [-2_000_000., 2_000_000.] {
                wrong = l.clone();
                wrong.transform[3] += shift;
                assert!(wave2_grid_identity(o, &wrong).is_err());
            }
        }
        Ok(())
    }
}
