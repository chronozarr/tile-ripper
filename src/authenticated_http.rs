//! Explicit, exact-object HTTPS sessions. No process-wide auth or URL logging.
//!
//! Redirect rules are directed edges with exact destination paths. Credentials
//! are loaded from a private netrc file and used only at the login origin.
use crate::{fail, Result};
use reqwest::{
    cookie::{CookieStore, Jar},
    header::{self, HeaderMap, HeaderValue},
    Client, Method, Url,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_HOPS: usize = 8;
const SESSION_BYTES: usize = 64 * 1024;
/// Charged in addition to format/cache admission; excludes the HTTP runtime and
/// TLS/allocator overhead, which are measured through process RSS.
pub const WORKING_RESERVATION: usize = 1024 * 1024;

/// One explicitly approved redirect, including same-origin redirects.
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedirectRule {
    pub from_origin: String,
    pub to_origin: String,
    pub destination_path: String,
}
/// Public, nonsecret access policy for exactly one canonical object.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub canonical_url: String,
    pub login_origin: String,
    pub resource_origin: String,
    pub head_resource_path: String,
    pub get_resource_path: String,
    pub redirects: Vec<RedirectRule>,
    /// Additional verified trust roots, useful for private archives and fixtures.
    #[serde(skip)]
    pub additional_roots: Vec<reqwest::Certificate>,
}
struct Route {
    url: Url,
    until: Instant,
}
#[derive(Default)]
struct Session {
    get: Option<Route>,
    cookies: BTreeMap<String, Jar>,
    cookie_bytes: usize,
    cookie_headers: usize,
    generation: u64,
}
#[derive(Default, Serialize)]
struct Counts {
    checkpoint_claims: u64,
    peak_in_flight: usize,
    wire_head: u64,
    wire_get: u64,
    redirects: u64,
    login_requests: u64,
    refreshes: u64,
    route_cache_hits: u64,
    response_body_bytes: u64,
    redirect_body_bytes: u64,
    signing_auth_seconds: f64,
    statuses: BTreeMap<u16, u64>,
    origins: BTreeMap<String, u64>,
}
/// A caller-owned session; deliberately has no Debug or Serialize implementation.
/// Clone an Arc to share its bounded cookies, connection pool and GET route.
pub struct AuthenticatedHttp {
    config: Config,
    canonical: Url,
    netrc: PathBuf,
    client: Client,
    runtime: tokio::runtime::Runtime,
    session: Mutex<Session>,
    resolution: Mutex<()>,
    in_flight: Mutex<usize>,
    concurrency_limit: usize,
    counts: Mutex<Counts>,
}
pub(crate) struct Response<'a> {
    pub(crate) inner: reqwest::Response,
    owner: &'a AuthenticatedHttp,
    cancelled: Option<&'a AtomicBool>,
    _permit: Permit<'a>,
}
struct Permit<'a>(&'a Mutex<usize>);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap() -= 1;
    }
}
fn origin(value: &str) -> Result<String> {
    let u = Url::parse(value).map_err(|_| "invalid authenticated HTTPS origin")?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || u.path() != "/"
        || u.host_str().is_none()
    {
        return fail("authenticated origins must be bare HTTPS origins");
    }
    Ok(u.origin().ascii_serialization())
}
fn safe_url(u: &Url) -> bool {
    u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && u.fragment().is_none()
        && u.as_str().len() <= 16384
}
fn strong_etag(value: &str) -> bool {
    value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && !value.starts_with("W/")
        && !value[1..value.len() - 1]
            .bytes()
            .any(|byte| byte < 33 || byte == 127 || byte == b'"')
}
fn valid_dswx_v11_filename(filename: &str) -> bool {
    const LAYERS: [&str; 10] = [
        "B01_WTR",
        "B02_BWTR",
        "B03_CONF",
        "B04_DIAG",
        "B05_WTR-1",
        "B06_WTR-2",
        "B07_LAND",
        "B08_SHAD",
        "B09_CLOUD",
        "B10_DEM",
    ];
    let Some(name) = filename.strip_suffix(".tif") else {
        return false;
    };
    let Some(stem) = LAYERS
        .iter()
        .find_map(|layer| name.strip_suffix(&format!("_{layer}")))
    else {
        return false;
    };
    let fields = stem.split('_').collect::<Vec<_>>();
    if fields.len() != 9
        || fields[..3] != ["OPERA", "L3", "DSWx-HLS"]
        || fields[6] != "L8"
            && fields[6] != "L9"
            && fields[6] != "S2A"
            && fields[6] != "S2B"
            && fields[6] != "S2C"
        || fields[7] != "30"
        || fields[8] != "v1.1"
    {
        return false;
    }
    let tile = fields[3].as_bytes();
    let valid_grid_letter = |byte: u8| byte.is_ascii_uppercase() && byte != b'I' && byte != b'O';
    let valid_tile = tile.len() == 6
        && tile[0] == b'T'
        && std::str::from_utf8(&tile[1..3])
            .ok()
            .and_then(|zone| zone.parse::<u8>().ok())
            .is_some_and(|zone| (1..=60).contains(&zone))
        && matches!(tile[3], b'C'..=b'H' | b'J'..=b'N' | b'P'..=b'X')
        && valid_grid_letter(tile[4])
        && valid_grid_letter(tile[5]);
    let valid_time = |value: &str| {
        value.len() == 16 && chrono::NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ").is_ok()
    };
    valid_tile && valid_time(fields[4]) && valid_time(fields[5])
}
impl AuthenticatedHttp {
    /// Opt in to the observed PO.DAAC SWOT access contract for one product.
    /// The file path is a credential source, never a credential value.
    pub fn earthdata(canonical_url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        let canonical = Url::parse(canonical_url).map_err(|_| "invalid Earthdata locator")?;
        let archive = "https://archive.swot.podaac.earthdata.nasa.gov";
        let login = "https://urs.earthdata.nasa.gov";
        let resource = "https://d176wrl3f7yqfo.cloudfront.net";
        if canonical.origin().ascii_serialization() != archive
            || !canonical
                .path()
                .starts_with("/podaac-swot-ops-cumulus-protected/SWOT_L2_HR_Raster_D/")
        {
            return fail("Earthdata profile requires a canonical SWOT Raster D archive locator");
        }
        // These are nonsecret, observed method adapters for the same S3 key.
        // A changed deployment path fails closed until separately qualified.
        let key = canonical
            .path()
            .strip_prefix("/podaac-swot-ops-cumulus-protected/")
            .ok_or("invalid SWOT key")?;
        let suffix = format!("/podaac-swot-ops-cumulus-protected.s3.us-west-2.amazonaws.com/{key}");
        let head_path = format!("/head-2c0fc4c958b8bc10f4fa177bae3b4543{suffix}");
        let get_path = format!("/s3-2c0fc4c958b8bc10f4fa177bae3b4543{suffix}");
        let rule = |from: &str, to: &str, path: &str| RedirectRule {
            from_origin: from.into(),
            to_origin: to.into(),
            destination_path: path.into(),
        };
        Self::new(
            Config {
                canonical_url: canonical_url.into(),
                login_origin: login.into(),
                resource_origin: resource.into(),
                head_resource_path: head_path.clone(),
                get_resource_path: get_path.clone(),
                redirects: vec![
                    rule(archive, login, "/oauth/authorize"),
                    rule(login, archive, "/login"),
                    rule(archive, archive, canonical.path()),
                    rule(archive, resource, &head_path),
                    rule(archive, resource, &get_path),
                ],
                additional_roots: vec![],
            },
            netrc,
        )
    }
    /// Opt in to the observed LP DAAC HLS V2 protected-object contract.
    ///
    /// This profile remains exact-object scoped. It accepts only unsigned HLS
    /// L30/S30 V2 TIFF object URLs and derives the provider's method-adapter
    /// paths for that same protected S3 key.
    pub fn earthdata_hls(canonical_url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        let canonical = Url::parse(canonical_url).map_err(|_| "invalid HLS Earthdata locator")?;
        let archive = "https://data.lpdaac.earthdatacloud.nasa.gov";
        let login = "https://urs.earthdata.nasa.gov";
        let resource = "https://d1nklfio7vscoe.cloudfront.net";
        let path = canonical.path();
        if canonical.origin().ascii_serialization() != archive
            || canonical.query().is_some()
            || canonical.fragment().is_some()
            || !(path.starts_with("/lp-prod-protected/HLSL30.020/")
                || path.starts_with("/lp-prod-protected/HLSS30.020/"))
            || !path.ends_with(".tif")
        {
            return fail(
                "HLS Earthdata profile requires an unsigned HLSL30/HLSS30 V2 TIFF locator",
            );
        }
        let key = path
            .strip_prefix("/lp-prod-protected/")
            .ok_or("invalid HLS protected key")?;
        if key.is_empty()
            || key
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return fail("invalid HLS protected object path");
        }
        let suffix = format!("/lp-prod-protected.s3.us-west-2.amazonaws.com/{key}");
        let head_path = format!("/head-2d2df3a34830d5223d1e9547cd713408{suffix}");
        let get_path = format!("/s3-2d2df3a34830d5223d1e9547cd713408{suffix}");
        let rule = |from: &str, to: &str, path: &str| RedirectRule {
            from_origin: from.into(),
            to_origin: to.into(),
            destination_path: path.into(),
        };
        Self::new(
            Config {
                canonical_url: canonical_url.into(),
                login_origin: login.into(),
                resource_origin: resource.into(),
                head_resource_path: head_path.clone(),
                get_resource_path: get_path.clone(),
                redirects: vec![
                    rule(archive, login, "/oauth/authorize"),
                    rule(login, archive, "/login"),
                    rule(archive, archive, canonical.path()),
                    rule(archive, resource, &head_path),
                    rule(archive, resource, &get_path),
                ],
                additional_roots: vec![],
            },
            netrc,
        )
    }
    /// Opt in to the observed PO.DAAC OPERA DSWx-HLS V1.1 object contract.
    ///
    /// This profile is exact-object scoped and intentionally accepts only the
    /// ten current DSWx-HLS V1.1 GeoTIFF layer names. A changed producer
    /// version, collection path, filename grammar, or provider method-adapter
    /// path fails closed until it is qualified separately.
    pub fn earthdata_dswx(canonical_url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        let canonical = Url::parse(canonical_url).map_err(|_| "invalid DSWx Earthdata locator")?;
        let archive = "https://archive.podaac.earthdata.nasa.gov";
        let login = "https://urs.earthdata.nasa.gov";
        let resource = "https://deotb6e7tfubr.cloudfront.net";
        let prefix = "/podaac-ops-cumulus-protected/OPERA_L3_DSWX-HLS_V1/";
        let path = canonical.path();
        let filename = path.strip_prefix(prefix).unwrap_or_default();
        if canonical.origin().ascii_serialization() != archive
            || canonical.query().is_some()
            || canonical.fragment().is_some()
            || filename.contains('/')
            || !valid_dswx_v11_filename(filename)
        {
            return fail(
                "DSWx Earthdata profile requires an unsigned OPERA DSWx-HLS V1.1 layer TIFF locator",
            );
        }
        let key = path
            .strip_prefix("/podaac-ops-cumulus-protected/")
            .ok_or("invalid DSWx protected key")?;
        let suffix = format!("/podaac-ops-cumulus-protected.s3.us-west-2.amazonaws.com/{key}");
        let head_path = format!("/head-edaf5da92e0ce48fb61175c28b67e95d{suffix}");
        let get_path = format!("/s3-edaf5da92e0ce48fb61175c28b67e95d{suffix}");
        let rule = |from: &str, to: &str, path: &str| RedirectRule {
            from_origin: from.into(),
            to_origin: to.into(),
            destination_path: path.into(),
        };
        Self::new(
            Config {
                canonical_url: canonical_url.into(),
                login_origin: login.into(),
                resource_origin: resource.into(),
                head_resource_path: head_path.clone(),
                get_resource_path: get_path.clone(),
                redirects: vec![
                    rule(archive, login, "/oauth/authorize"),
                    rule(login, archive, "/login"),
                    rule(archive, archive, canonical.path()),
                    rule(archive, resource, &head_path),
                    rule(archive, resource, &get_path),
                ],
                additional_roots: vec![],
            },
            netrc,
        )
    }
    /// Exact observed PO.DAAC DSWx-S1 V1.0 route, independently qualified
    /// from DSWx-HLS. The four layer names and source release are closed.
    pub fn earthdata_dswx_s1(canonical_url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        Self::earthdata_wave2(canonical_url, netrc, false)
    }
    /// Exact observed LP DAAC DIST-ALERT-HLS V1 route. No annual DIST product
    /// or another LP DAAC collection inherits this access policy.
    pub fn earthdata_dist_alert(canonical_url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        Self::earthdata_wave2(canonical_url, netrc, true)
    }
    fn earthdata_wave2(canonical_url: &str, netrc: impl Into<PathBuf>, dist: bool) -> Result<Self> {
        let canonical =
            Url::parse(canonical_url).map_err(|_| "invalid Wave 2 Earthdata locator")?;
        let (product, archive, resource, prefix, gate) = if dist {
            (
                "opera-dist-alert",
                "https://data.lpdaac.earthdatacloud.nasa.gov",
                "https://d1nklfio7vscoe.cloudfront.net",
                "/lp-prod-protected/",
                "2d2df3a34830d5223d1e9547cd713408",
            )
        } else {
            (
                "opera-dswx-s1",
                "https://archive.podaac.earthdata.nasa.gov",
                "https://deotb6e7tfubr.cloudfront.net",
                "/podaac-ops-cumulus-protected/",
                "edaf5da92e0ce48fb61175c28b67e95d",
            )
        };
        let collection = if dist {
            "OPERA_L3_DIST-ALERT-HLS_V1"
        } else {
            "OPERA_L3_DSWX-S1_V1"
        };
        let key = canonical
            .path()
            .strip_prefix(prefix)
            .ok_or("unqualified Wave 2 archive path")?;
        let parts = key.split('/').collect::<Vec<_>>();
        let filename = parts.last().copied().unwrap_or_default();
        let profile = crate::products::profile::profile(product)?;
        if canonical.origin().ascii_serialization() != archive
            || canonical.query().is_some()
            || canonical.fragment().is_some()
            || !canonical.username().is_empty()
            || canonical.password().is_some()
            || parts.first() != Some(&collection)
            || !filename.ends_with(".tif")
            || parts.len() != if dist { 3 } else { 2 }
        {
            return fail(
                "Wave 2 Earthdata locator violates exact product collection/object policy",
            );
        }
        profile.validate_identity(filename, None, None)?;
        let stem = filename
            .strip_suffix(".tif")
            .ok_or("invalid Wave 2 TIFF filename")?;
        let identity = stem.split('_').take(9).collect::<Vec<_>>().join("_");
        if dist && parts[1] != identity {
            return fail("DIST containing granule conflicts with exact object identity");
        }
        let bucket = prefix.trim_matches('/');
        let suffix = format!("/{bucket}.s3.us-west-2.amazonaws.com/{key}");
        let head_path = format!("/head-{gate}{suffix}");
        let get_path = format!("/s3-{gate}{suffix}");
        let login = "https://urs.earthdata.nasa.gov";
        let rule = |from: &str, to: &str, path: &str| RedirectRule {
            from_origin: from.into(),
            to_origin: to.into(),
            destination_path: path.into(),
        };
        Self::new(
            Config {
                canonical_url: canonical_url.into(),
                login_origin: login.into(),
                resource_origin: resource.into(),
                head_resource_path: head_path.clone(),
                get_resource_path: get_path.clone(),
                redirects: vec![
                    rule(archive, login, "/oauth/authorize"),
                    rule(login, archive, "/login"),
                    rule(archive, archive, canonical.path()),
                    rule(archive, resource, &head_path),
                    rule(archive, resource, &get_path),
                ],
                additional_roots: vec![],
            },
            netrc,
        )
    }
    /// Exact observed NSIDC SPL3SMP_E v006/R19240 object contract. No hosts
    /// or redirect grants are inherited from another Earthdata archive.
    pub fn earthdata_smap(canonical_url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        let canonical = Url::parse(canonical_url)?;
        let archive = "https://data.nsidc.earthdatacloud.nasa.gov";
        let prefix = "/nsidc-cumulus-prod-protected/SMAP/SPL3SMP_E/006/";
        let key = canonical
            .path()
            .strip_prefix(prefix)
            .ok_or("SMAP exact collection path required")?;
        let parts = key.split('/').collect::<Vec<_>>();
        if canonical.origin().ascii_serialization() != archive
            || canonical.query().is_some()
            || canonical.fragment().is_some()
            || !canonical.username().is_empty()
            || canonical.password().is_some()
            || parts.len() != 4
        {
            return fail("SMAP exact unsigned NSIDC object required");
        }
        let date = format!("{}{}{}", parts[0], parts[1], parts[2]);
        chrono::NaiveDate::parse_from_str(&date, "%Y%m%d")?;
        if parts[0].len() != 4
            || parts[1].len() != 2
            || parts[2].len() != 2
            || parts[3] != format!("SMAP_L3_SM_P_E_{date}_R19240_001.h5")
        {
            return fail("SMAP source date, processing release or filename conflict");
        }
        let resource = "https://d3h5e2j7riftk6.cloudfront.net";
        let suffix = format!(
            "/nsidc-cumulus-prod-protected.s3.us-west-2.amazonaws.com/SMAP/SPL3SMP_E/006/{key}"
        );
        let head = format!("/head-6fa4010df9e3d374e86134879ef72c9b{suffix}");
        let get = format!("/s3-6fa4010df9e3d374e86134879ef72c9b{suffix}");
        let login = "https://urs.earthdata.nasa.gov";
        let rule = |a: &str, b: &str, p: &str| RedirectRule {
            from_origin: a.into(),
            to_origin: b.into(),
            destination_path: p.into(),
        };
        Self::new(
            Config {
                canonical_url: canonical_url.into(),
                login_origin: login.into(),
                resource_origin: resource.into(),
                head_resource_path: head.clone(),
                get_resource_path: get.clone(),
                redirects: vec![
                    rule(archive, login, "/oauth/authorize"),
                    rule(login, archive, "/login"),
                    rule(archive, archive, canonical.path()),
                    rule(archive, resource, &head),
                    rule(archive, resource, &get),
                ],
                additional_roots: vec![],
            },
            netrc,
        )
    }
    /// Exact observed provisional NISAR GCOV P05023 source; independent ASF
    /// archive and resource grants, with original HDF5 as the only source.
    pub fn earthdata_nisar(canonical_url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        let u = Url::parse(canonical_url)?;
        let archive = "https://nisar.asf.earthdatacloud.nasa.gov";
        let prefix = "/NISAR/NISAR_L2_GCOV_PROVISIONAL_V1/";
        let key = u
            .path()
            .strip_prefix(prefix)
            .ok_or("NISAR provisional source path required")?;
        let p = key.split('/').collect::<Vec<_>>();
        if u.origin().ascii_serialization() != archive
            || u.query().is_some()
            || u.fragment().is_some()
            || !u.username().is_empty()
            || u.password().is_some()
            || p.len() != 2
            || p[0].len() != 91
            || !p[0].starts_with("NISAR_L2_PR_GCOV_")
            || !p[0].ends_with("_P05023_N_F_J_001")
            || !p[0]
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || p[1] != format!("{}.h5", p[0])
        {
            return fail("NISAR exact unsigned GCOV P05023 object required");
        }
        let resource = "https://d1mv8zhcvry6x4.cloudfront.net";
        let suffix=format!("/sds-n-cumulus-prod-nisar-products.s3.us-west-2.amazonaws.com/NISAR_L2_GCOV_PROVISIONAL_V1/{key}");
        let head = format!("/head-7fdfbcb0ce308dc58d08f97acb1f0590{suffix}");
        let get = format!("/s3-7fdfbcb0ce308dc58d08f97acb1f0590{suffix}");
        let login = "https://urs.earthdata.nasa.gov";
        let rule = |a: &str, b: &str, p: &str| RedirectRule {
            from_origin: a.into(),
            to_origin: b.into(),
            destination_path: p.into(),
        };
        Self::new(
            Config {
                canonical_url: canonical_url.into(),
                login_origin: login.into(),
                resource_origin: resource.into(),
                head_resource_path: head.clone(),
                get_resource_path: get.clone(),
                redirects: vec![
                    rule(archive, login, "/oauth/authorize"),
                    rule(login, archive, "/login"),
                    rule(archive, archive, u.path()),
                    rule(archive, resource, &head),
                    rule(archive, resource, &get),
                ],
                additional_roots: vec![],
            },
            netrc,
        )
    }
    /// Opt in to the observed ASF OPERA RTC-S1 V1 protected-object contract.
    ///
    /// Current CMR records expose unsigned Cumulus object URLs directly. This
    /// profile accepts only a VV/VH/HH/HV or mask GeoTIFF whose containing
    /// directory and filename identify the same valid RTC-S1 V1 granule. The
    /// fixed method-adapter paths refer to that exact ASF production S3 key.
    pub fn earthdata_rtc(canonical_url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        let canonical = Url::parse(canonical_url).map_err(|_| "invalid RTC Earthdata locator")?;
        let archive = "https://cumulus.asf.earthdatacloud.nasa.gov";
        let login = "https://urs.earthdata.nasa.gov";
        let resource = "https://d1bctmng8vmwga.cloudfront.net";
        let prefix = "/OPERA/OPERA_L2_RTC-S1/";
        let path = canonical.path();
        let parts = path
            .strip_prefix(prefix)
            .unwrap_or_default()
            .split('/')
            .collect::<Vec<_>>();
        if canonical.origin().ascii_serialization() != archive
            || canonical.query().is_some()
            || canonical.fragment().is_some()
            || parts.len() != 2
            || parts.iter().any(|part| part.is_empty())
        {
            return fail(
                "RTC Earthdata profile requires an unsigned OPERA RTC-S1 V1 layer TIFF locator",
            );
        }
        let granule = parts[0];
        let filename = parts[1];
        let layer = filename
            .strip_prefix(&format!("{granule}_"))
            .and_then(|value| value.strip_suffix(".tif"));
        if !matches!(layer, Some("VV" | "VH" | "HH" | "HV" | "mask")) {
            return fail("RTC Earthdata profile requires a supported RTC layer filename");
        }
        crate::products::profile::profile("opera-rtc-s1")?
            .validate_identity(granule, None, None)?;

        let key = path
            .strip_prefix("/OPERA/")
            .ok_or("invalid RTC protected key")?;
        let suffix = format!("/asf-cumulus-prod-opera-products.s3.us-west-2.amazonaws.com/{key}");
        let head_path = format!("/head-b339e3a40716b5999fea2aad1aee9f03{suffix}");
        let get_path = format!("/s3-b339e3a40716b5999fea2aad1aee9f03{suffix}");
        let rule = |from: &str, to: &str, path: &str| RedirectRule {
            from_origin: from.into(),
            to_origin: to.into(),
            destination_path: path.into(),
        };
        Self::new(
            Config {
                canonical_url: canonical_url.into(),
                login_origin: login.into(),
                resource_origin: resource.into(),
                head_resource_path: head_path.clone(),
                get_resource_path: get_path.clone(),
                redirects: vec![
                    rule(archive, login, "/oauth/authorize"),
                    rule(login, archive, "/login"),
                    rule(archive, archive, canonical.path()),
                    rule(archive, resource, &head_path),
                    rule(archive, resource, &get_path),
                ],
                additional_roots: vec![],
            },
            netrc,
        )
    }
    /// Exact object policy for the separately qualified DIST-S1 v1 route.
    pub fn earthdata_dist_s1(url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        Self::earthdata_dist_product(url, netrc, true)
    }
    /// Exact object policy for DIST-ANN-HLS v1; no other LP DAAC collection inherits it.
    pub fn earthdata_dist_ann(url: &str, netrc: impl Into<PathBuf>) -> Result<Self> {
        Self::earthdata_dist_product(url, netrc, false)
    }
    fn earthdata_dist_product(url: &str, netrc: impl Into<PathBuf>, radar: bool) -> Result<Self> {
        let canonical = Url::parse(url)?;
        let (product, archive, resource, prefix, bucket, gate) = if radar {
            (
                "opera-dist-alert-s1",
                "https://cumulus.asf.earthdatacloud.nasa.gov",
                "https://d1bctmng8vmwga.cloudfront.net",
                "/OPERA/",
                "asf-cumulus-prod-opera-products",
                "b339e3a40716b5999fea2aad1aee9f03",
            )
        } else {
            (
                "opera-dist-ann-hls",
                "https://data.lpdaac.earthdatacloud.nasa.gov",
                "https://d1nklfio7vscoe.cloudfront.net",
                "/lp-prod-protected/",
                "lp-prod-protected",
                "2d2df3a34830d5223d1e9547cd713408",
            )
        };
        let profile = crate::products::profile::profile(product)?;
        let key = canonical
            .path()
            .strip_prefix(prefix)
            .ok_or("DIST exact archive prefix required")?;
        let parts = key.split('/').collect::<Vec<_>>();
        if canonical.origin().ascii_serialization() != archive
            || !canonical.username().is_empty()
            || canonical.password().is_some()
            || canonical.query().is_some()
            || canonical.fragment().is_some()
            || parts.len() != 3
            || parts[0] != profile.catalog.collection
        {
            return fail("DIST exact unsigned collection/granule/layer object required");
        }
        profile.validate_identity(parts[1], None, None)?;
        if !profile
            .bands
            .iter()
            .any(|band| parts[2] == format!("{}_{}.tif", parts[1], band.name))
        {
            return fail("DIST filename must match the containing granule and admitted layer");
        }
        let suffix = format!("/{bucket}.s3.us-west-2.amazonaws.com/{key}");
        let head = format!("/head-{gate}{suffix}");
        let get = format!("/s3-{gate}{suffix}");
        let login = "https://urs.earthdata.nasa.gov";
        let rule = |a: &str, b: &str, p: &str| RedirectRule {
            from_origin: a.into(),
            to_origin: b.into(),
            destination_path: p.into(),
        };
        Self::new(
            Config {
                canonical_url: url.into(),
                login_origin: login.into(),
                resource_origin: resource.into(),
                head_resource_path: head.clone(),
                get_resource_path: get.clone(),
                redirects: vec![
                    rule(archive, login, "/oauth/authorize"),
                    rule(login, archive, "/login"),
                    rule(archive, archive, canonical.path()),
                    rule(archive, resource, &head),
                    rule(archive, resource, &get),
                ],
                additional_roots: vec![],
            },
            netrc,
        )
    }
    pub fn new(mut config: Config, netrc: impl Into<PathBuf>) -> Result<Self> {
        let canonical = Url::parse(&config.canonical_url)
            .map_err(|_| "invalid canonical authenticated locator")?;
        if !safe_url(&canonical) || canonical.query().is_some() || canonical.path().ends_with('/') {
            return fail("canonical authenticated locator must be an unsigned HTTPS object URL");
        }
        config.login_origin = origin(&config.login_origin)?;
        config.resource_origin = origin(&config.resource_origin)?;
        if config.login_origin == config.resource_origin
            || config.login_origin == canonical.origin().ascii_serialization()
        {
            return fail("login credentials require a distinct authentication origin");
        }
        for path in [&config.head_resource_path, &config.get_resource_path] {
            let u = Url::parse(&format!("{}{}", config.resource_origin, path))
                .map_err(|_| "invalid resource path policy")?;
            if u.path() != path
                || !path.starts_with('/')
                || u.query().is_some()
                || u.fragment().is_some()
                || u.origin().ascii_serialization() != config.resource_origin
            {
                return fail("resource identities require exact absolute paths");
            }
        }
        if config.redirects.len() > 16 {
            return fail("authenticated redirect policy exceeds bounds");
        }
        for r in &mut config.redirects {
            r.from_origin = origin(&r.from_origin)?;
            r.to_origin = origin(&r.to_origin)?;
            let target = Url::parse(&format!("{}{}", r.to_origin, r.destination_path))
                .map_err(|_| "invalid redirect destination policy")?;
            if !r.destination_path.starts_with('/')
                || target.origin().ascii_serialization() != r.to_origin
                || target.path() != r.destination_path
                || target.query().is_some()
                || target.fragment().is_some()
            {
                return fail("redirect policy requires an exact absolute destination path");
            }
        }
        let netrc = netrc.into();
        let login = Url::parse(&config.login_origin).map_err(|_| "invalid login origin")?;
        let _ = credentials(&netrc, login.host_str().ok_or("missing login host")?)?;
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .user_agent("tile-ripper/0.1 authenticated-bounded-acquisition");
        for root in &config.additional_roots {
            builder = builder.add_root_certificate(root.clone());
        }
        let client = builder
            .build()
            .map_err(|_| "cannot initialize authenticated HTTPS client")?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|_| "cannot initialize authenticated HTTP runtime")?;
        Ok(Self {
            config,
            canonical,
            netrc,
            client,
            runtime,
            session: Mutex::new(Session::default()),
            resolution: Mutex::new(()),
            in_flight: Mutex::new(0),
            concurrency_limit: crate::storage::worker_count()?.min(4),
            counts: Mutex::new(Counts::default()),
        })
    }
    pub(crate) fn accepts(&self, locator: &str) -> bool {
        locator == self.canonical.as_str()
    }
    /// Resolve the canonical object's current immutable identity through this
    /// session's exact redirect policy.
    pub(crate) fn head_identity(&self, cancelled: Option<&AtomicBool>) -> Result<(String, u64)> {
        let response = self.send(
            Method::HEAD,
            self.canonical.clone(),
            HeaderMap::new(),
            cancelled,
        )?;
        if response.inner.status().as_u16() != 200 {
            return fail("authenticated object identity HEAD failed");
        }
        let etag = response
            .inner
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|value| strong_etag(value))
            .ok_or("authenticated object identity requires a strong ETag")?
            .to_owned();
        let bytes = response
            .inner
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .ok_or("authenticated object identity HEAD missing content length")?;
        Ok((etag, bytes))
    }

    /// Recheck one previously pinned identity. `If-Match` is retained through
    /// the canonical and signed object hops, and the returned identity must
    /// still agree independently with both pins.
    pub(crate) fn validate_identity(
        &self,
        expected_etag: &str,
        expected_bytes: u64,
        cancelled: Option<&AtomicBool>,
    ) -> Result<()> {
        if !strong_etag(expected_etag) || expected_bytes == 0 {
            return fail("invalid authenticated object identity pin");
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_MATCH,
            HeaderValue::from_str(expected_etag)
                .map_err(|_| "invalid authenticated object ETag pin")?,
        );
        let response = self.send(Method::HEAD, self.canonical.clone(), headers, cancelled)?;
        if response.inner.status().as_u16() != 200
            || response
                .inner
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok())
                != Some(expected_etag)
            || response
                .inner
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                != Some(expected_bytes)
        {
            return fail("authenticated object identity changed");
        }
        Ok(())
    }
    fn object(&self, url: &Url, method: &Method) -> bool {
        url.origin().ascii_serialization() == self.config.resource_origin
            && url.path()
                == if *method == Method::HEAD {
                    &self.config.head_resource_path
                } else {
                    &self.config.get_resource_path
                }
    }
    fn check_redirect(&self, from: &Url, to: &Url, method: &Method) -> Result<()> {
        if !safe_url(to)
            || (to.origin().ascii_serialization() == self.config.resource_origin
                && !self.object(to, method))
            || !self.config.redirects.iter().any(|r| {
                r.from_origin == from.origin().ascii_serialization()
                    && r.to_origin == to.origin().ascii_serialization()
                    && r.destination_path == to.path()
            })
        {
            return fail("unsafe or unconfigured authenticated redirect");
        }
        Ok(())
    }
    fn wait<T, F: Future<Output = reqwest::Result<T>>>(
        &self,
        make_future: impl FnOnce() -> F,
        cancelled: Option<&AtomicBool>,
    ) -> Result<T> {
        self.runtime.block_on(async {
            let future = make_future();
            tokio::pin!(future);
            loop {
                if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) { return fail("acquisition cancelled"); }
                tokio::select! {
                    result = &mut future => return result.map_err(|error| {
                        let timeout = error.is_timeout();
                        let detail = format!("{:?}",error.without_url());
                        let message = if timeout { "retryable authenticated HTTP timeout" }
                            else if detail.contains("EkuError") { "authenticated TLS certificate purpose rejected" }
                            else if detail.contains("InvalidCertificate") { "authenticated TLS certificate rejected" }
                            else if detail.contains("dns") || detail.contains("Dns") { "retryable authenticated DNS failure" }
                            else { "retryable authenticated connection or body failure" };
                        message.into()
                    }),
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
        })
    }
    fn lock_resolution(
        &self,
        cancelled: Option<&AtomicBool>,
    ) -> Result<std::sync::MutexGuard<'_, ()>> {
        let started = Instant::now();
        loop {
            if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
                return fail("acquisition cancelled");
            }
            if let Ok(guard) = self.resolution.try_lock() {
                return Ok(guard);
            }
            if started.elapsed() > Duration::from_secs(30) {
                return fail("authentication resolution timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn cached(&self, method: &Method) -> Option<Url> {
        if *method != Method::GET {
            return None;
        }
        self.session
            .lock()
            .unwrap()
            .get
            .as_ref()
            .filter(|r| Instant::now() < r.until)
            .map(|r| r.url.clone())
    }
    fn store_route(&self, method: &Method, url: &Url) {
        if *method != Method::GET {
            return;
        }
        // Only a provider-advertised CloudFront expiry is understood. Unknown
        // signing schemes resolve from canonical on every operation.
        let expiration = url
            .query_pairs()
            .find(|(k, _)| k == "Expires")
            .and_then(|(_, v)| v.parse::<u64>().ok());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(u64::MAX, |d| d.as_secs());
        if let Some(ttl) = expiration
            .and_then(|e| e.checked_sub(now + 30))
            .filter(|n| *n > 0)
        {
            self.session.lock().unwrap().get = Some(Route {
                url: url.clone(),
                until: Instant::now() + Duration::from_secs(ttl.min(60)),
            });
        }
    }
    fn request(
        &self,
        method: &Method,
        url: &Url,
        headers: &HeaderMap,
        cancelled: Option<&AtomicBool>,
    ) -> Result<reqwest::Response> {
        let o = url.origin().ascii_serialization();
        let mut req = self
            .client
            .request(method.clone(), url.clone())
            .header(header::ACCEPT_ENCODING, "identity");
        // Object conditionals survive every object hop; OAuth is a separate
        // endpoint and must not receive product headers or an object body.
        if self.object(url, method) || url == &self.canonical {
            req = req.headers(headers.clone());
        }
        if o == self.config.login_origin {
            let auth = credentials(&self.netrc, url.host_str().ok_or("missing login host")?)?;
            req = req.header(header::AUTHORIZATION, auth);
        }
        // Separate jars per exact origin confine even Domain=.earthdata.nasa.gov
        // cookies. The signed object receives neither cookies nor Authorization.
        if o == self.config.login_origin || o == self.canonical.origin().ascii_serialization() {
            if let Some(mut cookie) = self
                .session
                .lock()
                .unwrap()
                .cookies
                .get(&o)
                .and_then(|j| j.cookies(url))
            {
                cookie.set_sensitive(true);
                req = req.header(header::COOKIE, cookie);
            }
        }
        {
            let mut c = self.counts.lock().unwrap();
            if *method == Method::HEAD {
                c.wire_head += 1;
            } else {
                c.wire_get += 1;
            }
            if o == self.config.login_origin {
                c.login_requests += 1;
            }
            *c.origins.entry(o.clone()).or_default() += 1;
        }
        let response = self.wait(|| req.send(), cancelled).map_err(|error| {
            let role = if o == self.config.login_origin {
                "login"
            } else if o == self.config.resource_origin {
                "resource"
            } else {
                "canonical"
            };
            format!("{error} at configured {role} origin")
        })?;
        *self
            .counts
            .lock()
            .unwrap()
            .statuses
            .entry(response.status().as_u16())
            .or_default() += 1;
        if o == self.config.login_origin || o == self.canonical.origin().ascii_serialization() {
            let mut s = self.session.lock().unwrap();
            let cookies = response.headers().get_all(header::SET_COOKIE);
            let n = cookies.iter().map(|h| h.as_bytes().len()).sum::<usize>();
            s.cookie_headers += cookies.iter().count();
            if s.cookie_headers > 128 {
                return fail("authenticated cookie count budget exceeded");
            }
            if s.cookie_bytes
                .checked_add(n)
                .is_none_or(|v| v > SESSION_BYTES)
            {
                return fail("authenticated cookie budget exceeded");
            }
            s.cookie_bytes += n;
            if n > 0 {
                s.generation += 1;
            }
            s.cookies
                .entry(o)
                .or_default()
                .set_cookies(&mut cookies.iter(), url);
        }
        Ok(response)
    }
    pub(crate) fn send<'a>(
        &'a self,
        method: Method,
        canonical: Url,
        headers: HeaderMap,
        cancelled: Option<&'a AtomicBool>,
    ) -> Result<Response<'a>> {
        let admission = Instant::now();
        let permit = loop {
            if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
                return fail("acquisition cancelled");
            }
            {
                let mut n = self.in_flight.lock().unwrap();
                if *n < self.concurrency_limit {
                    *n += 1;
                    let mut counts = self.counts.lock().unwrap();
                    counts.peak_in_flight = counts.peak_in_flight.max(*n);
                    break Permit(&self.in_flight);
                }
            }
            if admission.elapsed() > Duration::from_secs(30) {
                return fail("authenticated concurrency admission timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if method == Method::GET
            && (!headers.contains_key(header::IF_MATCH) || !headers.contains_key(header::RANGE))
        {
            return fail("authenticated GET requires a pinned conditional byte range");
        }
        if canonical != self.canonical
            || ![Method::HEAD, Method::GET].contains(&method)
            || headers.contains_key(header::AUTHORIZATION)
            || headers.contains_key(header::COOKIE)
        {
            return fail("authenticated transport is outside its canonical object scope");
        }
        // At most one new authentication/signing resolution after a rejected
        // route. 412 and identity conflicts never refresh or retry.
        let mut force = false;
        for refresh in 0..2 {
            let cached = if force { None } else { self.cached(&method) };
            let guard = if cached.is_none() {
                Some(self.lock_resolution(cancelled)?)
            } else {
                None
            };
            let cached = if force {
                None
            } else {
                cached.or_else(|| self.cached(&method))
            };
            let reused = cached.is_some();
            if reused {
                self.counts.lock().unwrap().route_cache_hits += 1;
            }
            let clock = Instant::now();
            let mut current = cached.unwrap_or_else(|| self.canonical.clone());
            let mut visited = Vec::new();
            let mut refresh_needed = false;
            for _ in 0..=MAX_HOPS {
                let visit = (current.clone(), self.session.lock().unwrap().generation);
                if visited.contains(&visit) {
                    return fail("authenticated redirect loop");
                }
                visited.push(visit);
                let mut response = self.request(&method, &current, &headers, cancelled)?;
                let status = response.status().as_u16();
                if [301, 302, 303, 307, 308].contains(&status) {
                    if visited.len() > MAX_HOPS {
                        return fail("authenticated redirect hop budget exhausted");
                    }
                    let target = response
                        .headers()
                        .get(header::LOCATION)
                        .and_then(|h| h.to_str().ok())
                        .and_then(|s| current.join(s).ok())
                        .ok_or("invalid authenticated redirect")?;
                    self.check_redirect(&current, &target, &method)?;
                    self.counts.lock().unwrap().redirects += 1;
                    let mut discard = Vec::new();
                    self.read_body(&mut response, 16384, &mut discard, cancelled, true)?;
                    current = target;
                    continue;
                }
                if [401, 403].contains(&status) {
                    if refresh == 1
                        || current.origin().ascii_serialization() == self.config.login_origin
                    {
                        return fail("authenticated access rejected; refresh exhausted");
                    }
                    let mut s = self.session.lock().unwrap();
                    // A concurrent newer GET route must not be invalidated by
                    // rejection of an older in-flight URL.
                    if s.get.as_ref().is_some_and(|r| r.url == current) {
                        s.get = None;
                    }
                    if !self.object(&current, &method) {
                        s.cookies.clear();
                        s.cookie_bytes = 0;
                        s.cookie_headers = 0;
                    }
                    refresh_needed = true;
                    force = true;
                    self.counts.lock().unwrap().refreshes += 1;
                    break;
                }
                if ![408, 429, 500, 502, 503, 504, 412].contains(&status)
                    && !self.object(&current, &method)
                {
                    return fail(
                        "authenticated route did not resolve to the configured object identity",
                    );
                }
                if status == 200 && method == Method::HEAD || status == 206 && method == Method::GET
                {
                    self.store_route(&method, &current);
                }
                if !reused {
                    self.counts.lock().unwrap().signing_auth_seconds +=
                        clock.elapsed().as_secs_f64();
                }
                drop(guard);
                return Ok(Response {
                    inner: response,
                    owner: self,
                    cancelled,
                    _permit: permit,
                });
            }
            drop(guard);
            if !refresh_needed {
                return fail("authenticated redirect hop budget exhausted");
            }
        }
        fail("authentication refresh exhausted")
    }
    fn read_body(
        &self,
        response: &mut reqwest::Response,
        cap: u64,
        body: &mut Vec<u8>,
        cancelled: Option<&AtomicBool>,
        redirect: bool,
    ) -> Result<()> {
        while let Some(chunk) = self.wait(|| response.chunk(), cancelled)? {
            let mut c = self.counts.lock().unwrap();
            c.response_body_bytes += chunk.len() as u64;
            if redirect {
                c.redirect_body_bytes += chunk.len() as u64;
            }
            drop(c);
            if (body.len() as u64)
                .checked_add(chunk.len() as u64)
                .is_none_or(|n| n > cap)
            {
                return fail("authenticated response body exceeds requested bounds");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(())
    }
    pub(crate) fn identity(&self) -> serde_json::Value {
        serde_json::json!({"canonical_url":self.canonical.as_str(),"resource_origin":self.config.resource_origin,
            "head_resource_path":self.config.head_resource_path,"get_resource_path":self.config.get_resource_path,"login_origin":self.config.login_origin,
            "credential_forwarding":"login origin only; none to archive or signed resource",
            "cookies":"memory only; exact origin jars; none to signed resource",
            "conditional_reads":"strong ETag, If-Match, exact Content-Range and Content-Length, identity encoding",
            "signed_routes":"GET only, advertised expiry minus 30 seconds, at most 60 seconds; HEAD always canonical",
            "redirect_hop_limit":MAX_HOPS,"refresh_limit":1,"per_origin_concurrency_limit":self.concurrency_limit,"working_reservation_bytes":WORKING_RESERVATION,
            "redirects":self.config.redirects.iter().map(|r|serde_json::json!({"from":r.from_origin,"to":r.to_origin,"path":r.destination_path})).collect::<Vec<_>>()})
    }
    #[cfg(feature = "native-hdf5")]
    pub(crate) fn checkpoint_policy(&self) -> Result<serde_json::Value> {
        // Route expiry, session cookies, credentials, CA-store contents and
        // concurrency are not object access-policy identity. TLS validation
        // still runs afresh; caller-added trust roots never authorize new paths.
        #[derive(Serialize)]
        struct Policy<'a> {
            policy_version: &'static str,
            canonical: &'a str,
            login_origin: &'a str,
            resource_origin: &'a str,
            head_path: &'a str,
            get_path: &'a str,
            redirects: Vec<[&'a str; 3]>,
        }
        crate::checkpoint::bounded_identity(&Policy {
            policy_version: "exact-object-https-netrc-v1",
            canonical: self.canonical.as_str(),
            login_origin: &self.config.login_origin,
            resource_origin: &self.config.resource_origin,
            head_path: &self.config.head_resource_path,
            get_path: &self.config.get_resource_path,
            redirects: self
                .config
                .redirects
                .iter()
                .map(|r| {
                    [
                        r.from_origin.as_str(),
                        r.to_origin.as_str(),
                        r.destination_path.as_str(),
                    ]
                })
                .collect(),
        })
    }
    #[cfg(feature = "native-hdf5")]
    pub(crate) fn claim_checkpoint(&self) -> Result<()> {
        let mut counts = self.counts.lock().unwrap();
        if counts.wire_head != 0 || counts.wire_get != 0 || counts.checkpoint_claims != 0 {
            return fail(
                "checkpoint preparation requires a fresh caller-owned authenticated session",
            );
        }
        counts.checkpoint_claims = 1;
        Ok(())
    }
    pub fn statistics(&self) -> serde_json::Value {
        self.stats()
    }
    pub(crate) fn require_fresh(&self) -> Result<()> {
        let counts = self.counts.lock().unwrap();
        if counts.wire_head != 0 || counts.wire_get != 0 || counts.checkpoint_claims != 0 {
            return fail("job factory must return a fresh caller-owned exact-object session");
        }
        Ok(())
    }
    pub(crate) fn stats(&self) -> serde_json::Value {
        serde_json::json!(*self.counts.lock().unwrap())
    }
}
impl Response<'_> {
    pub(crate) fn read_body(&mut self, cap: u64, body: &mut Vec<u8>) -> Result<()> {
        self.owner
            .read_body(&mut self.inner, cap, body, self.cancelled, false)
    }
}

/// Deliberately limited netrc reader. No default-machine fallback or macros.
/// Reject symlinks, public files and parser errors without echoing their text.
fn credentials(path: &Path, host: &str) -> Result<HeaderValue> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| "cannot open private netrc credential source")?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect credential source")?;
    private_credential_file(&metadata)?;
    if !metadata.is_file() || metadata.len() > SESSION_BYTES as u64 {
        return fail("invalid bounded netrc credential source");
    }
    let mut text = String::new();
    file.take(SESSION_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| "invalid netrc credential encoding")?;
    if text.len() > SESSION_BYTES {
        return fail("netrc credential source exceeds bounds");
    }
    let tokens = tokenize(&text)?;
    let mut matched = false;
    let (mut user, mut password) = (None, None);
    let mut i = 0;
    while i < tokens.len() {
        let key = tokens[i].as_str();
        i += 1;
        match key {
            "machine" => {
                if matched {
                    break;
                }
                matched = tokens.get(i).is_some_and(|s| s == host);
                user = None;
                password = None;
                i += 1;
            }
            "login" | "password" | "account" => {
                let value = tokens.get(i).ok_or("invalid netrc credential syntax")?;
                i += 1;
                if matched && key == "login" {
                    user = Some(value.as_str());
                }
                if matched && key == "password" {
                    password = Some(value.as_str());
                }
            }
            "default" => {
                if matched {
                    break;
                }
            }
            _ => return fail("unsupported netrc credential syntax"),
        }
    }
    let (user, password) = (
        user.filter(|s| !s.is_empty()),
        password.filter(|s| !s.is_empty()),
    );
    let (Some(user), Some(password)) = (user, password) else {
        return fail("netrc has no explicit credentials for the configured login host");
    };
    if user.contains(':')
        || user
            .bytes()
            .chain(password.bytes())
            .any(|b| b < 32 || b == 127)
    {
        return fail("invalid netrc credential characters");
    }
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
    let mut header = HeaderValue::from_str(&format!("Basic {encoded}"))
        .map_err(|_| "invalid credential header")?;
    header.set_sensitive(true);
    Ok(header)
}
#[cfg(unix)]
fn private_credential_file(metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
        return fail("netrc credential source must be private and owned by the caller");
    }
    Ok(())
}
#[cfg(not(unix))]
fn private_credential_file(_: &std::fs::Metadata) -> Result<()> {
    fail("private netrc authentication currently requires Unix ownership and permission checks")
}
fn tokenize(input: &str) -> Result<Vec<String>> {
    let mut chars = input.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '#' {
            for x in chars.by_ref() {
                if x == '\n' {
                    break;
                }
            }
            continue;
        }
        let mut token = String::new();
        let quote = if c == '\'' || c == '"' {
            Some(c)
        } else {
            token.push(if c == '\\' {
                chars.next().ok_or("invalid netrc escape")?
            } else {
                c
            });
            None
        };
        let mut closed = quote.is_none();
        while let Some(&x) = chars.peek() {
            if quote == Some(x) {
                chars.next();
                closed = true;
                break;
            }
            if quote.is_none() && x.is_whitespace() {
                break;
            }
            chars.next();
            if x == '\\' {
                token.push(chars.next().ok_or("invalid netrc escape")?);
            } else {
                token.push(x);
            }
        }
        if !closed {
            return fail("invalid netrc quoted value");
        }
        if tokens.len() >= 1024 {
            return fail("netrc token budget exceeded");
        }
        tokens.push(token);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn netrc_fixture(label: &str) -> Result<PathBuf> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = std::env::temp_dir().join(format!(
            "tile-ripper-{label}-{}-{}.netrc",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(b"machine urs.earthdata.nasa.gov login fixture password fixture\n")?;
        Ok(path)
    }
    #[cfg(unix)]
    #[test]
    fn hls_profile_is_release_and_exact_object_scoped() -> Result<()> {
        let path = netrc_fixture("hls-profile")?;
        let l30 = "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/HLSL30.020/HLS.L30.T10SEG.2026001T000000.v2.0/HLS.L30.T10SEG.2026001T000000.v2.0.Fmask.tif";
        let session = AuthenticatedHttp::earthdata_hls(l30, &path)?;
        let identity = session.identity();
        assert_eq!(
            identity["resource_origin"],
            "https://d1nklfio7vscoe.cloudfront.net"
        );
        assert_eq!(
            identity["head_resource_path"],
            "/head-2d2df3a34830d5223d1e9547cd713408/lp-prod-protected.s3.us-west-2.amazonaws.com/HLSL30.020/HLS.L30.T10SEG.2026001T000000.v2.0/HLS.L30.T10SEG.2026001T000000.v2.0.Fmask.tif"
        );
        assert!(session.accepts(l30));
        assert!(!session.accepts(&format!("{l30}?signed=secret")));
        let s30 = l30
            .replace("HLSL30.020", "HLSS30.020")
            .replace("HLS.L30.", "HLS.S30.");
        assert!(AuthenticatedHttp::earthdata_hls(&s30, &path).is_ok());
        assert!(
            AuthenticatedHttp::earthdata_hls(&l30.replace("HLSL30.020", "HLSL30.030"), &path)
                .is_err()
        );
        assert!(AuthenticatedHttp::earthdata_hls(&format!("{l30}?signed=secret"), &path).is_err());
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn dswx_profile_is_version_layer_and_exact_object_scoped() -> Result<()> {
        let path = netrc_fixture("dswx-profile")?;
        let base = "https://archive.podaac.earthdata.nasa.gov/podaac-ops-cumulus-protected/OPERA_L3_DSWX-HLS_V1/OPERA_L3_DSWx-HLS_T17SPU_20261018T154740Z_20261022T014315Z_L8_30_v1.1";
        let wtr = format!("{base}_B01_WTR.tif");
        let session = AuthenticatedHttp::earthdata_dswx(&wtr, &path)?;
        let identity = session.identity();
        assert_eq!(
            identity["resource_origin"],
            "https://deotb6e7tfubr.cloudfront.net"
        );
        assert_eq!(
            identity["head_resource_path"],
            "/head-edaf5da92e0ce48fb61175c28b67e95d/podaac-ops-cumulus-protected.s3.us-west-2.amazonaws.com/OPERA_L3_DSWX-HLS_V1/OPERA_L3_DSWx-HLS_T17SPU_20261018T154740Z_20261022T014315Z_L8_30_v1.1_B01_WTR.tif"
        );
        assert!(session.accepts(&wtr));
        assert!(AuthenticatedHttp::earthdata_dswx(&format!("{base}_B04_DIAG.tif"), &path).is_ok());
        assert!(AuthenticatedHttp::earthdata_dswx(&format!("{base}_B10_DEM.tif"), &path).is_ok());
        for invalid in [
            format!("{wtr}?signed=secret"),
            wtr.replace("v1.1", "v1.0"),
            wtr.replace("podaac-ops-cumulus-protected", "podaac-ops-cumulus-public"),
            wtr.replace("OPERA_L3_DSWX-HLS_V1", "OPERA_L3_DSWX-HLS_V2"),
            wtr.replace("B01_WTR", "B11_UNKNOWN"),
            wtr.replace("T17SPU", "T17SP/I"),
            wtr.replace("T17SPU", "T00SPU"),
            wtr.replace("20261018T154740Z", "20261318T154740Z"),
            wtr.replace("_L8_30_", "_L7_30_"),
        ] {
            assert!(AuthenticatedHttp::earthdata_dswx(&invalid, &path).is_err());
        }
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn rtc_profile_is_version_layer_and_exact_object_scoped() -> Result<()> {
        let path = netrc_fixture("rtc-profile")?;
        let granule =
            "OPERA_L2_RTC-S1_T117-249581-IW3_20261023T171312Z_20261023T230650Z_S1C_30_v1.0";
        let base = format!(
            "https://cumulus.asf.earthdatacloud.nasa.gov/OPERA/OPERA_L2_RTC-S1/{granule}/{granule}"
        );
        let vv = format!("{base}_VV.tif");
        let session = AuthenticatedHttp::earthdata_rtc(&vv, &path)?;
        let identity = session.identity();
        assert_eq!(
            identity["resource_origin"],
            "https://d1bctmng8vmwga.cloudfront.net"
        );
        assert_eq!(
            identity["head_resource_path"],
            format!(
                "/head-b339e3a40716b5999fea2aad1aee9f03/asf-cumulus-prod-opera-products.s3.us-west-2.amazonaws.com/OPERA_L2_RTC-S1/{granule}/{granule}_VV.tif"
            )
        );
        assert!(session.accepts(&vv));
        for layer in ["VH", "HH", "HV", "mask"] {
            assert!(
                AuthenticatedHttp::earthdata_rtc(&format!("{base}_{layer}.tif"), &path).is_ok()
            );
        }
        for invalid in [
            format!("{vv}?signed=secret"),
            vv.replace("v1.0", "v1.1"),
            vv.replace("_VV.tif", "_browse.png"),
            vv.replace(
                "cumulus.asf.earthdatacloud.nasa.gov",
                "datapool.asf.alaska.edu",
            ),
            vv.replace("OPERA_L2_RTC-S1/", "OPERA_L2_RTC-S1_V2/"),
            vv.replace(&format!("/{granule}_VV.tif"), "/different_VV.tif"),
        ] {
            assert!(AuthenticatedHttp::earthdata_rtc(&invalid, &path).is_err());
        }
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn new_dist_routes_bind_collection_granule_layer_and_release() -> Result<()> {
        let path = netrc_fixture("dist-expansion")?;
        let radar_id =
            "OPERA_L3_DIST-ALERT-S1_T15UWR_20260228T235832Z_20260714T134753Z_S1C_30_v1.0";
        let annual_id = "OPERA_L3_DIST-ANN-HLS_T26XNT_2025_20260109T064522Z_30_v1";
        for (radar, id, base, layer) in [
            (true, radar_id, "https://cumulus.asf.earthdatacloud.nasa.gov/OPERA/OPERA_L3_DIST-ALERT-S1_V1", "GEN-METRIC"),
            (false, annual_id, "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/OPERA_L3_DIST-ANN-HLS_V1", "VEG-DIST-STATUS"),
        ] {
            let url = format!("{base}/{id}/{id}_{layer}.tif");
            let make = |url: &str| AuthenticatedHttp::earthdata_dist_product(url, &path, radar);
            let session = make(&url)?;
            assert!(session.accepts(&url));
            for invalid in [format!("{url}?token=secret"), format!("{url}#fragment"),
                url.replace("_V1/", "_V2/"), url.replace(&format!("/{id}_"), "/different_"),
                url.replace(&format!("_{layer}.tif"), "_UNKNOWN.tif"),
                url.replace("earthdatacloud.nasa.gov", "example.org"),
                url.replace("_30_v1", "_30_v2")] {
                assert!(make(&invalid).is_err(), "must reject {invalid}");
            }
            assert!(AuthenticatedHttp::earthdata_dist_product(&url, &path, !radar).is_err());
        }
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[cfg(all(unix, feature = "native-hdf5"))]
    #[test]
    fn checkpoint_claim_requires_an_unused_session_and_is_single_use() -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path =
            std::env::temp_dir().join(format!("tile-ripper-session-{}.netrc", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(b"machine urs.earthdata.nasa.gov login fixture password fixture\n")?;
        let url="https://archive.swot.podaac.earthdata.nasa.gov/podaac-swot-ops-cumulus-protected/SWOT_L2_HR_Raster_D/product.nc";
        let session = AuthenticatedHttp::earthdata(url, &path)?;
        session.claim_checkpoint()?;
        assert!(session.claim_checkpoint().is_err());
        let used = AuthenticatedHttp::earthdata(url, &path)?;
        used.counts.lock().unwrap().wire_head = 1;
        assert!(used.claim_checkpoint().is_err());
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[test]
    fn wave2_product_auth_is_exact_object_and_release_scoped() -> Result<()> {
        let path = netrc_fixture("wave2-profile")?;
        let radar = "https://archive.podaac.earthdata.nasa.gov/podaac-ops-cumulus-protected/OPERA_L3_DSWX-S1_V1/OPERA_L3_DSWx-S1_T17SQV_20261021T230515Z_20261023T070839Z_S1D_30_v1.0_B01_WTR.tif";
        let id = "OPERA_L3_DIST-ALERT-HLS_T17SPU_20261021T160701Z_20261023T063220Z_S2A_30_v1";
        let dist = format!("https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/OPERA_L3_DIST-ALERT-HLS_V1/{id}/{id}_VEG-DIST-STATUS.tif");
        assert!(AuthenticatedHttp::earthdata_dswx_s1(radar, &path).is_ok());
        assert!(AuthenticatedHttp::earthdata_dist_alert(&dist, &path).is_ok());
        assert!(AuthenticatedHttp::earthdata_dswx(radar, &path).is_err());
        assert!(AuthenticatedHttp::earthdata_hls(&dist, &path).is_err());
        for invalid in [
            radar.replace("DSWX-S1_V1", "DSWX-S1_V2"),
            radar.replace("v1.0", "v1.1"),
            radar.replace("B01_WTR", "B04_WTR"),
            format!("{radar}?sig=secret"),
            format!("{radar}#fragment"),
            radar.replace("nasa.gov/", "nasa.gov.evil.org/"),
        ] {
            assert!(AuthenticatedHttp::earthdata_dswx_s1(&invalid, &path).is_err());
        }
        for invalid in [
            dist.replace("_V1/", "_V2/"),
            dist.replacen(id, "other", 1),
            dist.replace("VEG-DIST-STATUS", "VEG-HIST"),
            dist.replace("DIST-ALERT", "DIST-ANN"),
            format!("{dist}?sig=secret"),
        ] {
            assert!(AuthenticatedHttp::earthdata_dist_alert(&invalid, &path).is_err());
        }
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn netrc_quotes_comments_and_invalid_values() {
        assert_eq!(
            tokenize("# secret\nmachine h login 'a b' password \"p\\\"q\"").unwrap(),
            ["machine", "h", "login", "a b", "password", "p\"q"]
        );
        assert!(tokenize("password 'unclosed").is_err());
        assert_eq!(
            tokenize(r"password \#value\ with\ spaces").unwrap(),
            ["password", "#value with spaces"]
        );
        assert!(tokenize(&"a ".repeat(1025)).is_err());
        assert!(origin("http://localhost").is_err());
        assert!(origin("https://name:secret@localhost").is_err());
        assert!(origin("https://localhost?secret").is_err());
    }
    #[test]
    fn smap_policy_is_exact_object_date_release_and_archive_scoped() -> Result<()> {
        let netrc = netrc_fixture("smap-profile")?;
        let url="https://data.nsidc.earthdatacloud.nasa.gov/nsidc-cumulus-prod-protected/SMAP/SPL3SMP_E/006/2026/10/23/SMAP_L3_SM_P_E_20261023_R19240_001.h5";
        let session = AuthenticatedHttp::earthdata_smap(url, &netrc)?;
        assert!(session.accepts(url));
        assert!(!session.accepts(&url.replace("20261023", "20261022")));
        for bad in [
            url.replace("/006/", "/007/"),
            url.replace("R19240", "R19241"),
            url.replace("/10/23/", "/10/22/"),
            url.replace(
                "data.nsidc.earthdatacloud.nasa.gov",
                "archive.podaac.earthdata.nasa.gov",
            ),
            format!("{url}?secret=token"),
        ] {
            assert!(AuthenticatedHttp::earthdata_smap(&bad, &netrc).is_err());
        }
        assert!(session
            .config
            .redirects
            .iter()
            .all(|r| r.to_origin == "https://urs.earthdata.nasa.gov"
                || r.to_origin == "https://data.nsidc.earthdatacloud.nasa.gov"
                || r.to_origin == "https://d3h5e2j7riftk6.cloudfront.net"));
        std::fs::remove_file(netrc)?;
        Ok(())
    }
    #[test]
    fn nisar_policy_cannot_inherit_another_asf_archive_or_source() -> Result<()> {
        let netrc = netrc_fixture("nisar-profile")?;
        let id="NISAR_L2_PR_GCOV_033_148_A_020_4005_DHDH_A_20261022T105546_20261022T105620_P05023_N_F_J_001";
        let url=format!("https://nisar.asf.earthdatacloud.nasa.gov/NISAR/NISAR_L2_GCOV_PROVISIONAL_V1/{id}/{id}.h5");
        let session = AuthenticatedHttp::earthdata_nisar(&url, &netrc)?;
        assert!(session.accepts(&url));
        for bad in [
            url.replace("P05023", "P05024"),
            url.replace("GCOV", "RSLC"),
            url.replace("nisar.asf.", "cumulus.asf."),
            format!("{url}?signed=secret"),
        ] {
            assert!(AuthenticatedHttp::earthdata_nisar(&bad, &netrc).is_err());
        }
        assert!(!session.accepts(&url.replace("20261022T105546", "20261022T105547")));
        std::fs::remove_file(netrc)?;
        Ok(())
    }
}
