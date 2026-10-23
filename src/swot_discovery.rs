//! Narrow, anonymous, bounded NASA CMR discovery for SWOT Raster D.
//!
//! Catalog relevance, an archive locator, a filename/metadata family match,
//! native scientific inspection and an admitted index selection are different
//! claims. Discovery never opens an archive object or reads credentials.
use crate::{fail, Result};
use chrono::{DateTime, FixedOffset};
use reqwest::{blocking::Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    time::{Duration, Instant},
};

pub const COLLECTION: &str = "SWOT_L2_HR_Raster_D";
pub const CMR: &str = "https://cmr.earthdata.nasa.gov/search/";
pub const ARCHIVE: &str = "https://archive.swot.podaac.earthdata.nasa.gov/podaac-swot-ops-cumulus-protected/SWOT_L2_HR_Raster_D/";
pub const MAX_REPORT_BYTES: usize = 4 * 1024 * 1024;
#[cfg(feature = "native-hdf5")]
pub mod bridge;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub page_size: usize,
    pub max_pages: usize,
    pub max_records: usize,
    pub max_response_bytes: usize,
    pub max_total_bytes: usize,
    pub timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    pub retries: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            page_size: 20,
            max_pages: 10,
            max_records: 400,
            max_response_bytes: 1024 * 1024,
            max_total_bytes: 8 * 1024 * 1024,
            timeout_seconds: 120,
            request_timeout_seconds: 15,
            retries: 1,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub collection: String,
    /// Optional assertion, checked against freshly resolved collection metadata.
    pub collection_concept_id: Option<String>,
    pub bbox: [f64; 4],
    pub start: String,
    pub end: String,
    pub limit: usize,
    #[serde(default)]
    pub limits: Limits,
}
pub(crate) fn datetime(s: &str) -> Result<DateTime<FixedOffset>> {
    if s.len() > 64 {
        return fail("timestamp exceeds bound");
    }
    DateTime::parse_from_rfc3339(s).map_err(|_| "explicit RFC3339 timestamp required".into())
}
fn concept(s: &str, prefix: char) -> bool {
    s.strip_prefix(prefix)
        .and_then(|s| s.strip_suffix("-POCLOUD"))
        .is_some_and(|n| !n.is_empty() && n.len() <= 20 && n.bytes().all(|b| b.is_ascii_digit()))
}
impl Query {
    pub fn validate(&self) -> Result<()> {
        let [w, s, e, n] = self.bbox;
        if self.collection != COLLECTION
            || self
                .collection_concept_id
                .as_ref()
                .is_some_and(|s| !concept(s, 'C'))
        {
            return fail(
                "only explicit SWOT_L2_HR_Raster_D / POCLOUD collection identity is supported",
            );
        }
        if self.bbox.iter().any(|v| !v.is_finite())
            || w < -180.
            || e > 180.
            || s < -90.
            || n > 90.
            || w >= e
            || s >= n
            || e - w > 30.
            || n - s > 30.
            || (e - w) * (n - s) > 400.
        {
            return fail("discovery refused: WGS84 bbox must be ordered, at most 30 degrees per axis and 400 square degrees; split antimeridian searches explicitly");
        }
        let span = datetime(&self.end)? - datetime(&self.start)?;
        if span <= chrono::Duration::zero() || span > chrono::Duration::days(31) {
            return fail(
                "discovery refused: explicit positive interval of at most 31 days required",
            );
        }
        let l = &self.limits;
        if !(1..=100).contains(&self.limit)
            || !(1..=100).contains(&l.page_size)
            || !(1..=10).contains(&l.max_pages)
            || !(1..=1000).contains(&l.max_records)
            || !(1024..=1024 * 1024).contains(&l.max_response_bytes)
            || l.max_total_bytes < l.max_response_bytes
            || l.max_total_bytes > 8 * 1024 * 1024
            || !(1..=120).contains(&l.timeout_seconds)
            || !(1..=30).contains(&l.request_timeout_seconds)
            || l.retries > 2
        {
            return fail("discovery refused: limits exceed bounded CMR envelope; candidates 1..100, pages <=10, records <=1000, page <=1 MiB, total <=8 MiB, elapsed <=120 s, retries <=2");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionIdentity {
    pub concept_id: String,
    pub revision: u64,
    pub short_name: String,
    pub version: String,
    pub provider: String,
}
/// Catalog assertions retained alongside native source provenance. Catalog
/// times/size are reported values, never a substitute for source version checks.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProvenance {
    pub collection: CollectionIdentity,
    pub concept_id: String,
    pub revision: u64,
    pub granule_ur: String,
    pub acquisition_start: String,
    pub acquisition_end: String,
    pub processing_release: String,
    pub product_counter: String,
    pub pge_version: Option<String>,
    pub source_size_bytes: Option<u64>,
}
impl CatalogProvenance {
    pub fn validate(&self, asset: &crate::Asset) -> Result<ProductIdentity> {
        let p = ProductIdentity::parse(&self.granule_ur)?;
        if !concept(&self.collection.concept_id, 'C')
            || self.collection.short_name != COLLECTION
            || self.collection.version != "D"
            || self.collection.provider != "POCLOUD"
            || !concept(&self.concept_id, 'G')
            || self.revision == 0
            || self.collection.revision == 0
            || asset.locator != format!("{ARCHIVE}{}", p.filename)
            || asset.id != self.concept_id
            || asset.datetime.as_deref() != Some(&self.acquisition_start)
            || self.processing_release != "PGD0"
            || self.processing_release != p.processing_release
            || self.product_counter != p.product_counter
            || datetime(&self.acquisition_start)? > datetime(&self.acquisition_end)?
        {
            return fail(
                "catalog provenance conflicts with explicit canonical SWOT source identity",
            );
        }
        for (reported, filename) in [
            (&self.acquisition_start, &p.filename_start),
            (&self.acquisition_end, &p.filename_end),
        ] {
            if (datetime(reported)? - datetime(filename)?)
                .num_milliseconds()
                .abs()
                > 1000
            {
                return fail("catalog acquisition time conflicts with canonical product identity");
            }
        }
        Ok(p)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductIdentity {
    pub filename: String,
    pub descriptor: String,
    pub resolution_m: u32,
    pub cycle: u32,
    pub pass: u32,
    pub scene: u32,
    pub filename_start: String,
    pub filename_end: String,
    pub processing_release: String,
    pub product_counter: String,
}
impl ProductIdentity {
    pub fn parse(name: &str) -> Result<Self> {
        if name.len() > 240 || !name.is_ascii() {
            return fail("invalid bounded SWOT product identity");
        }
        let stem = name.strip_suffix(".nc").unwrap_or(name);
        let p: Vec<_> = stem.split('_').collect();
        if p.len() != 17
            || p[..4] != ["SWOT", "L2", "HR", "Raster"]
            || !matches!(p[4], "100m" | "250m")
            || !p[5].starts_with("UTM")
            || !matches!(p[6], "N" | "S")
            || p[7..10] != ["x", "x", "x"]
            || p[10].len() != 3
            || p[11].len() != 3
            || p[12].len() != 4
            || !p[12].ends_with('F')
            || p[15].len() != 4
            || p[16].len() != 2
            || !p[15].bytes().all(|b| b.is_ascii_alphanumeric())
            || !p[16].bytes().all(|b| b.is_ascii_digit())
        {
            return fail("ambiguous or unsupported SWOT product name");
        }
        let utm = &p[5][3..];
        if utm.len() != 3
            || !utm[..2].parse::<u32>().is_ok_and(|n| (1..=60).contains(&n))
            || !"CDEFGHJKLMNPQRSTUVWX".contains(&utm[2..])
        {
            return fail("invalid UTM product descriptor");
        }
        let time = |v: &str| -> Result<String> {
            chrono::NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
                .map(|d| d.and_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .map_err(|_| "invalid product timestamp".into())
        };
        let start = time(p[13])?;
        let end = time(p[14])?;
        if datetime(&end)? < datetime(&start)? {
            return fail("reversed product interval");
        }
        Ok(Self {
            filename: format!("{stem}.nc"),
            descriptor: p[4..10].join("_"),
            resolution_m: p[4][..3].parse()?,
            cycle: p[10].parse()?,
            pass: p[11].parse()?,
            scene: p[12][..3].parse()?,
            filename_start: start,
            filename_end: end,
            processing_release: p[15].into(),
            product_counter: p[16].into(),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    /// Reported WGS84 rectangles; may cross the antimeridian.
    pub bounding_rectangles: Vec<[f64; 4]>,
    /// Reported outer polygon boundaries in longitude/latitude order.
    pub polygons: Vec<Vec<[f64; 2]>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessStatus {
    SupportedLocator,
    Missing,
    Ambiguous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FamilyStatus {
    AppearsRasterDPgd0,
    Unsupported,
    Ambiguous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectionStatus {
    NotInspected,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogRelevance {
    ReportedByCmr,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSelectionStatus {
    Unresolved,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub catalog_relevance: CatalogRelevance,
    pub native_selection: NativeSelectionStatus,
    pub concept_id: String,
    pub revision: u64,
    pub granule_ur: Option<String>,
    pub product: Option<ProductIdentity>,
    pub acquisition_start: Option<String>,
    pub acquisition_end: Option<String>,
    pub collection_version: Option<String>,
    pub pge_version: Option<String>,
    pub coverage: Option<Coverage>,
    pub source_size_bytes: Option<u64>,
    pub source_url: Option<String>,
    pub data_access: AccessStatus,
    pub processing_family: FamilyStatus,
    pub scientific_profile: InspectionStatus,
    pub exclusions: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    Complete,
    Truncated,
    Failed,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statistics {
    pub http_requests: usize,
    pub response_body_bytes: usize,
    pub pages: usize,
    pub records_inspected: usize,
    pub duplicates: usize,
    pub rejected_unidentified_records: usize,
    pub elapsed_seconds: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub discovery_schema: u32,
    pub query: Query,
    pub collection: Option<CollectionIdentity>,
    pub status: Completion,
    pub reasons: Vec<String>,
    pub catalog_hits: Option<u64>,
    pub candidates: Vec<Candidate>,
    pub statistics: Statistics,
    pub scope: String,
}
impl Report {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_REPORT_BYTES {
            return fail("discovery report exceeds 4 MiB");
        }
        let report: Self = serde_json::from_slice(bytes).map_err(|_| "invalid discovery report")?;
        report.query.validate()?;
        if report.discovery_schema != 1 || report.candidates.len() > report.query.limit {
            return fail("invalid bounded discovery report");
        }
        Ok(report)
    }
}
pub struct CmrClient {
    client: Client,
    endpoint: Url,
}
struct Page {
    body: Value,
    after: Option<String>,
    hits: u64,
}
impl CmrClient {
    pub fn new() -> Result<Self> {
        Self::at(CMR)
    }
    /// Explicit loopback-only transport for frozen catalog fixtures. Archive
    /// locators are still restricted to the exact NASA product family.
    pub fn loopback_fixture(endpoint: &str) -> Result<Self> {
        let u = Url::parse(endpoint).map_err(|_| "invalid fixture endpoint")?;
        if u.scheme() != "http"
            || !matches!(u.host_str(), Some("127.0.0.1" | "[::1]"))
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
            || !u.path().ends_with('/')
        {
            return fail(
                "CMR fixtures require an explicit numeric loopback HTTP base URL ending in /",
            );
        }
        Self::at(endpoint)
    }
    fn at(endpoint: &str) -> Result<Self> {
        Ok(Self {
            endpoint: Url::parse(endpoint)?,
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(5))
                .no_proxy()
                .user_agent("tile-ripper/0.1 bounded-swot-discovery")
                .build()?,
        })
    }
    fn page(
        &self,
        route: &str,
        params: &[(String, String)],
        after: Option<&str>,
        q: &Query,
        stats: &mut Statistics,
        deadline: Instant,
    ) -> Result<Page> {
        let l = &q.limits;
        for attempt in 0..=l.retries {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("catalog query deadline exceeded")?;
            if remaining.is_zero() || stats.response_body_bytes >= l.max_total_bytes {
                return fail("catalog total byte/time limit exceeded");
            }
            let mut r = self
                .client
                .get(self.endpoint.join(route)?)
                .query(params)
                .header("Client-Id", "tile-ripper-bounded-swot-discovery")
                .header("Accept", "application/vnd.nasa.cmr.umm_results+json")
                .header("Accept-Encoding", "identity")
                .timeout(remaining.min(Duration::from_secs(l.request_timeout_seconds)));
            if let Some(a) = after {
                r = r.header("CMR-Search-After", a);
            }
            stats.http_requests += 1;
            let response = r.send();
            let mut response = match response {
                Ok(r) => r,
                Err(_) if attempt < l.retries => continue,
                Err(_) => {
                    return fail(
                        "catalog request failed or timed out (no archive access attempted)",
                    )
                }
            };
            let status = response.status();
            let cap = l
                .max_response_bytes
                .min(l.max_total_bytes - stats.response_body_bytes);
            let after = response
                .headers()
                .get("CMR-Search-After")
                .map(|h| h.to_str().map(str::to_owned))
                .transpose()
                .map_err(|_| "invalid catalog pagination header")?;
            if after.as_ref().is_some_and(|s| s.len() > 4096) {
                return fail("catalog pagination token exceeds bound");
            }
            let timed_out = response
                .headers()
                .get("CMR-Timed-Out")
                .is_some_and(|h| h.as_bytes() != b"false");
            let mut bytes = Vec::new();
            let read = response
                .by_ref()
                .take(cap as u64 + 1)
                .read_to_end(&mut bytes);
            stats.response_body_bytes += bytes.len();
            if bytes.len() > cap {
                return fail("catalog response/total byte limit exceeded (one sentinel byte may be observed)");
            }
            if read.is_err() {
                if attempt < l.retries {
                    continue;
                }
                return fail("catalog body failed or timed out");
            }
            if !status.is_success() {
                // No response body, URL, or server-provided error text is echoed.
                if attempt < l.retries && (status.as_u16() == 429 || status.is_server_error()) {
                    // Respect rate limiting by ending this run; no immediate 429 retry.
                    if status.as_u16() == 429 {
                        return fail("catalog rate limited; retry later");
                    }
                    continue;
                }
                return Err(format!("catalog HTTP {} refused", status.as_u16()).into());
            }
            if timed_out {
                return fail("CMR reported a partial timed-out search");
            }
            let body: Value =
                serde_json::from_slice(&bytes).map_err(|_| "invalid bounded catalog JSON")?;
            metadata_bound(&body)?;
            let hits = body["hits"].as_u64().ok_or("missing catalog hit count")?;
            return Ok(Page { body, after, hits });
        }
        unreachable!()
    }
    /// Serial metadata-only search. Partial results always carry an explicit
    /// truncated/failed status. Search-After is not an atomic catalog snapshot.
    pub fn discover(&self, q: &Query) -> Result<Report> {
        q.validate()?;
        let started = Instant::now();
        let deadline = started + Duration::from_secs(q.limits.timeout_seconds);
        let mut report=Report { discovery_schema:1, query:q.clone(), collection:None,
            status:Completion::Complete,reasons:vec![],catalog_hits:None,candidates:vec![],statistics:Statistics::default(),
            scope:"CMR reports geographic/temporal relevance only; approximate footprints do not establish native-grid intersection or useful river WSE. No archive object or credentials read. Source inspection and explicit native indices required; per-page Search-After queries are not an atomic catalog snapshot.".into() };
        let result = (|| -> Result<()> {
            let params = vec![
                ("short_name".into(), COLLECTION.into()),
                ("version".into(), "D".into()),
                ("provider".into(), "POCLOUD".into()),
                ("page_size".into(), "2".into()),
            ];
            let page = self.page(
                "collections.umm_json",
                &params,
                None,
                q,
                &mut report.statistics,
                deadline,
            )?;
            let items = page.body["items"]
                .as_array()
                .ok_or("missing collection metadata")?;
            if page.hits != 1 || items.len() != 1 {
                return fail(
                    "collection resolution is missing or ambiguous; no granule query made",
                );
            }
            let i = &items[0];
            let id = text_at(i, "/meta/concept-id").ok_or("missing collection concept ID")?;
            if !concept(id, 'C')
                || i["umm"]["ShortName"] != COLLECTION
                || i["umm"]["Version"] != "D"
                || i["meta"]["provider-id"] != "POCLOUD"
                || i["meta"]["deleted"] == true
                || q.collection_concept_id.as_ref().is_some_and(|s| s != id)
            {
                return fail(
                    "resolved collection conflicts with explicit Raster D / POCLOUD identity",
                );
            }
            report.collection = Some(CollectionIdentity {
                concept_id: id.into(),
                revision: i["meta"]["revision-id"]
                    .as_u64()
                    .ok_or("missing collection revision")?,
                short_name: COLLECTION.into(),
                version: "D".into(),
                provider: "POCLOUD".into(),
            });
            let mut records = BTreeMap::<String, Candidate>::new();
            let mut cursor = None;
            let mut cursors = BTreeSet::new();
            loop {
                if report.statistics.pages >= q.limits.max_pages
                    || report.statistics.records_inspected >= q.limits.max_records
                {
                    report.status = Completion::Truncated;
                    report
                        .reasons
                        .push("page or inspected-record limit reached".into());
                    break;
                }
                let size = q
                    .limits
                    .page_size
                    .min(q.limits.max_records - report.statistics.records_inspected);
                let params = vec![
                    ("collection_concept_id".into(), id.into()),
                    (
                        "bounding_box".into(),
                        q.bbox.map(|v| v.to_string()).join(","),
                    ),
                    ("temporal".into(), format!("{},{}", q.start, q.end)),
                    ("page_size".into(), size.to_string()),
                    ("sort_key[]".into(), "start_date".into()),
                    ("sort_key[]".into(), "producer_granule_id".into()),
                ];
                let page = self.page(
                    "granules.umm_json",
                    &params,
                    cursor.as_deref(),
                    q,
                    &mut report.statistics,
                    deadline,
                )?;
                report.statistics.pages += 1;
                if report.catalog_hits.is_some_and(|n| n != page.hits) {
                    report.status = Completion::Truncated;
                    report
                        .reasons
                        .push("catalog hit count changed during pagination".into());
                }
                report.catalog_hits = Some(page.hits);
                let items = page.body["items"]
                    .as_array()
                    .ok_or("missing granule items")?;
                if items.len() > size {
                    return fail("catalog exceeded requested page size");
                }
                for item in items {
                    report.statistics.records_inspected += 1;
                    let Some(key) = text_at(item, "/meta/concept-id").filter(|s| concept(s, 'G'))
                    else {
                        report.statistics.rejected_unidentified_records += 1;
                        continue;
                    };
                    let candidate = candidate(item, id, q);
                    if let Some(old) = records.get_mut(key) {
                        report.statistics.duplicates += 1;
                        if *old != candidate {
                            // Keep a deterministic representative, refuse its use.
                            let a = serde_json::to_string(&old)?;
                            let b = serde_json::to_string(&candidate)?;
                            if b < a {
                                *old = candidate;
                            }
                            old.exclusions
                                .push("conflicting records for stable CMR granule identity".into());
                        }
                    } else {
                        records.insert(key.into(), candidate);
                    }
                }
                report.candidates = records.values().cloned().collect();
                if records.len() >= q.limit
                    && (page.hits > report.statistics.records_inspected as u64
                        || records.len() > q.limit)
                {
                    report.status = Completion::Truncated;
                    report.reasons.push("candidate limit reached; explicit selection is still required (job maximum four)".into());
                    break;
                }
                if items.is_empty() || items.len() < size {
                    if page.hits > report.statistics.records_inspected as u64 {
                        report.status = Completion::Truncated;
                        report
                            .reasons
                            .push("catalog ended before its reported hit count".into());
                    }
                    break;
                }
                match page.after {
                    Some(next) if cursors.insert(next.clone()) => cursor = Some(next),
                    Some(_) => return fail("catalog repeated its Search-After cursor"),
                    None if page.hits <= report.statistics.records_inspected as u64 => break,
                    None => return fail("catalog omitted required Search-After pagination cursor"),
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            report.status = Completion::Failed;
            report.reasons.push(error.to_string());
        }
        let mut products = BTreeMap::<String, usize>::new();
        for c in &report.candidates {
            if let Some(p) = &c.product {
                *products.entry(p.filename.clone()).or_default() += 1;
            }
        }
        for c in &mut report.candidates {
            if c.product
                .as_ref()
                .is_some_and(|p| products[&p.filename] > 1)
            {
                c.exclusions.push("same product under multiple CMR identities; explicit conflict resolution required".into());
            }
            c.exclusions.sort();
            c.exclusions.dedup();
        }
        report.candidates.sort_by(|a, b| {
            let instant = |c: &Candidate| {
                c.acquisition_start
                    .as_deref()
                    .and_then(|s| datetime(s).ok())
            };
            (instant(a), &a.granule_ur, &a.concept_id).cmp(&(
                instant(b),
                &b.granule_ur,
                &b.concept_id,
            ))
        });
        report.candidates.truncate(q.limit);
        report.statistics.elapsed_seconds = started.elapsed().as_secs_f64();
        Ok(report)
    }
}
fn text_at<'a>(v: &'a Value, p: &str) -> Option<&'a str> {
    v.pointer(p)?
        .as_str()
        .filter(|s| s.len() <= 512 && !s.chars().any(char::is_control))
}
fn metadata_bound(v: &Value) -> Result<()> {
    fn count(v: &Value, n: &mut usize) -> bool {
        *n += 1;
        if *n > 100_000 {
            return false;
        }
        match v {
            Value::String(s) => s.len() <= 16 * 1024,
            Value::Array(a) => a.iter().all(|v| count(v, n)),
            Value::Object(o) => o.iter().all(|(k, v)| k.len() <= 512 && count(v, n)),
            _ => true,
        }
    }
    if !count(v, &mut 0) {
        return fail("catalog parsed metadata exceeds 100000 nodes or 16 KiB string bound");
    }
    Ok(())
}
fn coverage(v: &Value) -> Result<Coverage> {
    let g = &v["SpatialExtent"]["HorizontalSpatialDomain"]["Geometry"];
    let mut result = Coverage {
        bounding_rectangles: vec![],
        polygons: vec![],
    };
    if let Some(a) = g["BoundingRectangles"].as_array() {
        if a.len() > 16 {
            return fail("too many catalog rectangles");
        }
        for b in a {
            let mut bbox = [0.; 4];
            for (out, key) in bbox.iter_mut().zip([
                "WestBoundingCoordinate",
                "SouthBoundingCoordinate",
                "EastBoundingCoordinate",
                "NorthBoundingCoordinate",
            ]) {
                *out = b[key].as_f64().ok_or("invalid catalog rectangle")?;
            }
            if bbox.iter().any(|n| !n.is_finite())
                || bbox[0].abs() > 180.
                || bbox[2].abs() > 180.
                || bbox[1] < -90.
                || bbox[3] > 90.
                || bbox[1] > bbox[3]
            {
                return fail("invalid catalog rectangle");
            }
            result.bounding_rectangles.push(bbox);
        }
    }
    if let Some(a) = g["GPolygons"].as_array() {
        if a.len() > 16 {
            return fail("too many catalog polygons");
        }
        let mut n = 0;
        for p in a {
            let points = p["Boundary"]["Points"]
                .as_array()
                .ok_or("invalid catalog polygon")?;
            n += points.len();
            if n > 256 || points.len() < 4 {
                return fail("catalog polygon point bound");
            }
            let mut polygon = vec![];
            for point in points {
                let x = point["Longitude"]
                    .as_f64()
                    .ok_or("invalid catalog longitude")?;
                let y = point["Latitude"]
                    .as_f64()
                    .ok_or("invalid catalog latitude")?;
                if !x.is_finite() || !y.is_finite() || x.abs() > 180. || y.abs() > 90. {
                    return fail("invalid catalog point");
                }
                polygon.push([x, y]);
            }
            if polygon.first() != polygon.last() {
                return fail("unclosed catalog polygon");
            }
            result.polygons.push(polygon);
        }
    }
    if result.bounding_rectangles.is_empty() && result.polygons.is_empty() {
        return fail("missing supported catalog footprint");
    }
    Ok(result)
}
fn candidate(item: &Value, collection_id: &str, q: &Query) -> Candidate {
    let u = &item["umm"];
    let mut c = Candidate {
        catalog_relevance: CatalogRelevance::ReportedByCmr,
        native_selection: NativeSelectionStatus::Unresolved,
        concept_id: text_at(item, "/meta/concept-id").unwrap_or("").into(),
        revision: item["meta"]["revision-id"].as_u64().unwrap_or(0),
        granule_ur: text_at(u, "/GranuleUR")
            .filter(|s| {
                s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
            })
            .map(str::to_owned),
        product: None,
        acquisition_start: text_at(u, "/TemporalExtent/RangeDateTime/BeginningDateTime")
            .filter(|s| datetime(s).is_ok())
            .map(str::to_owned),
        acquisition_end: text_at(u, "/TemporalExtent/RangeDateTime/EndingDateTime")
            .filter(|s| datetime(s).is_ok())
            .map(str::to_owned),
        collection_version: text_at(u, "/CollectionReference/Version")
            .filter(|s| s.len() <= 16 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.'))
            .map(str::to_owned),
        pge_version: text_at(u, "/PGEVersionClass/PGEVersion")
            .filter(|s| s.len() <= 32 && s.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
            .map(str::to_owned),
        coverage: None,
        source_size_bytes: None,
        source_url: None,
        data_access: AccessStatus::Missing,
        processing_family: FamilyStatus::Ambiguous,
        scientific_profile: InspectionStatus::NotInspected,
        exclusions: vec![],
    };
    if item["meta"]["collection-concept-id"] != collection_id
        || item["meta"]["provider-id"] != "POCLOUD"
        || item["meta"]["concept-type"] != "granule"
        || item["meta"]["deleted"] == true
        || u["CollectionReference"]["ShortName"] != COLLECTION
        || c.collection_version.as_deref() != Some("D")
        || c.revision == 0
    {
        c.exclusions
            .push("missing or conflicting collection/version/revision identity".into());
    }
    let dates = (|| -> Result<()> {
        let a = datetime(c.acquisition_start.as_deref().ok_or("missing start")?)?;
        let b = datetime(c.acquisition_end.as_deref().ok_or("missing end")?)?;
        if a > b || a > datetime(&q.end)? || b < datetime(&q.start)? {
            return fail("catalog interval is invalid or outside query");
        }
        Ok(())
    })();
    if dates.is_err() {
        c.exclusions
            .push("missing, invalid or irrelevant catalog interval".into());
    }
    match coverage(u) {
        Ok(g) => c.coverage = Some(g),
        Err(_) => c
            .exclusions
            .push("missing, invalid or oversized catalog footprint".into()),
    }
    let product = c
        .granule_ur
        .as_deref()
        .ok_or("missing granule UR")
        .and_then(|s| ProductIdentity::parse(s).map_err(|_| "invalid product identity"));
    match product {
        Ok(p) => {
            for (reported, filename) in [
                (&c.acquisition_start, &p.filename_start),
                (&c.acquisition_end, &p.filename_end),
            ] {
                if reported
                    .as_deref()
                    .and_then(|s| datetime(s).ok())
                    .is_some_and(|t| {
                        (t - datetime(filename).expect("parsed product time"))
                            .num_milliseconds()
                            .abs()
                            > 1000
                    })
                {
                    c.exclusions
                        .push("catalog time conflicts with canonical product identity".into());
                }
            }
            c.processing_family = if p.processing_release == "PGD0" {
                FamilyStatus::AppearsRasterDPgd0
            } else {
                FamilyStatus::Unsupported
            };
            if c.processing_family == FamilyStatus::Unsupported {
                c.exclusions
                    .push("unsupported processing release (only PGD0 is qualified)".into());
            }
            let identifiers = u["DataGranule"]["Identifiers"].as_array();
            let get = |kind: &str| -> Vec<&str> {
                identifiers
                    .into_iter()
                    .flatten()
                    .filter(|i| i["IdentifierType"] == kind)
                    .filter_map(|i| i["Identifier"].as_str())
                    .collect()
            };
            if get("ProducerGranuleId") != [p.filename.as_str()]
                || get("CRID") != [p.processing_release.as_str()]
            {
                c.processing_family = FamilyStatus::Ambiguous;
                c.exclusions
                    .push("ambiguous or conflicting producer/processing identity".into());
            }
            let mut urls = BTreeSet::new();
            let mut invalid = false;
            for link in u["RelatedUrls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|l| l["Type"] == "GET DATA")
            {
                if let Some(url) = link["URL"].as_str() {
                    if url == format!("{ARCHIVE}{}", p.filename) {
                        urls.insert(url.to_owned());
                    } else {
                        invalid = true;
                    }
                } else {
                    invalid = true;
                }
            }
            if invalid || urls.len() > 1 {
                c.data_access = AccessStatus::Ambiguous;
                c.exclusions.push("unsupported, unsigned-locator conflict or ambiguous GET DATA links; raw URLs omitted".into());
            } else if let Some(url) = urls.into_iter().next() {
                c.source_url = Some(url);
                c.data_access = AccessStatus::SupportedLocator;
            } else {
                c.exclusions
                    .push("no supported authoritative unsigned archive GET DATA locator".into());
            }
            let sizes: BTreeSet<_> = u["DataGranule"]["ArchiveAndDistributionInformation"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|i| i["Name"] == p.filename)
                .filter_map(|i| i["SizeInBytes"].as_u64())
                .collect();
            if sizes.len() == 1 {
                c.source_size_bytes = sizes.into_iter().next();
                if c.source_size_bytes == Some(0) {
                    c.exclusions.push("invalid zero catalog source size".into());
                }
            } else if sizes.len() > 1 {
                c.exclusions
                    .push("conflicting authoritative source sizes".into());
            }
            c.product = Some(p);
        }
        Err(_) => c
            .exclusions
            .push("missing, invalid or ambiguous canonical product identity".into()),
    }
    c
}
