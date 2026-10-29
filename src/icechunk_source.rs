//! Snapshot selection and physical reference resolution use the maintained Icechunk
//! libraries. Payload HTTP remains the ordinary conditional, bounded transport.
use crate::{
    fail,
    model::*,
    storage::{Http, MetadataBudget, Objects},
    Result,
};
use icechunk::{
    config::{CachingConfig, ManifestConfig, ManifestPreloadCondition, ManifestPreloadConfig},
    format::{
        manifest::{Checksum, ChunkPayload},
        ChunkIndices, Path,
    },
    repository::VersionInfo,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use zarrs::storage::{self as s, byte_range::ByteRange, AsyncReadableStorageTraits};
use zarrs_icechunk::AsyncIcechunkStore;

#[derive(Clone)]
enum Payload {
    Inline(s::Bytes),
    Physical {
        source: Arc<Objects>,
        key: String,
        offset: u64,
        length: u64,
        checksum: Value,
    },
}
pub(crate) struct Binding {
    runtime: &'static tokio::runtime::Runtime,
    store: AsyncIcechunkStore,
    root: String,
    selector: String,
    snapshot: String,
    budget: Budget,
    metadata_budget: Arc<MetadataBudget>,
    client: Arc<reqwest::blocking::Client>,
    metadata: Mutex<BTreeMap<String, Option<s::Bytes>>>,
    payloads: Mutex<BTreeMap<String, Option<Payload>>>,
    physical: Mutex<BTreeMap<String, Arc<Objects>>>,
    sealed: AtomicBool,
    failed: AtomicBool,
    cached_bytes: AtomicUsize,
}
fn run<F: Future + Send>(runtime: &tokio::runtime::Runtime, future: F) -> Result<F::Output>
where
    F::Output: Send,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|scope| {
            scope
                .spawn(|| runtime.block_on(future))
                .join()
                .map_err(|_| "Icechunk runtime worker failed".into())
        })
    } else {
        Ok(runtime.block_on(future))
    }
}
fn selector(locator: &str) -> Result<(String, String, VersionInfo)> {
    let mut url = reqwest::Url::parse(locator)
        .map_err(|_| "Icechunk requires a public HTTP repository URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
    {
        return fail("Icechunk requires a public HTTP URL without credentials or query");
    }
    let selection = url.fragment().unwrap_or("branch=main").to_owned();
    let (kind, value) = selection
        .split_once('=')
        .ok_or("Icechunk fragment must be branch=NAME, tag=NAME or snapshot=ID")?;
    if value.is_empty()
        || value.contains(['&', '#', '=', '%'])
        || value
            .chars()
            .any(|c| !c.is_ascii() || c.is_ascii_whitespace() || c.is_ascii_control())
    {
        return fail(
            "invalid Icechunk version selector; use one unescaped branch, tag or snapshot name",
        );
    }
    let version = match kind {
        "branch" => VersionInfo::BranchTipRef(value.into()),
        "tag" => VersionInfo::TagRef(value.into()),
        "snapshot" => VersionInfo::SnapshotId(
            value
                .try_into()
                .map_err(|_| "invalid Icechunk snapshot ID")?,
        ),
        _ => return fail("Icechunk fragment must select branch, tag or snapshot"),
    };
    url.set_fragment(None);
    let root = url.as_str().trim_end_matches('/').to_owned();
    Ok((root, selection, version))
}
impl Binding {
    pub(crate) fn open(
        asset: &Asset,
        budget: &Budget,
        metadata_budget: Arc<MetadataBudget>,
    ) -> Result<Self> {
        if asset.etag.is_some() {
            return fail("Icechunk selects a snapshot, not one asset ETag");
        }
        let (root, selector, version) = selector(&asset.locator)?;
        // A process-owned runtime also permits synchronous acquisition calls
        // inside an async caller without dropping a runtime in that context.
        static RUNTIME: std::sync::OnceLock<std::result::Result<tokio::runtime::Runtime, String>> =
            std::sync::OnceLock::new();
        let runtime = RUNTIME
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|error| error.clone())?;
        let config = icechunk::RepositoryConfig {
            caching: Some(CachingConfig {
                num_snapshot_nodes: Some(budget.max_nodes as u64),
                num_chunk_refs: Some(budget.max_nodes as u64),
                num_transaction_changes: Some(0),
                num_bytes_attributes: Some(0),
                num_bytes_chunks: Some(0),
            }),
            manifest: Some(ManifestConfig {
                preload: Some(ManifestPreloadConfig {
                    max_total_refs: Some(0),
                    preload_if: Some(ManifestPreloadCondition::False),
                    max_arrays_to_scan: Some(0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let storage = icechunk::storage::new_http_storage(
            &root,
            Some(std::collections::HashMap::from([
                ("timeout".into(), "30s".into()),
                ("connect_timeout".into(), "10s".into()),
            ])),
            None,
        )?;
        let session = run(runtime, async {
            let repo =
                icechunk::Repository::open(Some(config), storage, Default::default()).await?;
            repo.readonly_session(&version).await
        })??;
        let snapshot = session.snapshot_id().to_string();
        Ok(Self {
            runtime,
            store: AsyncIcechunkStore::new(session),
            root,
            selector,
            snapshot,
            budget: budget.clone(),
            metadata_budget,
            client: Arc::new(Http::build_client()?),
            metadata: Mutex::new(BTreeMap::new()),
            payloads: Mutex::new(BTreeMap::new()),
            physical: Mutex::new(BTreeMap::new()),
            sealed: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            cached_bytes: AtomicUsize::new(0),
        })
    }
    pub(crate) fn reserve(&self, bytes: usize) -> Result<()> {
        if !self.metadata_budget.reserve(bytes) {
            return fail(
                "aggregate Icechunk metadata/coordinate/index cache exceeds half buffer_bytes",
            );
        }
        Ok(())
    }
    fn reserve_cached(&self, bytes: usize) -> Result<()> {
        self.reserve(bytes)?;
        self.cached_bytes.fetch_add(bytes, Ordering::AcqRel);
        Ok(())
    }
    pub(crate) fn prepared_extra_bytes(&self) -> usize {
        self.cached_bytes.load(Ordering::Acquire)
            + self
                .physical
                .lock()
                .unwrap()
                .values()
                .map(|source| source.physical_cached_bytes())
                .sum::<usize>()
    }
    fn check(&self) -> Result<()> {
        if self.failed.load(Ordering::Acquire) {
            return fail("Icechunk source previously failed; prepare again");
        }
        Ok(())
    }
    fn latch<T>(&self, result: Result<T>) -> Result<T> {
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        }
        result
    }
    fn meta(&self, key: &s::StoreKey) -> Result<Option<s::Bytes>> {
        if !key.as_str().ends_with("zarr.json") {
            return Ok(None);
        }
        let mut cache = self
            .metadata
            .lock()
            .map_err(|_| "Icechunk metadata lock poisoned")?;
        if let Some(bytes) = cache.get(key.as_str()) {
            return Ok(bytes.clone());
        }
        if cache.len() >= self.budget.max_nodes {
            return fail("Icechunk metadata exceeds max_nodes");
        }
        let result = run(self.runtime, self.store.get(key))??;
        if let Some(bytes) = &result {
            if bytes.len() > 1024 * 1024 {
                return fail("Icechunk array metadata exceeds one MiB");
            }
            let metadata: Value = serde_json::from_slice(bytes)?;
            if metadata["node_type"] == "array"
                && metadata["chunk_key_encoding"]
                    != json!({"name":"default","configuration":{"separator":"/"}})
            {
                return fail("Icechunk admission requires default '/' chunk key encoding; other encodings are unsupported");
            }
            self.reserve_cached(bytes.len())?;
        }
        cache.insert(key.to_string(), result.clone());
        Ok(result)
    }
    fn payload(&self, key: &s::StoreKey) -> Result<Option<Payload>> {
        self.check()?;
        let mut cache = self
            .payloads
            .lock()
            .map_err(|_| "Icechunk payload lock poisoned")?;
        if let Some(payload) = cache.get(key.as_str()) {
            return Ok(payload.clone());
        }
        if self.sealed.load(Ordering::Acquire) {
            return fail("unplanned Icechunk chunk read after sealing");
        }
        if cache.len() >= self.budget.max_nodes {
            return fail("Icechunk references exceed max_nodes");
        }
        let Some((array, coords)) = key.as_str().rsplit_once("/c/") else {
            return Ok(None);
        };
        let path: Path = format!("/{array}").try_into()?;
        let coords = ChunkIndices(
            coords
                .split('/')
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()?,
        );
        let reference = run(self.runtime, async {
            self.store
                .session()
                .read()
                .await
                .get_chunk_ref(&path, &coords)
                .await
        })??;
        let result = match reference {
            None => None,
            Some(ChunkPayload::Inline(bytes)) => {
                self.reserve_cached(bytes.len())?;
                Some(Payload::Inline(bytes))
            }
            Some(ChunkPayload::Ref(reference)) => Some(self.physical_payload(
                format!("{}/chunks/{}", self.root, reference.id),
                reference.offset,
                reference.length,
                None,
            )?),
            Some(ChunkPayload::Virtual(reference)) => {
                let raw = reference.location.url();
                let url = if let Some(path) = raw.strip_prefix("s3://") {
                    let (bucket, key) =
                        path.split_once('/').ok_or("invalid virtual S3 reference")?;
                    format!("https://{bucket}.s3.amazonaws.com/{key}")
                } else if raw.starts_with("http://") || raw.starts_with("https://") {
                    raw.to_owned()
                } else {
                    return fail("Icechunk virtual references admit public HTTP(S) and S3 only");
                };
                Some(self.physical_payload(
                    url,
                    reference.offset,
                    reference.length,
                    reference.checksum,
                )?)
            }
            Some(_) => return fail("unsupported Icechunk payload type"),
        };
        self.reserve_cached(256 + key.as_str().len())?;
        cache.insert(key.to_string(), result.clone());
        Ok(result)
    }
    fn physical_payload(
        &self,
        url: String,
        offset: u64,
        length: u64,
        checksum: Option<Checksum>,
    ) -> Result<Payload> {
        let parsed = reqwest::Url::parse(&url)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return fail("unsafe Icechunk physical source URL");
        }
        if parsed.as_str() != url {
            return fail("Icechunk physical source URL normalization would change its identity");
        }
        if length == 0 || offset.checked_add(length).is_none() {
            return fail("invalid Icechunk payload span");
        }
        let cut = url.rfind('/').ok_or("invalid Icechunk object URL")?;
        let (base, key) = (&url[..cut + 1], &url[cut + 1..]);
        let (etag, modified) = match &checksum {
            Some(Checksum::ETag(value)) => {
                let v = &value.0;
                (
                    Some(if v.starts_with('"') {
                        v.clone()
                    } else {
                        format!("\"{v}\"")
                    }),
                    None,
                )
            }
            Some(Checksum::LastModified(value)) => (None, Some(value.0 as i64)),
            None => (None, None),
        };
        // Different snapshot reference checksums must never share a less restrictive binding.
        let binding_key = format!("{url}|{checksum:?}");
        let mut physical = self
            .physical
            .lock()
            .map_err(|_| "Icechunk physical lock poisoned")?;
        let source = if let Some(source) = physical.get(&binding_key) {
            source.clone()
        } else {
            let source = Arc::new(Objects::Http(Http::icechunk_payload(
                base,
                &self.budget,
                self.client.clone(),
                self.metadata_budget.clone(),
                etag,
                modified,
            )?));
            physical.insert(binding_key, source.clone());
            source
        };
        let size = match zarrs::storage::ReadableStorageTraits::size_key(
            source.as_ref(),
            &s::StoreKey::new(key)?,
        )? {
            Some(size) => Some(size),
            None => source.size(key)?,
        }
        .ok_or("Icechunk referenced physical object is absent")?;
        if offset.checked_add(length).is_none_or(|end| end > size) {
            return fail("Icechunk payload exceeds referenced physical object");
        }
        Ok(Payload::Physical {
            source,
            key: key.into(),
            offset,
            length,
            checksum: serde_json::to_value(checksum)?,
        })
    }
    pub(crate) fn get(&self, key: &s::StoreKey) -> Result<Option<s::Bytes>> {
        self.check()?;
        self.latch((|| {
            if key.as_str().ends_with("zarr.json") {
                return self.meta(key);
            }
            let Some(payload) = self.payload(key)? else {
                return Ok(None);
            };
            let length = match &payload {
                Payload::Inline(bytes) => bytes.len() as u64,
                Payload::Physical { length, .. } => *length,
            };
            if length > self.budget.buffer_bytes as u64 / 2 {
                return fail(
                    "encoded Icechunk chunk exceeds half buffer_bytes; use selective shard ranges",
                );
            }
            self.read_payload(payload, &[ByteRange::FromStart(0, Some(length))])
                .map(|mut b| Some(b.remove(0)))
        })())
    }
    pub(crate) fn size(&self, key: &s::StoreKey) -> Result<Option<u64>> {
        self.latch((|| {
            if key.as_str().ends_with("zarr.json") {
                return self.meta(key).map(|bytes| bytes.map(|b| b.len() as u64));
            }
            Ok(self.payload(key)?.map(|p| match p {
                Payload::Inline(bytes) => bytes.len() as u64,
                Payload::Physical { length, .. } => length,
            }))
        })())
    }
    fn read_payload(&self, payload: Payload, ranges: &[ByteRange]) -> Result<Vec<s::Bytes>> {
        let size = match &payload {
            Payload::Inline(bytes) => bytes.len() as u64,
            Payload::Physical { length, .. } => *length,
        };
        let mut out = Vec::new();
        for range in ranges {
            let (start, end) = match *range {
                ByteRange::FromStart(start, length) => (
                    start,
                    length
                        .map_or(Some(size), |n| start.checked_add(n))
                        .ok_or("Icechunk range overflow")?,
                ),
                ByteRange::Suffix(length) => (
                    size.checked_sub(length)
                        .ok_or("Icechunk suffix exceeds payload")?,
                    size,
                ),
            };
            if start >= end || end > size || end - start > self.budget.buffer_bytes as u64 / 2 {
                return fail("Icechunk range exceeds admitted payload or buffer");
            }
            match &payload {
                Payload::Inline(bytes) => out.push(bytes.slice(start as usize..end as usize)),
                Payload::Physical {
                    source,
                    key,
                    offset,
                    ..
                } => out.extend(
                    source
                        .ranges(
                            &s::StoreKey::new(key)?,
                            &[ByteRange::FromStart(
                                offset
                                    .checked_add(start)
                                    .ok_or("Icechunk physical offset overflow")?,
                                Some(end - start),
                            )],
                        )?
                        .ok_or("Icechunk physical source disappeared")?,
                ),
            }
        }
        Ok(out)
    }
    pub(crate) fn ranges(
        &self,
        key: &s::StoreKey,
        ranges: &[ByteRange],
    ) -> Result<Option<Vec<s::Bytes>>> {
        self.check()?;
        self.latch((|| match self.payload(key)? {
            Some(payload) => self.read_payload(payload, ranges).map(Some),
            None => Ok(None),
        })())
    }
    pub(crate) fn validate(&self, seal: bool) -> Result<()> {
        self.check()?;
        self.latch((|| {
            for source in self
                .physical
                .lock()
                .map_err(|_| "Icechunk physical lock poisoned")?
                .values()
            {
                source.validate_physical(seal)?;
            }
            if seal {
                self.sealed.store(true, Ordering::Release);
            }
            Ok(())
        })())
    }
    pub(crate) fn phase(&self, phase: &'static str) {
        for source in self.physical.lock().unwrap().values() {
            source.set_phase(phase);
        }
    }
    pub(crate) fn identity(&self, key: &str) -> Value {
        let payloads = self.payloads.lock().unwrap();
        let payload = payloads.get(key).map(|p| match p {
            Some(Payload::Physical{source,key,offset,length,checksum}) => json!({"kind":"physical_reference","physical":source.identity(key),"offset":offset,"length":length,"reference_checksum":checksum,"reference_checksum_present":!checksum.is_null()}),
            Some(Payload::Inline(bytes)) => json!({"kind":"snapshot_inline","length":bytes.len()}),
            None => json!({"kind":"snapshot_missing"}),
        });
        json!({"repository":self.root,"snapshot_id":self.snapshot,"payload":payload})
    }
    pub(crate) fn describe(&self) -> Value {
        let physical: Vec<_> = self
            .physical
            .lock()
            .unwrap()
            .values()
            .map(|source| source.physical_stats())
            .collect();
        json!({"kind":"icechunk","repository":self.root,"requested_version":self.selector,"snapshot_id":self.snapshot,"snapshot_policy":"resolved once; never repinned","library":"icechunk 2.1.0 / zarrs_icechunk 0.5.1","transport_scope":"physical payload bodies and conditional validation; excludes library-owned repository/snapshot/manifest metadata HTTP and internal allocations","metadata_http_bytes":Value::Null,"physical_payload_transports":physical,"prepared_references":self.payloads.lock().unwrap().len()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selectors_are_explicit_and_unsafe_urls_fail() {
        let (root, requested, _) = selector("https://bucket.example/repo/#branch=main").unwrap();
        assert_eq!(root, "https://bucket.example/repo");
        assert_eq!(requested, "branch=main");
        for url in [
            "https://user:secret@bucket.example/repo",
            "https://bucket.example/repo?token=x",
            "file:///tmp/repo",
            "https://bucket.example/repo#snapshot=bad",
            "https://bucket.example/repo#branch=main&tag=x",
            "https://bucket.example/repo#branch=some%20name",
        ] {
            assert!(selector(url).is_err(), "{url}");
        }
    }
}
