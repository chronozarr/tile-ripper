//! Private format-independent object reads and prepared HTTP version bindings.
//! Zarr chunk/grid interpretation stays in its existing adapter.
use crate::{cog, fail, model::*, Result};
use reqwest::{blocking::Client, header, Method, Url};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use zarrs::{
    filesystem::FilesystemStore,
    storage::{self as s, ReadableStorageTraits},
};

pub(crate) fn requires_zarr_bindings(r: &Request) -> bool {
    matches!(&r.source,Source::Assets{assets} if assets.iter().any(|a|a.format == Format::Hdf5 || a.format == Format::ZarrV3 || (a.format == Format::ZarrV2 && cog::remote(&a.locator))))
}
#[derive(Default)]
pub(crate) struct Sources {
    objects: BTreeMap<String, Arc<Objects>>,
    #[cfg(feature = "native-cog")]
    pub(crate) native: crate::native_cog::Snapshots,
    pub(crate) v3: crate::zarr_v3::Context,
    #[cfg(feature = "native-hdf5")]
    pub(crate) hdf5: crate::hdf5::Context,
}

impl Sources {
    pub(crate) fn validate_cog_identities(
        &self,
        request: &Request,
        limits: &crate::stac_workflow::Limits,
    ) -> Result<()> {
        #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
        if self.native.transport_reservation()? > 0 {
            return self.native.validate();
        }
        #[cfg(feature = "native-cog")]
        self.native.validate()?;
        let result = crate::stac_workflow::validate_identities(request, limits);
        #[cfg(feature = "native-cog")]
        {
            if result.is_err() {
                self.native.invalidate();
            } else {
                self.native.validate()?;
            }
        }
        result
    }
    #[cfg(feature = "native-hdf5")]
    pub(crate) fn from_hdf5(hdf5: crate::hdf5::Context) -> Self {
        Self {
            hdf5,
            ..Default::default()
        }
    }
    pub fn new(r: &Request) -> Result<Self> {
        Self::with_transport(r, Default::default())
    }
    pub(crate) fn with_transport(
        r: &Request,
        transport: crate::http_transport::Transport,
    ) -> Result<Self> {
        let mut sources = Self::default();
        #[cfg(feature = "native-hdf5")]
        {
            sources.hdf5.transport = transport.clone();
        }
        let _ = transport;
        #[cfg(feature = "native-cog")]
        {
            sources.native = crate::native_cog::Snapshots::new(r.budget.buffer_bytes);
        }
        let mut shared_client: Option<Arc<Client>> = None;
        let metadata_budget = Arc::new(MetadataBudget::new(r.budget.buffer_bytes / 2));
        if let Source::Assets { assets } = &r.source {
            for a in assets.iter().filter(|a| {
                a.format == Format::ZarrV3
                    || (a.format == Format::ZarrV2 && cog::remote(&a.locator))
            }) {
                if a.format == Format::ZarrV3 && !cog::remote(&a.locator) {
                    if a.etag.is_some() {
                        return fail("local Zarr cannot use an HTTP ETag");
                    }
                    sources.objects.insert(
                        a.id.clone(),
                        crate::zarr_v3::metadata_view(
                            Arc::new(Objects::Local(FilesystemStore::new(&a.locator)?)),
                            &r.budget,
                        )?,
                    );
                    continue;
                }
                if a.etag.is_some() {
                    return fail("HTTP groups use per-object versions, not one asset ETag");
                }
                let client = match &shared_client {
                    Some(client) => client.clone(),
                    None => {
                        let client = Arc::new(Http::build_client()?);
                        shared_client = Some(client.clone());
                        client
                    }
                };
                let mut http = Http::new(
                    &a.locator,
                    &r.budget,
                    client.clone(),
                    metadata_budget.clone(),
                )?;
                http.validate_data = a.format == Format::ZarrV3;
                let source = Arc::new(Objects::Http(http));
                let source = if a.format == Format::ZarrV3 {
                    crate::zarr_v3::metadata_view(source, &r.budget)?
                } else {
                    crate::zarr_acquire::metadata_view(source, &r.budget)?
                };
                sources.objects.insert(a.id.clone(), source);
            }
        }
        if metadata_budget.used() > metadata_budget.cap {
            return fail("aggregate HTTP metadata/coordinate cache exceeds half buffer_bytes");
        }
        Ok(sources)
    }
    #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
    pub(crate) fn with_cog_sessions(
        r: &Request,
        sessions: BTreeMap<String, Arc<crate::authenticated_http::AuthenticatedHttp>>,
    ) -> Result<Self> {
        let mut sources = Self::with_transport(r, Default::default())?;
        sources.native =
            crate::native_cog::Snapshots::with_sessions(r.budget.buffer_bytes, sessions)?;
        Ok(sources)
    }
    #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
    pub(crate) fn resolve_authenticated_cog_identities(&self, request: &mut Request) -> Result<()> {
        let Source::Assets { assets } = &mut request.source else {
            return fail("authenticated COG identity resolution requires explicit assets");
        };
        self.native.resolve_authenticated(assets)?;
        Ok(())
    }
    pub fn get(&self, a: &Asset) -> Result<Arc<Objects>> {
        if let Some(s) = self.objects.get(&a.id) {
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
        #[cfg(feature = "native-cog")]
        self.native.validate()?;
        #[cfg(feature = "native-hdf5")]
        self.hdf5.validate()?;
        self.v3.validate()?;
        self.phase("prepare_validation");
        for s in self.objects.values() {
            if let Some(h) = s.http() {
                h.seal()?;
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_cancellable(None)
    }
    pub fn validate_cancellable(
        &self,
        _cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        #[cfg(feature = "native-cog")]
        self.native.validate()?;
        #[cfg(feature = "native-hdf5")]
        self.hdf5.validate_cancellable(_cancelled)?;
        self.v3.validate()?;
        for s in self.objects.values() {
            if let Some(h) = s.http() {
                h.validate()?;
            }
        }
        Ok(())
    }
    pub fn phase(&self, phase: &'static str) {
        #[cfg(feature = "native-hdf5")]
        self.hdf5.phase(phase);
        for source in self.objects.values() {
            if let Some(http) = source.http() {
                let mut state = http.state.lock().unwrap();
                state.phase = phase;
                state.stats.phases.entry(phase.into()).or_default();
            }
        }
    }
    pub fn describe(&self, id: &str) -> serde_json::Value {
        #[cfg(feature = "native-hdf5")]
        if let Some(value) = self.hdf5.describe(id) {
            return value;
        }
        #[cfg(feature = "native-cog")]
        if let Some(value) = self.native.describe(id) {
            return serde_json::json!({"native_cog":value});
        }
        self.objects.get(id).map_or(serde_json::Value::Null, |s| {
            s.http().map_or(serde_json::Value::Null, |h| {
                serde_json::json!(h.state.lock().unwrap().versions)
            })
        })
    }
    pub(crate) fn v3_prepared_bytes(&self) -> usize {
        if self.v3.bytes() == 0 {
            return 0;
        }
        self.v3.bytes()
            + self
                .objects
                .values()
                .map(|s| {
                    s.overlay_bytes() + s.http().map_or(0, |h| h.state.lock().unwrap().cached_bytes)
                })
                .sum::<usize>()
    }
    pub(crate) fn transport_reservation(&self) -> Result<usize> {
        #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
        {
            self.native.transport_reservation()
        }
        #[cfg(not(all(feature = "native-cog", feature = "authenticated-http")))]
        Ok(0)
    }
    pub fn stats(&self) -> serde_json::Value {
        #[allow(unused_mut)]
        let mut value = serde_json::json!(self
            .objects
            .iter()
            .filter_map(|(id, source)| {
                let http = source.http()?;
                let state = http.state.lock().unwrap();
                let mut stats = serde_json::json!(state.stats);
                stats["cached_metadata_bytes"] = serde_json::json!(state.cached_bytes);
                stats["metadata_overlay_bytes"] = serde_json::json!(source.overlay_bytes());
                stats["cache_limit_bytes"] = serde_json::json!(http.cap);
                stats["object_identities"] = serde_json::json!(state.versions.len());
                Some((id.clone(), stats))
            })
            .collect::<BTreeMap<_, _>>());
        #[cfg(feature = "native-hdf5")]
        self.hdf5.add_stats(&mut value);
        #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
        if self.native.transport_reservation().unwrap_or(0) > 0 {
            value["native_cog_authenticated"] = self.native.transport_stats();
        }
        value
    }
}
pub(crate) struct MetadataBudget {
    cap: usize,
    used: AtomicUsize,
}
impl MetadataBudget {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            used: AtomicUsize::new(0),
        }
    }
    fn reserve(&self, bytes: usize) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= self.cap)
            })
            .is_ok()
    }
    fn release(&self, bytes: usize) {
        let previous = self.used.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes);
    }
    fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
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
    /// Bounded physical ranges for the v3 adapter. V2 keeps its established
    /// whole-object trait behavior, including its accounting and cache policy.
    pub(crate) fn ranges(
        &self,
        key: &s::StoreKey,
        ranges: &[s::byte_range::ByteRange],
    ) -> Result<Option<Vec<s::Bytes>>> {
        match self {
            Self::Http(http) => {
                let size = self
                    .size_key(key)?
                    .ok_or("range target is absent or unpinned")?;
                let mut out = Vec::new();
                for range in ranges {
                    let (start, end) = match *range {
                        s::byte_range::ByteRange::FromStart(start, length) => (
                            start,
                            length
                                .map_or(Some(size), |n| start.checked_add(n))
                                .ok_or("byte range overflow")?,
                        ),
                        s::byte_range::ByteRange::Suffix(length) => (
                            size.checked_sub(length)
                                .ok_or("index exceeds object size")?,
                            size,
                        ),
                    };
                    if start >= end || end > size {
                        return fail("invalid bounded object range");
                    }
                    let (_, bytes) = http
                        .request(key, Access::Range { start, end })?
                        .ok_or("bound range target disappeared")?;
                    out.push(bytes.into());
                }
                Ok(Some(out))
            }
            Self::Local(store) => {
                let Some(values) = store.get_partial_many(key, Box::new(ranges.iter().copied()))?
                else {
                    return Ok(None);
                };
                Ok(Some(values.collect::<std::result::Result<Vec<_>, _>>()?))
            }
            Self::Overlay { origin, .. } => origin.ranges(key, ranges),
        }
    }
    pub(crate) fn remote(&self) -> bool {
        self.http().is_some()
    }
    pub(crate) fn identity(&self, key: &str) -> serde_json::Value {
        self.http().map_or_else(|| serde_json::json!({"kind":"local_pinned_stat"}), |h| {
            serde_json::json!({"url":h.base.join(key).ok().map(|u|u.to_string()),"version":h.state.lock().unwrap().versions.get(key),"authenticated_scope":h.transport.identity()})
        })
    }
    pub(crate) fn local_path(&self, key: &s::StoreKey) -> Option<std::path::PathBuf> {
        match self {
            Self::Local(store) => Some(store.key_to_fspath(key)),
            Self::Overlay { origin, .. } => origin.local_path(key),
            Self::Http(_) => None,
        }
    }
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
    pub(crate) fn reserve_overlay_cache(&self, bytes: usize) -> Result<()> {
        if let Some(http) = self.http() {
            if !http.metadata_budget.reserve(bytes) {
                return fail("aggregate HTTP metadata/coordinate cache exceeds half buffer_bytes");
            }
        }
        Ok(())
    }
    pub fn discard_cached(&self, key: &str) {
        if let Some(h) = self.http() {
            let mut state = h.state.lock().unwrap();
            if let Some(Some(bytes)) = state.cached.remove(key) {
                state.cached_bytes -= bytes.len();
                h.metadata_budget.release(bytes.len());
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
    pub fn sizes(&self, keys: &[String]) -> Result<Vec<Option<u64>>> {
        match self {
            Self::Local(_) => keys.iter().map(|key| self.size(key)).collect(),
            Self::Http(http) => http.sizes(keys),
            Self::Overlay { origin, entries } => {
                let mut unresolved = Vec::new();
                let mut positions = Vec::new();
                let mut result = vec![None; keys.len()];
                for (i, key) in keys.iter().enumerate() {
                    if let Some(bytes) = entries.get(key) {
                        result[i] = Some(bytes.len() as u64);
                    } else {
                        unresolved.push(key.clone());
                        positions.push(i);
                    }
                }
                for (i, size) in positions.into_iter().zip(origin.sizes(&unresolved)?) {
                    result[i] = size;
                }
                Ok(result)
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
    ranges: BTreeMap<String, Counts>,
}
#[derive(Default)]
struct State {
    phase: &'static str,
    sealed: bool,
    failed: bool,
    cached: BTreeMap<String, Option<Vec<u8>>>,
    cached_bytes: usize,
    versions: BTreeMap<String, Option<Version>>,
    stats: Stats,
}
pub(crate) struct Http {
    base: Url,
    client: Arc<Client>,
    transport: crate::http_transport::Transport,
    cap: usize,
    nodes: usize,
    state: Mutex<State>,
    metadata_budget: Arc<MetadataBudget>,
    validate_data: bool,
}
#[derive(Clone, Copy)]
enum Access {
    Get,
    MetadataHead,
    DataHead,
    Range { start: u64, end: u64 },
}
impl Http {
    fn build_client() -> Result<Client> {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(64)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("tile-ripper/0.1 bounded-acquisition")
            .build()
            .map_err(|_| "cannot initialize HTTP client".into())
    }
    fn new(
        url: &str,
        b: &Budget,
        client: Arc<Client>,
        metadata_budget: Arc<MetadataBudget>,
    ) -> Result<Self> {
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
            client,
            transport: Default::default(),
            cap: b.buffer_bytes / 2,
            nodes: b.max_nodes,
            state: Mutex::new(State {
                phase: "planning",
                ..State::default()
            }),
            metadata_budget,
            validate_data: false,
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
            if let Some(existing) = state.cached.get(key.as_str()) {
                return Ok(existing.clone().map(Into::into));
            }
            if state
                .cached_bytes
                .checked_add(size)
                .is_none_or(|n| n > self.cap)
                || state.cached.len() >= self.nodes
            {
                return fail("HTTP metadata/coordinate cache exceeds buffer or max_nodes budget");
            }
            if !self.metadata_budget.reserve(size) {
                return fail("aggregate HTTP metadata/coordinate cache exceeds half buffer_bytes");
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
        self.validate_cancellable(None)
    }
    fn validate_cancellable(
        &self,
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        let keys: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .versions
            .iter()
            .filter(|(_, v)| self.validate_data || v.as_ref().is_none_or(|v| v.metadata))
            .map(|(k, _)| k.clone())
            .collect();
        parallel_map(&keys, |key| {
            self.request_cancellable(&s::StoreKey::new(key)?, Access::MetadataHead, cancelled)
                .map(|_| ())
        })?;
        Ok(())
    }
    fn sizes(&self, keys: &[String]) -> Result<Vec<Option<u64>>> {
        parallel_map(keys, |key| {
            self.request(&s::StoreKey::new(key)?, Access::DataHead)
                .map(|result| result.map(|(version, _)| version.size))
        })
    }
    fn request(&self, key: &s::StoreKey, access: Access) -> Result<Option<(Version, Vec<u8>)>> {
        self.request_cancellable(key, access, None)
    }
    fn request_cancellable(
        &self,
        key: &s::StoreKey,
        access: Access,
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<Option<(Version, Vec<u8>)>> {
        if self.state.lock().unwrap().failed {
            return fail("HTTP source binding invalidated by an earlier failure; prepare again");
        }
        let head = matches!(access, Access::MetadataHead | Access::DataHead);
        let data_head = matches!(access, Access::DataHead);
        let range = match access {
            Access::Range { start, end } => Some((start, end)),
            _ => None,
        };
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
        if let Some((start, end)) = range {
            let expected = known
                .as_ref()
                .and_then(|v| v.as_ref())
                .ok_or("range requires a pinned object version")?;
            if !self.validate_data
                || start >= end
                || end > expected.size
                || end - start > self.cap as u64
            {
                return fail("range exceeds admitted object or buffer bounds");
            }
        }
        for attempt in 0..3 {
            let started = Instant::now();
            if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
                return fail("acquisition cancelled");
            }
            let mut headers = header::HeaderMap::new();
            headers.insert(
                header::ACCEPT_ENCODING,
                header::HeaderValue::from_static("identity"),
            );
            if let Some((start, end)) = range {
                headers.insert(header::RANGE, format!("bytes={start}-{}", end - 1).parse()?);
            }
            if let Some(expected) = &known {
                match expected {
                    Some(v) => {
                        headers.insert(header::IF_MATCH, v.etag.parse()?);
                    }
                    None => {
                        headers
                            .insert(header::IF_NONE_MATCH, header::HeaderValue::from_static("*"));
                    }
                }
            }
            let mut retry_after = None;
            let mut status = 0;
            let mut body = Vec::new();
            let result = (|| -> Result<Option<Version>> {
                let mut response = self.transport.send(
                    &self.client,
                    if head { Method::HEAD } else { Method::GET },
                    url.clone(),
                    headers,
                    cancelled,
                )?;
                status = response.status();
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
                if status != if range.is_some() { 206 } else { 200 } {
                    return fail("HTTP object request rejected (unsupported status or redirect)");
                }
                if known == Some(None) {
                    return fail("previously missing HTTP object appeared; prepare again");
                }
                if (range.is_some() || self.transport.authenticated())
                    && response
                        .headers()
                        .get(header::CONTENT_ENCODING)
                        .is_some_and(|v| v != "identity")
                {
                    return fail("encoded HTTP range responses are unsupported");
                }
                let etag = response
                    .headers()
                    .get(header::ETAG)
                    .and_then(|v| v.to_str().ok())
                    .filter(|v| strong_etag(v))
                    .ok_or("HTTP source requires strong ETags")?
                    .to_owned();
                let length = response
                    .headers()
                    .get(header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                if length.is_some_and(|n| n > self.cap as u64)
                    && !(head
                        && self.validate_data
                        && (data_head
                            || known
                                .as_ref()
                                .and_then(|v| v.as_ref())
                                .is_some_and(|v| !v.metadata)))
                {
                    return fail("HTTP object exceeds half buffer_bytes");
                }
                // Reject framing/version conflicts before allocating a response body.
                if let Some((start, end)) = range {
                    let expected = known
                        .as_ref()
                        .and_then(|v| v.as_ref())
                        .expect("admitted range");
                    let framing = format!("bytes {start}-{}/{}", end - 1, expected.size);
                    if response
                        .headers()
                        .get(header::CONTENT_RANGE)
                        .and_then(|v| v.to_str().ok())
                        != Some(framing.as_str())
                        || length != Some(end - start)
                    {
                        return fail("invalid HTTP range framing");
                    }
                    if etag != expected.etag {
                        return fail("HTTP source version conflict; prepare again");
                    }
                    body.reserve_exact((end - start) as usize + 1);
                }
                if !head {
                    response.read_body(range.map_or(self.cap as u64, |(s, e)| e - s), &mut body)?;
                    if body.len() > self.cap {
                        return fail("HTTP object exceeds half buffer_bytes");
                    }
                    if length.is_some_and(|n| n != body.len() as u64) {
                        return fail("retryable interrupted HTTP body");
                    }
                }
                let size = if let Some((start, end)) = range {
                    let expected = known
                        .as_ref()
                        .and_then(|v| v.as_ref())
                        .expect("admitted range");
                    let content_range = format!("bytes {start}-{}/{}", end - 1, expected.size);
                    if response
                        .headers()
                        .get(header::CONTENT_RANGE)
                        .and_then(|v| v.to_str().ok())
                        != Some(content_range.as_str())
                        || length != Some(end - start)
                        || body.len() as u64 != end - start
                    {
                        return fail("invalid HTTP range framing");
                    }
                    expected.size
                } else if head {
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
                if let Some((start, end)) = range {
                    let entry = state
                        .stats
                        .ranges
                        .entry(format!("{key_string} {start}:{end}"))
                        .or_default();
                    entry.attempts += 1;
                    entry.bytes += body.len() as u64;
                    entry.seconds += started.elapsed().as_secs_f64();
                    *entry.statuses.entry(status).or_default() += 1;
                    if attempt > 0 {
                        entry.retries += 1;
                    }
                }
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
                    if state.failed {
                        return fail(
                            "HTTP source binding invalidated by an earlier failure; prepare again",
                        );
                    }
                    if state
                        .versions
                        .get(key_string)
                        .is_some_and(|current| current != &version)
                    {
                        state.failed = true;
                        return fail("HTTP source version conflict; prepare again");
                    }
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
                    let until = Instant::now() + wait;
                    while Instant::now() < until {
                        if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
                            return fail("acquisition cancelled");
                        }
                        std::thread::sleep(
                            until
                                .saturating_duration_since(Instant::now())
                                .min(Duration::from_millis(20)),
                        );
                    }
                }
                Err(e) => {
                    self.state.lock().unwrap().failed = true;
                    if e.to_string().starts_with("retryable") {
                        return fail(&format!(
                            "HTTP retry budget exhausted after 3 attempts (last status {status}; {e})"
                        ));
                    }
                    return Err(e);
                }
            }
        }
        fail("HTTP retries exhausted")
    }
}

pub(crate) fn worker_count() -> Result<usize> {
    fn configured(name: &str, default: usize) -> Result<usize> {
        let value = match std::env::var(name) {
            Ok(value) => value
                .parse::<usize>()
                .map_err(|_| format!("{name} must be an integer from 1 to 64"))?,
            Err(std::env::VarError::NotPresent) => default,
            Err(std::env::VarError::NotUnicode(_)) => {
                return fail(&format!("{name} must be an integer from 1 to 64"));
            }
        };
        if !(1..=64).contains(&value) {
            return fail(&format!("{name} must be from 1 to 64"));
        }
        Ok(value)
    }

    let cpus = thread::available_parallelism().map_or(1, usize::from);
    Ok(configured("TILE_RIPPER_WORKERS", cpus.min(2))?.min(configured("TILE_RIPPER_PER_HOST", 4)?))
}

/// Execute each distinct key once with bounded scoped workers and restore the
/// caller's order, including duplicate positions in the requested result.
pub(crate) fn parallel_map<T, F>(keys: &[String], f: F) -> Result<Vec<T>>
where
    T: Clone + Send,
    F: Fn(&str) -> Result<T> + Sync,
{
    let mut unique = Vec::<String>::new();
    let mut positions = Vec::with_capacity(keys.len());
    let mut indices = std::collections::HashMap::<String, usize>::new();
    for key in keys {
        let index = *indices.entry(key.clone()).or_insert_with(|| {
            unique.push(key.clone());
            unique.len() - 1
        });
        positions.push(index);
    }
    if unique.is_empty() {
        return Ok(Vec::new());
    }
    let workers = worker_count()?.min(unique.len());
    let next = AtomicUsize::new(0);
    let values = Mutex::new(
        std::iter::repeat_with(|| None)
            .take(unique.len())
            .collect::<Vec<Option<Result<T>>>>(),
    );
    let panicked = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let next = &next;
            let values = &values;
            let unique = &unique;
            let f = &f;
            handles.push(scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= unique.len() {
                    break;
                }
                let result = f(&unique[i]);
                values.lock().unwrap()[i] = Some(result);
            }));
        }
        handles.into_iter().any(|handle| handle.join().is_err())
    });
    if panicked {
        return fail("bounded HTTP worker panicked");
    }
    let distinct = values
        .into_inner()
        .map_err(|_| "bounded HTTP result lock poisoned")?
        .into_iter()
        .map(|value| value.ok_or("bounded HTTP worker omitted a key")?)
        .collect::<Result<Vec<_>>>()?;
    let mut ordered = Vec::with_capacity(positions.len());
    for index in positions {
        ordered.push(distinct[index].clone());
    }
    Ok(ordered)
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
/// One file over the same pinned, conditional exact-range transport as v3.
#[cfg(feature = "native-hdf5")]
pub(crate) struct ObjectFile {
    objects: Objects,
    key: s::StoreKey,
    pub size: u64,
    local_version: Option<(std::path::PathBuf, serde_json::Value)>,
}
#[cfg(feature = "native-hdf5")]
fn local_file_version(path: &std::path::Path) -> Result<serde_json::Value> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return fail("HDF5 local source must be an ordinary file");
    }
    let mut version = serde_json::json!({"size":metadata.len(),
        "modified":metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos().to_string()});
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        version["unix"] = serde_json::json!([
            metadata.dev(),
            metadata.ino(),
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64
        ]);
    }
    Ok(version)
}
#[cfg(feature = "native-hdf5")]
impl ObjectFile {
    pub fn open(
        asset: &Asset,
        budget: &Budget,
        transport: crate::http_transport::Transport,
    ) -> Result<Self> {
        let (objects, key) = if cog::remote(&asset.locator) {
            let mut url = Url::parse(&asset.locator)?;
            let filename = url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .ok_or("missing file name")?
                .to_owned();
            if filename.is_empty() || url.query().is_some() || url.fragment().is_some() {
                return fail("HDF5 requires a public file URL without query or fragment");
            }
            let key = s::StoreKey::new(&filename)?;
            url.path_segments_mut()
                .map_err(|_| "invalid file URL")?
                .pop();
            let mut http = Http::new(
                url.as_str(),
                budget,
                Arc::new(Http::build_client()?),
                Arc::new(MetadataBudget::new(budget.buffer_bytes / 2)),
            )?;
            http.transport = transport;
            http.validate_data = true;
            http.sizes(&[key.to_string()])?
                .first()
                .and_then(|s| *s)
                .ok_or("HDF5 object missing")?;
            if let Some(etag) = &asset.etag {
                if http
                    .state
                    .lock()
                    .unwrap()
                    .versions
                    .get(key.as_str())
                    .and_then(|v| v.as_ref())
                    .is_none_or(|v| &v.etag != etag)
                {
                    return fail("HDF5 requested ETag differs from source");
                }
            }
            // The live size probe just bound this sole object's strong ETag
            // and size. No source bytes or cached metadata were consumed in
            // between: seal that binding without immediately probing it again.
            // Inspection/preparation, pre/post execution and publication still
            // validate online. Checkpoints still need their separate GET proof.
            http.state.lock().unwrap().sealed = true;
            #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
            if let Some(checkpoint) = http.transport.checkpoint() {
                let version = http
                    .state
                    .lock()
                    .unwrap()
                    .versions
                    .get(key.as_str())
                    .and_then(Option::as_ref)
                    .cloned()
                    .ok_or("checkpoint source lacks pinned version")?;
                if version.size == 0 {
                    return fail("checkpoint source object is empty");
                }
                // Canonical HEAD can be public even for a protected object.
                // Never let a complete checkpoint bypass a fresh GET login.
                http.state.lock().unwrap().phase = "checkpoint_online_authorization";
                let proof = http
                    .request(
                        &key,
                        Access::Range {
                            start: 0,
                            end: version.size.min(8),
                        },
                    )?
                    .ok_or("checkpoint authorization object absent")?
                    .1;
                http.state.lock().unwrap().phase = "planning";
                checkpoint.bind(&asset.locator, &version.etag, version.size)?;
                checkpoint.write(0, &proof)?;
            }
            (Objects::Http(http), key)
        } else {
            if asset.etag.is_some() {
                return fail("local HDF5 cannot use HTTP ETag");
            }
            let path = std::path::Path::new(&asset.locator);
            let key = s::StoreKey::new(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .ok_or("invalid file name")?,
            )?;
            (
                Objects::Local(FilesystemStore::new(
                    path.parent().unwrap_or_else(|| std::path::Path::new(".")),
                )?),
                key,
            )
        };
        let size = objects.size_key(&key)?.ok_or("HDF5 file missing")?;
        let local_version = objects
            .local_path(&key)
            .map(|path| {
                let version = local_file_version(&path)?;
                if version["size"].as_u64() != Some(size) {
                    return fail("local HDF5 source changed while opening");
                }
                Ok((path, version))
            })
            .transpose()?;
        Ok(Self {
            objects,
            key,
            size,
            local_version,
        })
    }
    pub fn read(&self, start: u64, len: usize) -> Result<Vec<u8>> {
        self.read_cancellable(start, len, None)
    }
    pub fn read_cancellable(
        &self,
        start: u64,
        len: usize,
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<Vec<u8>> {
        if let Some(h) = self.objects.http() {
            if len == 0 {
                return Ok(Vec::new());
            }
            if cancelled.is_some_and(|c| c.load(Ordering::Acquire)) {
                return fail("acquisition cancelled");
            }
            if h.state.lock().unwrap().failed {
                return fail(
                    "HTTP source binding invalidated by an earlier failure; prepare again",
                );
            }
            let end = start.checked_add(len as u64).ok_or("HDF5 range overflow")?;
            #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
            if let Some(store) = h.transport.checkpoint() {
                if len > h.cap || end > self.size {
                    return fail(
                        "checkpoint read exceeds current object or transport buffer bounds",
                    );
                }
                if let Some(body) = store.read(start, len)? {
                    return Ok(body);
                }
            }
            let body = h
                .request_cancellable(&self.key, Access::Range { start, end }, cancelled)?
                .ok_or("HDF5 range absent")?
                .1;
            #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
            if let Some(store) = h.transport.checkpoint() {
                store.write(start, &body)?;
            }
            return Ok(body);
        }
        if len == 0 {
            return Ok(Vec::new());
        }
        self.validate_local()?;
        let range = s::byte_range::ByteRange::FromStart(start, Some(u64::try_from(len)?));
        let bytes = self
            .objects
            .ranges(&self.key, &[range])?
            .ok_or("HDF5 range absent")?
            .pop()
            .ok_or("HDF5 range absent")?;
        if bytes.len() != len {
            return fail("short HDF5 range");
        }
        self.validate_local()?;
        Ok(bytes.to_vec())
    }
    fn validate_local(&self) -> Result<()> {
        if let Some((path, version)) = &self.local_version {
            if &local_file_version(path)? != version {
                return fail("local HDF5 source changed after preparation");
            }
        }
        Ok(())
    }
    pub fn validate_cancellable(
        &self,
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        self.validate_local()?;
        if let Some(h) = self.objects.http() {
            h.validate_cancellable(cancelled)?;
        }
        Ok(())
    }
    pub fn phase(&self, phase: &'static str) {
        if let Some(h) = self.objects.http() {
            h.state.lock().unwrap().phase = phase;
        }
    }
    pub fn identity(&self) -> serde_json::Value {
        if let Some((path, version)) = &self.local_version {
            return serde_json::json!({"kind":"local_pinned_stat","path":path,"version":version});
        }
        self.objects.identity(self.key.as_str())
    }
    pub fn stats(&self) -> serde_json::Value {
        self.objects.http().map_or(serde_json::Value::Null, |h| {
            let mut stats = serde_json::json!(h.state.lock().unwrap().stats);
            stats["authenticated_http"] = h.transport.stats();
            #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
            if let Some(store) = h.transport.checkpoint() {
                stats["checkpoint"] = store.stats();
            }
            stats
        })
    }
}

fn strong_etag(value: &str) -> bool {
    value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && !value[1..value.len() - 1]
            .bytes()
            .any(|b| b < 33 || b == 127 || b == b'"')
}

#[cfg(test)]
mod tests {
    #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
    #[test]
    #[ignore = "run by bench/earthdata_auth.py with its private HTTPS fixtures"]
    fn authenticated_concurrent_routes_and_sticky_conflicts() {
        use crate::authenticated_http::{AuthenticatedHttp, Config};
        let mut config: Config = serde_json::from_slice(
            &std::fs::read(std::env::var_os("TILE_RIPPER_AUTH_POLICY").unwrap()).unwrap(),
        )
        .unwrap();
        config.additional_roots.push(
            reqwest::Certificate::from_pem(
                &std::fs::read(std::env::var_os("TILE_RIPPER_AUTH_CA").unwrap()).unwrap(),
            )
            .unwrap(),
        );
        let url = config.canonical_url.clone();
        let transport = std::sync::Arc::new(
            AuthenticatedHttp::new(config, std::env::var_os("TILE_RIPPER_AUTH_NETRC").unwrap())
                .unwrap(),
        );
        let asset = crate::Asset {
            id: "fixture".into(),
            locator: url.clone(),
            etag: None,
            format: crate::Format::Hdf5,
            datetime: None,
        };
        let file = super::ObjectFile::open(
            &asset,
            &crate::Budget {
                buffer_bytes: 32 * 1024 * 1024,
                max_nodes: 10000,
                chunk_edge: 256,
            },
            crate::http_transport::Transport::Authenticated(transport.clone()),
        )
        .unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    assert_eq!(file.read(0, 8).unwrap(), b"\x89HDF\r\n\x1a\n");
                });
            }
        });
        let stats = transport.statistics();
        assert!(stats["peak_in_flight"].as_u64().unwrap() <= 4);
        assert_eq!(stats["refreshes"], 1);
        let wire_before = (stats["wire_head"].clone(), stats["wire_get"].clone());
        let unexpected =
            reqwest::Url::parse("https://untrusted.invalid/secret?signature=HIDDEN").unwrap();
        assert!(transport
            .send(
                reqwest::Method::HEAD,
                unexpected,
                reqwest::header::HeaderMap::new(),
                None
            )
            .is_err());
        assert!(transport
            .send(
                reqwest::Method::GET,
                reqwest::Url::parse(&url).unwrap(),
                reqwest::header::HeaderMap::new(),
                None
            )
            .is_err());
        let stats = transport.statistics();
        assert_eq!(
            wire_before,
            (stats["wire_head"].clone(), stats["wire_get"].clone())
        );
        let control = std::env::var("TILE_RIPPER_TEST_CONTROL").unwrap();
        let client = reqwest::blocking::Client::new();
        client.get(format!("{control}/mutate")).send().unwrap();
        assert!(file.read(0, 8).is_err());
        client.get(format!("{control}/restore")).send().unwrap();
        assert!(
            file.validate_cancellable(None).is_err(),
            "a restored server must not clear a detected source conflict"
        );
        assert!(file.read(0, 8).is_err());
    }

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

    #[test]
    fn bounded_parallel_map_deduplicates_and_restores_order() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let keys = vec!["b".to_owned(), "a".to_owned(), "b".to_owned()];
        let values = super::parallel_map(&keys, |key| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(key.to_owned())
        })
        .unwrap();
        assert_eq!(values, ["b", "a", "b"]);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn metadata_budget_is_shared_bounded_and_releasable() {
        let shared = std::sync::Arc::new(super::MetadataBudget::new(10));
        let first_store = shared.clone();
        let second_store = shared.clone();
        assert!(first_store.reserve(6));
        assert!(!second_store.reserve(5));
        first_store.release(6);
        assert!(second_store.reserve(10));
        assert_eq!(shared.used(), 10);
    }
}
