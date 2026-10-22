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
    pub fn statistics(&self) -> serde_json::Value {
        self.stats()
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
}
