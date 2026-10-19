//! Private format-independent object reads and prepared HTTP version bindings.
//! Zarr chunk/grid interpretation stays in its existing adapter.
use crate::{cog, fail, model::*, Result};
use reqwest::{blocking::Client, header, Method, Url};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zarrs::{
    filesystem::FilesystemStore,
    storage::{self as s, ReadableStorageTraits},
};

pub(crate) fn has_remote_zarr(r: &Request) -> bool {
    matches!(&r.source,Source::Assets{assets} if assets.iter().any(|a|a.format==Format::ZarrV2 && cog::remote(&a.locator)))
}
#[derive(Default)]
pub(crate) struct Sources(BTreeMap<String, Arc<Objects>>);
impl Sources {
    pub fn new(r: &Request) -> Result<Self> {
        if let Source::Assets { assets } = &r.source {
            if assets
                .iter()
                .filter(|a| a.format == Format::ZarrV2 && cog::remote(&a.locator))
                .count()
                > 1
            {
                return fail("one HTTP Zarr store per request is supported in this milestone");
            }
        }
        let mut sources = Self::default();
        if let Source::Assets { assets } = &r.source {
            for a in assets
                .iter()
                .filter(|a| a.format == Format::ZarrV2 && cog::remote(&a.locator))
            {
                if a.etag.is_some() {
                    return fail("HTTP groups use per-object versions, not one asset ETag");
                }
                sources.0.insert(
                    a.id.clone(),
                    crate::zarr_acquire::metadata_view(
                        Arc::new(Objects::Http(Http::new(&a.locator, &r.budget)?)),
                        &r.budget,
                    )?,
                );
            }
        }
        Ok(sources)
    }
    pub fn get(&self, a: &Asset) -> Result<Arc<Objects>> {
        if let Some(s) = self.0.get(&a.id) {
            return Ok(s.clone());
        }
        if cog::remote(&a.locator) {
            return fail("HTTP source requires prepared storage bindings");
        }
        if a.etag.is_some() {
            return fail("local Zarr cannot use an HTTP ETag");
        }
        Ok(Arc::new(Objects::Local(FilesystemStore::new(&a.locator)?)))
    }
    pub fn seal(&self) -> Result<()> {
        self.phase("prepare_validation");
        for s in self.0.values() {
            if let Some(h) = s.http() {
                h.seal()?;
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        for s in self.0.values() {
            if let Some(h) = s.http() {
                h.validate()?;
            }
        }
        Ok(())
    }
    pub fn phase(&self, phase: &'static str) {
        for source in self.0.values() {
            if let Some(http) = source.http() {
                let mut state = http.state.lock().unwrap();
                state.phase = phase;
                state.stats.phases.entry(phase.into()).or_default();
            }
        }
    }
    pub fn describe(&self, id: &str) -> serde_json::Value {
        self.0.get(id).map_or(serde_json::Value::Null, |s| {
            s.http().map_or(serde_json::Value::Null, |h| {
                serde_json::json!(h.state.lock().unwrap().versions)
            })
        })
    }
    pub fn stats(&self) -> serde_json::Value {
        serde_json::json!(self
            .0
            .iter()
            .map(|(id, source)| {
                let http = source.http().unwrap();
                let state = http.state.lock().unwrap();
                let mut stats = serde_json::json!(state.stats);
                stats["cached_metadata_bytes"] = serde_json::json!(state.cached_bytes);
                stats["metadata_overlay_bytes"] = serde_json::json!(source.overlay_bytes());
                stats["cache_limit_bytes"] = serde_json::json!(http.cap);
                stats["object_identities"] = serde_json::json!(state.versions.len());
                (id.clone(), stats)
            })
            .collect::<BTreeMap<_, _>>())
    }
}
pub(crate) enum Objects {
    Local(FilesystemStore),
    Http(Http),
    Overlay {
        origin: Arc<Objects>,
        entries: BTreeMap<String, Vec<u8>>,
    },
}
impl Objects {
    fn overlay_bytes(&self) -> usize {
        match self {
            Self::Overlay { entries, origin } => {
                entries.values().map(Vec::len).sum::<usize>() + origin.overlay_bytes()
            }
            _ => 0,
        }
    }
    fn http(&self) -> Option<&Http> {
        match self {
            Self::Http(h) => Some(h),
            Self::Overlay { origin, .. } => origin.http(),
            Self::Local(_) => None,
        }
    }
    pub fn discard_cached(&self, key: &str) {
        if let Some(h) = self.http() {
            let mut state = h.state.lock().unwrap();
            if let Some(Some(bytes)) = state.cached.remove(key) {
                state.cached_bytes -= bytes.len();
            }
        }
    }
    /// Admission-only probes cannot affect explicitly opened v2 metadata/data.
    /// Cache the answer, but do not pretend its absence is a pixel dependency.
    pub fn admission_probe(&self, key: &str) -> Result<s::MaybeBytes> {
        let answer = self.metadata_bytes(key)?;
        if let Some(http) = self.http() {
            http.state.lock().unwrap().versions.remove(key);
        }
        Ok(answer)
    }
    pub fn metadata_bytes(&self, key: &str) -> Result<s::MaybeBytes> {
        let key = s::StoreKey::new(key)?;
        if let Self::Local(store) = self {
            if store.size_key(&key)?.is_some_and(|n| n > 1024 * 1024) {
                return fail("source metadata exceeds 1 MiB");
            }
        }
        let bytes = self.get(&key)?;
        if bytes.as_ref().is_some_and(|b| b.len() > 1024 * 1024) {
            return fail("source metadata exceeds 1 MiB");
        }
        Ok(bytes)
    }
    pub fn json(&self, key: &str) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(
            &self
                .metadata_bytes(key)?
                .ok_or("required source metadata is absent")?,
        )?)
    }
    pub fn size(&self, key: &str) -> Result<Option<u64>> {
        let key = s::StoreKey::new(key)?;
        match self {
            Self::Http(h) => Ok(h.request(&key, Access::DataHead)?.map(|r| r.0.size)),
            Self::Local(s) => Ok(s.size_key(&key)?),
            Self::Overlay { origin, entries } => {
                if let Some(bytes) = entries.get(key.as_str()) {
                    Ok(Some(bytes.len() as u64))
                } else {
                    origin.size(key.as_str())
                }
            }
        }
    }
}
impl ReadableStorageTraits for Objects {
    fn get(&self, key: &s::StoreKey) -> std::result::Result<s::MaybeBytes, s::StorageError> {
        match self {
            Self::Local(s) => s.get(key),
            Self::Http(h) => h
                .get(key)
                .map_err(|e| s::StorageError::Other(e.to_string())),
            Self::Overlay { origin, entries } => {
                if let Some(bytes) = entries.get(key.as_str()) {
                    Ok(Some(bytes.clone().into()))
                } else {
                    origin.get(key)
                }
            }
        }
    }
    fn get_partial_many<'a>(
        &'a self,
        key: &s::StoreKey,
        ranges: s::byte_range::ByteRangeIterator<'a>,
    ) -> std::result::Result<s::MaybeBytesIterator<'a>, s::StorageError> {
        // Supported codecs retrieve complete objects. Keep the standard library
        // range fallback available without adding another network transport.
        let Some(bytes) = self.get(key)? else {
            return Ok(None);
        };
        let mut out = Vec::new();
        for range in ranges {
            let start = usize::try_from(range.start(bytes.len() as u64))
                .map_err(|_| s::StorageError::Other("invalid byte range".into()))?;
            let end = usize::try_from(range.end(bytes.len() as u64))
                .map_err(|_| s::StorageError::Other("invalid byte range".into()))?;
            if start > end || end > bytes.len() {
                return Err(s::StorageError::Other("invalid byte range".into()));
            }
            out.push(Ok(bytes.slice(start..end)));
        }
        Ok(Some(Box::new(out.into_iter())))
    }
    fn size_key(&self, key: &s::StoreKey) -> std::result::Result<Option<u64>, s::StorageError> {
        if let Some(http) = self.http() {
            let state = http
                .state
                .lock()
                .map_err(|_| s::StorageError::Other("HTTP lock poisoned".into()))?;
            // HTTP get already bounds the body. Do not introduce an extra
            // planning HEAD that would classify coordinate bodies as data.
            return Ok(state
                .versions
                .get(key.as_str())
                .and_then(|v| v.as_ref().map(|v| v.size)));
        }
        self.size(key.as_str())
            .map_err(|e| s::StorageError::Other(e.to_string()))
    }
    fn supports_get_partial(&self) -> bool {
        false
    }
}
#[derive(Clone, PartialEq, Serialize)]
struct Version {
    etag: String,
    size: u64,
    metadata: bool,
}
#[derive(Default, Serialize)]
struct Counts {
    seconds: f64,
    attempts: u64,
    bytes: u64,
    retries: u64,
    statuses: BTreeMap<u16, u64>,
}
#[derive(Default, Serialize)]
struct Stats {
    phases: BTreeMap<String, BTreeMap<String, Counts>>,
    requests: u64,
    bytes: u64,
    retries: u64,
    seconds: f64,
    objects: BTreeMap<String, Counts>,
}
#[derive(Default)]
struct State {
    phase: &'static str,
    sealed: bool,
    cached: BTreeMap<String, Option<Vec<u8>>>,
    cached_bytes: usize,
    versions: BTreeMap<String, Option<Version>>,
    stats: Stats,
}
pub(crate) struct Http {
    base: Url,
    client: Client,
    cap: usize,
    nodes: usize,
    state: Mutex<State>,
    network: Mutex<()>,
}
#[derive(Clone, Copy)]
enum Access {
    Get,
    MetadataHead,
    DataHead,
}
impl Http {
    fn new(url: &str, b: &Budget) -> Result<Self> {
        let mut base = Url::parse(url).map_err(|_| "invalid HTTP source URL")?;
        if !["http", "https"].contains(&base.scheme())
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return fail("HTTP Zarr requires a public URL without credentials, query or fragment");
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            base,
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .pool_max_idle_per_host(1)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("tile-ripper/0.1 bounded-acquisition")
                .build()
                .map_err(|_| "cannot initialize HTTP client")?,
            cap: b.buffer_bytes / 2,
            nodes: b.max_nodes,
            state: Mutex::new(State {
                phase: "planning",
                ..State::default()
            }),
            network: Mutex::new(()),
        })
    }
    fn get(&self, key: &s::StoreKey) -> Result<s::MaybeBytes> {
        if let Some(b) = self.state.lock().unwrap().cached.get(key.as_str()).cloned() {
            return Ok(b.map(Into::into));
        }
        // Absent data objects were pinned by HEAD and are revalidated before and
        // after execution. No redundant GET is needed to synthesize native fill.
        {
            let state = self.state.lock().unwrap();
            if state.sealed && state.versions.get(key.as_str()) == Some(&None) {
                return Ok(None);
            }
        }
        let result = self.request(key, Access::Get)?;
        let bytes = result.map(|r| r.1);
        let mut state = self.state.lock().unwrap();
        if !state.sealed {
            let size = bytes.as_ref().map_or(0, Vec::len);
            if state
                .cached_bytes
                .checked_add(size)
                .is_none_or(|n| n > self.cap)
                || state.cached.len() >= self.nodes
            {
                return fail("HTTP metadata/coordinate cache exceeds buffer or max_nodes budget");
            }
            state.cached_bytes += size;
            state.cached.insert(key.to_string(), bytes.clone());
        }
        Ok(bytes.map(Into::into))
    }
    fn seal(&self) -> Result<()> {
        self.state.lock().unwrap().sealed = true;
        self.validate()
    }
    fn validate(&self) -> Result<()> {
        let keys: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .versions
            .iter()
            .filter(|(_, v)| v.as_ref().is_none_or(|v| v.metadata))
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            self.request(&s::StoreKey::new(key)?, Access::MetadataHead)?;
        }
        Ok(())
    }
    fn request(&self, key: &s::StoreKey, access: Access) -> Result<Option<(Version, Vec<u8>)>> {
        let head = !matches!(access, Access::Get);
        let data_head = matches!(access, Access::DataHead);
        let _network = self.network.lock().map_err(|_| "HTTP lock poisoned")?;
        let key_string = key.as_str();
        let url = self
            .base
            .join(key_string)
            .map_err(|_| "invalid relative object key")?;
        if url.origin() != self.base.origin() || !url.path().starts_with(self.base.path()) {
            return fail("object key escapes source prefix");
        }
        let (known, metadata) = {
            let state = self.state.lock().unwrap();
            (
                state.versions.get(key_string).cloned(),
                !data_head && !state.sealed,
            )
        };
        if !head && self.state.lock().unwrap().sealed && known.is_none() {
            return fail("execution attempted an unplanned HTTP object");
        }
        for attempt in 0..3 {
            let started = Instant::now();
            let mut req = self
                .client
                .request(if head { Method::HEAD } else { Method::GET }, url.clone());
            if let Some(expected) = &known {
                req = match expected {
                    Some(v) => req.header(header::IF_MATCH, &v.etag),
                    None => req.header(header::IF_NONE_MATCH, "*"),
                };
            }
            let mut retry_after = None;
            let mut status = 0;
            let mut body = Vec::new();
            let result = (|| -> Result<Option<Version>> {
                let mut response = req.send().map_err(|_| "retryable HTTP transport failure")?;
                status = response.status().as_u16();
                if [408, 429, 500, 502, 503, 504].contains(&status) {
                    retry_after = response
                        .headers()
                        .get(header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    return fail("retryable HTTP status");
                }
                if status == 412 {
                    return fail("HTTP source version conflict; prepare again");
                }
                if status == 404 {
                    if known.as_ref().is_some_and(Option::is_some) {
                        return fail("HTTP source disappeared since preparation");
                    }
                    return Ok(None);
                }
                if status != 200 {
                    return fail("HTTP object request rejected (unsupported status or redirect)");
                }
                if known == Some(None) {
                    return fail("previously missing HTTP object appeared; prepare again");
                }
                let etag = response
                    .headers()
                    .get(header::ETAG)
                    .and_then(|v| v.to_str().ok())
                    .filter(|v| v.starts_with('"') && v.ends_with('"') && !v.starts_with("W/"))
                    .ok_or("HTTP source requires strong ETags")?
                    .to_owned();
                let length = response
                    .headers()
                    .get(header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                if length.is_some_and(|n| n > self.cap as u64) {
                    return fail("HTTP object exceeds half buffer_bytes");
                }
                if !head {
                    response
                        .by_ref()
                        .take(self.cap as u64 + 1)
                        .read_to_end(&mut body)
                        .map_err(|_| "retryable interrupted HTTP body")?;
                    if body.len() > self.cap {
                        return fail("HTTP object exceeds half buffer_bytes");
                    }
                    if length.is_some_and(|n| n != body.len() as u64) {
                        return fail("retryable interrupted HTTP body");
                    }
                }
                let size = if head {
                    length.ok_or("HTTP HEAD requires Content-Length")?
                } else {
                    body.len() as u64
                };
                let v = Version {
                    etag,
                    size,
                    metadata: known
                        .as_ref()
                        .and_then(|v| v.as_ref())
                        .map_or(metadata, |v| v.metadata),
                };
                if known
                    .as_ref()
                    .and_then(|v| v.as_ref())
                    .is_some_and(|old| old != &v)
                {
                    return fail("HTTP source version conflict; prepare again");
                }
                Ok(Some(v))
            })();
            {
                let mut state = self.state.lock().unwrap();
                state.stats.requests += 1;
                state.stats.bytes += body.len() as u64;
                state.stats.seconds += started.elapsed().as_secs_f64();
                let phase = state.phase.to_string();
                let phase_entry = state
                    .stats
                    .phases
                    .entry(phase)
                    .or_default()
                    .entry(format!(
                        "{} {}",
                        if head { "HEAD" } else { "GET" },
                        key_string
                    ))
                    .or_default();
                phase_entry.seconds += started.elapsed().as_secs_f64();
                phase_entry.attempts += 1;
                phase_entry.bytes += body.len() as u64;
                *phase_entry.statuses.entry(status).or_default() += 1;
                if attempt > 0 {
                    phase_entry.retries += 1;
                }
                let entry = state
                    .stats
                    .objects
                    .entry(format!(
                        "{} {}",
                        if head { "HEAD" } else { "GET" },
                        key_string
                    ))
                    .or_default();
                entry.seconds += started.elapsed().as_secs_f64();
                entry.attempts += 1;
                entry.bytes += body.len() as u64;
                *entry.statuses.entry(status).or_default() += 1;
                if attempt > 0 {
                    entry.retries += 1;
                    state.stats.retries += 1;
                }
            }
            match result {
                Ok(version) => {
                    let mut state = self.state.lock().unwrap();
                    if !state.versions.contains_key(key_string)
                        && state.versions.len() >= self.nodes
                    {
                        return fail("HTTP object identity budget exceeds max_nodes");
                    }
                    state.versions.insert(key_string.into(), version.clone());
                    return Ok(version.map(|v| (v, body)));
                }
                Err(e) if e.to_string().starts_with("retryable") && attempt < 2 => {
                    let wait = if let Some(value) = retry_after {
                        retry_delay(&value)?
                    } else {
                        Duration::from_millis(25 * (1 << attempt))
                    };
                    std::thread::sleep(wait);
                }
                Err(e) => {
                    if e.to_string().starts_with("retryable") {
                        return fail(&format!(
                            "HTTP retry budget exhausted after 3 attempts (last status {status})"
                        ));
                    }
                    return Err(e);
                }
            }
        }
        fail("HTTP retries exhausted")
    }
}

fn retry_delay(value: &str) -> Result<Duration> {
    let seconds = if let Ok(seconds) = value.parse::<u64>() {
        seconds
    } else {
        let date = chrono::DateTime::parse_from_rfc2822(value)
            .map_err(|_| "unsupported Retry-After value")?
            .timestamp();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        date.saturating_sub(now).max(0) as u64
    };
    if seconds > 2 {
        return fail("Retry-After exceeds bounded retry window; retry acquisition later");
    }
    Ok(Duration::from_secs(seconds))
}
#[cfg(test)]
mod tests {
    #[test]
    fn retry_after_respects_seconds_dates_and_bounded_waits() {
        assert_eq!(super::retry_delay("1").unwrap().as_secs(), 1);
        assert_eq!(
            super::retry_delay("Tue, 15 Nov 1994 08:12:31 GMT")
                .unwrap()
                .as_secs(),
            0
        );
        assert!(super::retry_delay("60").is_err());
        assert!(super::retry_delay("not-a-date").is_err());
    }
}
