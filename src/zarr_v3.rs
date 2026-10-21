//! Narrow scientific v3 admission over zarrs; indices and codecs remain library-owned.
use crate::{fail, model::*, planner::push, storage::Objects, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use zarrs::{
    array::{
        codec::ShardingCodec, Array, ArrayShardedReadableExt, ArrayShardedReadableExtCache,
        CodecOptions,
    },
    storage::{self as s, byte_range::ByteRange, ReadableStorageTraits},
};

type Logical = Vec<usize>;
type LocalIdentity = Option<(u64, std::time::SystemTime)>;
fn local_identity(path: &std::path::Path) -> Result<LocalIdentity> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => {
            if !m.is_file() || m.file_type().is_symlink() {
                return fail("v3 data keys must be regular files");
            }
            Ok(Some((m.len(), m.modified()?)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
#[derive(Default)]
pub(crate) struct Context {
    datasets: Mutex<BTreeMap<String, Arc<crate::zarr_acquire::Dataset>>>,
    arrays: Mutex<BTreeMap<(String, String), Arc<Binding>>>,
    retained: AtomicUsize,
    local: Mutex<BTreeMap<std::path::PathBuf, LocalIdentity>>,
}
impl Context {
    fn pin_local(&self, source: &Objects, keys: &[String]) -> Result<()> {
        let mut local = self
            .local
            .lock()
            .map_err(|_| "v3 local identity lock poisoned")?;
        for key in keys {
            if let Some(path) = source.local_path(&s::StoreKey::new(key)?) {
                local.insert(path.clone(), local_identity(&path)?);
            }
        }
        Ok(())
    }
    pub(crate) fn validate(&self) -> Result<()> {
        for (path, expected) in self
            .local
            .lock()
            .map_err(|_| "v3 local identity lock poisoned")?
            .iter()
        {
            if local_identity(path)? != *expected {
                return fail("v3 local shard/index version conflict; prepare again");
            }
        }
        Ok(())
    }
    fn reserve(&self, source: &Objects, budget: &Budget, bytes: usize) -> Result<()> {
        self.retained
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(bytes)
                    .filter(|n| *n <= budget.buffer_bytes / 2)
            })
            .map_err(|_| "v3 retained coordinate/index metadata exceeds half buffer_bytes")?;
        if let Err(e) = source.reserve_overlay_cache(bytes) {
            self.retained.fetch_sub(bytes, Ordering::AcqRel);
            return Err(e);
        }
        Ok(())
    }
    pub(crate) fn bytes(&self) -> usize {
        self.retained.load(Ordering::Acquire)
    }
    pub(crate) fn dataset(&self, id: &str) -> Result<Arc<crate::zarr_acquire::Dataset>> {
        self.datasets
            .lock()
            .map_err(|_| "v3 context lock poisoned")?
            .get(id)
            .cloned()
            .ok_or_else(|| "v3 dataset was not prepared".into())
    }
    pub(crate) fn binding(&self, id: &str, path: &str) -> Result<Arc<Binding>> {
        self.arrays
            .lock()
            .map_err(|_| "v3 context lock poisoned")?
            .get(&(id.into(), path.trim_start_matches('/').into()))
            .cloned()
            .ok_or_else(|| "v3 variable was not prepared".into())
    }
    pub(crate) fn describe(&self, id: &str) -> Value {
        let arrays = self.arrays.lock().unwrap();
        json!(arrays.iter().filter(|((asset,_),_)|asset==id).map(|((_,path),b)| (path.clone(),json!({"shape":b.info.shape,"logical_chunk_shape":b.info.inner,"physical_chunk_shape":b.info.outer,"codecs":b.info.metadata["codecs"],"physical_objects":b.ranges.values().map(|(key,_)|key).collect::<BTreeSet<_>>().len(),"selected_logical_chunks":b.ranges.len(),"physical_read_plan":if b.info.sharded {b.physical_plan()}else{Value::Null},"access":if b.info.sharded {"selective_encoded_subchunks"}else{"whole_ordinary_chunks"},"cf_missing_value":b.nodata.map(|n|if n.is_nan(){json!("NaN")}else{json!(n)})}))).collect::<BTreeMap<_,_>>())
    }
}

pub(crate) fn metadata_view(source: Arc<Objects>, budget: &Budget) -> Result<Arc<Objects>> {
    let root = source.json("zarr.json")?;
    if root["zarr_format"] != 3 || root["node_type"] != "group" {
        return fail("v3 source must be a group");
    }
    if source.admission_probe(".zgroup")?.is_some() {
        return fail("hybrid v2/v3 source is unsupported");
    }
    let Some(consolidated) = root.get("consolidated_metadata").filter(|v| !v.is_null()) else {
        return Ok(source);
    };
    if consolidated["kind"] != "inline" {
        return fail("only inline v3 consolidated metadata is supported");
    }
    let metadata = consolidated["metadata"]
        .as_object()
        .ok_or("invalid v3 consolidated metadata")?;
    if metadata.len() > budget.max_nodes {
        return fail("v3 consolidated metadata exceeds max_nodes");
    }
    let mut entries = BTreeMap::new();
    let mut bytes = 0usize;
    for (path, doc) in metadata {
        if path.is_empty()
            || path
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return fail("unsafe consolidated metadata path");
        }
        let encoded = serde_json::to_vec(doc)?;
        bytes = bytes
            .checked_add(encoded.len())
            .ok_or("metadata overflow")?;
        if bytes > budget.buffer_bytes / 2 {
            return fail("expanded v3 metadata exceeds half buffer_bytes");
        }
        entries.insert(format!("{path}/zarr.json"), encoded);
    }
    source.reserve_overlay_cache(bytes)?;
    Ok(Arc::new(Objects::Overlay {
        origin: source,
        entries,
    }))
}

fn sizes(value: &Value) -> Result<Vec<usize>> {
    value
        .as_array()
        .ok_or("missing v3 dimensions")?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| *v > 0)
                .ok_or_else(|| "invalid nonempty v3 dimension".into())
        })
        .collect()
}
fn serial() -> CodecOptions {
    let mut o = CodecOptions::default();
    o.set_concurrent_target(1);
    o
}
pub(crate) fn attrs(source: &Objects, path: &str) -> Result<Value> {
    let metadata = source.json(&if path.is_empty() {
        "zarr.json".into()
    } else {
        format!("{path}/zarr.json")
    })?;
    match metadata.get("attributes") {
        None => Ok(json!({})),
        Some(Value::Object(v)) => Ok(Value::Object(v.clone())),
        _ => fail("v3 attributes must be an object"),
    }
}

#[derive(Clone)]
struct Info {
    metadata: Value,
    shape: Vec<usize>,
    outer: Vec<usize>,
    inner: Vec<usize>,
    names: Vec<String>,
    dtype: String,
    sharded: bool,
    index_bytes: usize,
    index_start: bool,
}
impl Info {
    fn read(source: &Objects, path: &str, budget: &Budget) -> Result<Self> {
        let metadata = source.json(&format!("{path}/zarr.json"))?;
        if metadata["zarr_format"] != 3
            || metadata["node_type"] != "array"
            || metadata["chunk_grid"]["name"] != "regular"
            || metadata
                .get("storage_transformers")
                .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
        {
            return fail("v3 requires regular arrays without storage transformers");
        }
        let shape = sizes(&metadata["shape"])?;
        let outer = sizes(&metadata["chunk_grid"]["configuration"]["chunk_shape"])?;
        let names: Vec<String> = serde_json::from_value(metadata["dimension_names"].clone())
            .map_err(|_| "v3 requires explicit dimension_names")?;
        if shape.len() != outer.len() || names.len() != shape.len() || shape.len() > 3 {
            return fail("v3 rank/dimension mapping mismatch");
        }
        let dtype = match metadata["data_type"].as_str() {
            Some("uint16") => "<u2",
            Some("float32") => "<f4",
            Some("float64") => "<f8",
            Some("int64") => "<i8",
            _ => return fail("unsupported v3 numeric dtype"),
        }
        .to_string();
        let element = if dtype == "<u2" {
            2
        } else if dtype == "<f4" {
            4
        } else {
            8
        };
        outer
            .iter()
            .try_fold(element, |n: usize, c| n.checked_mul(*c))
            .filter(|n| *n <= isize::MAX as usize / 32)
            .ok_or("v3 physical chunk shape overflow")?;
        let codecs = metadata["codecs"].as_array().ok_or("missing v3 codecs")?;
        let sharded = codecs
            .first()
            .is_some_and(|v| v["name"] == "sharding_indexed");
        let (inner, index_bytes, index_start) = if sharded {
            if codecs.len() != 1 {
                return fail("outer codecs require full-shard retrieval; this selective profile refuses them");
            }
            let config = &codecs[0]["configuration"];
            let inner = sizes(&config["chunk_shape"])?;
            if inner.len() != outer.len() || outer.iter().zip(&inner).any(|(o, i)| o % i != 0) {
                return fail("inner chunks must evenly divide the shard");
            }
            let index = config["index_codecs"]
                .as_array()
                .ok_or("missing shard index codecs")?;
            if !(index.len() == 1 || index.len() == 2)
                || index[0]["name"] != "bytes"
                || index[0]["configuration"]["endian"] != "little"
                || (index.len() == 2 && index[1]["name"] != "crc32c")
            {
                return fail("selective profile requires bytes plus optional CRC32C index; no full-shard fallback");
            }
            let n = outer
                .iter()
                .zip(&inner)
                .try_fold(16usize, |n, (o, i)| n.checked_mul(o / i))
                .ok_or("shard index overflow")?;
            let encoded = n
                .checked_add(if index.len() == 2 { 4 } else { 0 })
                .ok_or("shard index overflow")?;
            if encoded > budget.buffer_bytes / 8 {
                return fail("one shard index exceeds the prepared cache budget");
            }
            let start = match config["index_location"].as_str().unwrap_or("end") {
                "start" => true,
                "end" => false,
                _ => return fail("unknown shard index location"),
            };
            (inner, encoded, start)
        } else {
            (outer.clone(), 0, false)
        };
        let expected = inner
            .iter()
            .try_fold(element, |n: usize, c| n.checked_mul(*c))
            .ok_or("inner chunk overflow")?;
        if expected > budget.buffer_bytes / 2 {
            return fail("decoded inner chunk exceeds half buffer_bytes");
        }
        Ok(Self {
            metadata,
            shape,
            outer,
            inner,
            names,
            dtype,
            sharded,
            index_bytes,
            index_start,
        })
    }
    fn inner_metadata(&self) -> Value {
        let mut m = self.metadata.clone();
        m["shape"] = json!(self.inner);
        m["chunk_grid"]["configuration"]["chunk_shape"] = json!(self.inner);
        if self.sharded {
            m["codecs"] = self.metadata["codecs"][0]["configuration"]["codecs"].clone();
        }
        m
    }
    fn shard_indices(&self, logical: &[usize]) -> Vec<u64> {
        logical
            .iter()
            .zip(&self.inner)
            .zip(&self.outer)
            .map(|((i, c), o)| (i * c / o) as u64)
            .collect()
    }
    fn entry(&self, logical: &[usize]) -> usize {
        logical
            .iter()
            .zip(&self.outer)
            .zip(&self.inner)
            .fold(0, |n, ((i, o), c)| n * (o / c) + i % (o / c))
    }
}

#[derive(Clone)]
enum ShardIndex {
    Absent,
    Present {
        size: u64,
        encoded: s::Bytes,
        decoded: Vec<u64>,
    },
}
struct ShardGuard {
    source: Arc<Objects>,
    info: Info,
    indexes: Arc<BTreeMap<String, ShardIndex>>,
}
impl ReadableStorageTraits for ShardGuard {
    fn get(&self, _: &s::StoreKey) -> std::result::Result<s::MaybeBytes, s::StorageError> {
        Err(s::StorageError::Other(
            "full-shard retrieval is disabled for the selective profile".into(),
        ))
    }
    fn get_partial_many<'a>(
        &'a self,
        key: &s::StoreKey,
        ranges: s::byte_range::ByteRangeIterator<'a>,
    ) -> std::result::Result<s::MaybeBytesIterator<'a>, s::StorageError> {
        let result = (|| -> Result<Option<Vec<s::Bytes>>> {
            let Some(ShardIndex::Present { size, encoded, .. }) = self.indexes.get(key.as_str())
            else {
                return Ok(None);
            };
            let ranges: Vec<_> = ranges.collect();
            let mut out = Vec::new();
            for range in ranges {
                let (start, end) = bounds(range, *size)?;
                let index_start = if self.info.index_start {
                    0
                } else {
                    size - self.info.index_bytes as u64
                };
                if start == index_start && end - start == self.info.index_bytes as u64 {
                    out.push(encoded.clone());
                } else {
                    out.extend(
                        self.source
                            .ranges(key, &[range])?
                            .ok_or("shard disappeared")?,
                    );
                }
            }
            Ok(Some(out))
        })();
        result
            .map(|v| {
                v.map(|v| {
                    Box::new(v.into_iter().map(Ok))
                        as Box<
                            dyn Iterator<Item = std::result::Result<s::Bytes, s::StorageError>>
                                + 'a,
                        >
                })
            })
            .map_err(|e| s::StorageError::Other(e.to_string()))
    }
    fn size_key(&self, key: &s::StoreKey) -> std::result::Result<Option<u64>, s::StorageError> {
        Ok(match self.indexes.get(key.as_str()) {
            Some(ShardIndex::Present { size, .. }) => Some(*size),
            _ => None,
        })
    }
    fn supports_get_partial(&self) -> bool {
        true
    }
}
fn bounds(range: ByteRange, size: u64) -> Result<(u64, u64)> {
    let (start, end) = match range {
        ByteRange::FromStart(s, n) => (
            s,
            n.map_or(Some(size), |n| s.checked_add(n))
                .ok_or("offset overflow")?,
        ),
        ByteRange::Suffix(n) => (size.checked_sub(n).ok_or("index larger than shard")?, size),
    };
    if start >= end || end > size {
        return fail("invalid encoded byte range");
    }
    Ok((start, end))
}

enum SourceArray {
    Ordinary(Array<Objects>),
    Sharded {
        array: Array<ShardGuard>,
        caches: BTreeMap<String, ArrayShardedReadableExtCache>,
    },
}
pub(crate) struct Binding {
    info: Info,
    source: Arc<Objects>,
    array: SourceArray,
    ranges: BTreeMap<Logical, (String, Option<(u64, u64)>)>,
    nodata: Option<f64>,
}
impl Binding {
    pub(crate) fn encoded_bytes(&self, indices: &[usize]) -> Result<usize> {
        Ok(self
            .ranges
            .get(indices)
            .ok_or("unplanned v3 logical chunk")?
            .1
            .map_or(0, |(_, n)| n as usize))
    }
    fn span(&self, indices: &[usize]) -> Result<Option<(&str, u64, u64)>> {
        let (key, range) = self
            .ranges
            .get(indices)
            .ok_or("unplanned v3 logical chunk")?;
        // Skip the library helper for MAX/MAX missing entries (upstream overflow).
        let Some((offset, length)) = *range else {
            return Ok(None);
        };
        match &self.array {
            SourceArray::Ordinary(array) => {
                if array
                    .chunk_key(&indices.iter().map(|v| *v as u64).collect::<Vec<_>>())
                    .as_str()
                    != key
                {
                    return fail("ordinary chunk key mismatch");
                }
            }
            SourceArray::Sharded { array, caches } => {
                let range = array.subchunk_byte_range(
                    &caches[key],
                    &indices.iter().map(|v| *v as u64).collect::<Vec<_>>(),
                )?;
                if range != Some(ByteRange::FromStart(offset, Some(length))) {
                    return fail("library shard range differs from validated index");
                }
            }
        }
        Ok(Some((key, offset, length)))
    }
    pub(crate) fn batch(
        &self,
        logical: &[Vec<usize>],
        config: &crate::zarr_ranges::Config,
        cap: usize,
    ) -> Result<Batch> {
        let mut shards: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for idx in logical {
            let (key, range) = self.ranges.get(idx).ok_or("unplanned coalescing member")?;
            if let Some((offset, len)) = range {
                shards
                    .entry(key.clone())
                    .or_default()
                    .push((*offset, *len, idx.clone()));
            }
        }
        let fetches = shards
            .into_iter()
            .flat_map(|(key, spans)| {
                config
                    .groups(&spans, cap as u64)
                    .into_iter()
                    .filter(|g| g.members.len() > 1)
                    .map(move |range| Fetch {
                        key: key.clone(),
                        range,
                    })
            })
            .collect();
        Ok(Batch { fetches })
    }
    pub(crate) fn coalescible(&self) -> bool {
        self.info.sharded && self.source.remote()
    }
    fn physical_plan(&self) -> Value {
        let mut shards: BTreeMap<String, Vec<_>> = BTreeMap::new();
        let mut missing: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for (idx, (key, range)) in &self.ranges {
            let spans = shards.entry(key.clone()).or_default();
            if let Some((start, len)) = range {
                spans.push((*start, *len, idx.clone()));
            } else {
                missing.entry(key.clone()).or_default().push(idx);
            }
        }
        json!(shards.into_iter().map(|(key,mut spans)| {
            spans.sort_by_key(|s|s.0);
            let mut last=0; let mut first=true;
            let physical: Vec<_> = spans.iter().map(|(start,len,idx)| {
                let gap=if first {0} else {start.saturating_sub(last)};
                first=false; last=start+len;
                json!({"logical":idx,"offset":start,"length":len,"gap_before":gap})
            }).collect();
            let candidates = crate::zarr_ranges::Config::from_env(crate::storage::worker_count().unwrap_or(2)).ok().map(|c|c.groups(&spans,c.max_span));
            json!({"key":key,"identity":self.source.identity(&key),"physical_order":physical,"missing_logical_chunks":missing.get(&key),"unique_selected_bytes":spans.iter().map(|s|s.1).sum::<u64>(),"repeated_selected_bytes":0,"candidate_body_bytes":candidates.as_ref().map(|g|g.iter().map(|r|r.len() as u64).sum::<u64>()),"candidate_requests":candidates.as_ref().map(Vec::len),"candidate_ranges_before_execution_window_and_memory_limits":candidates})
        }).collect::<Vec<_>>())
    }
}

#[derive(Default)]
struct OneChunk(Mutex<Option<s::Bytes>>);
impl ReadableStorageTraits for OneChunk {
    fn get(&self, _: &s::StoreKey) -> std::result::Result<s::MaybeBytes, s::StorageError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn get_partial_many<'a>(
        &'a self,
        key: &s::StoreKey,
        ranges: s::byte_range::ByteRangeIterator<'a>,
    ) -> std::result::Result<s::MaybeBytesIterator<'a>, s::StorageError> {
        let Some(b) = self.get(key)? else {
            return Ok(None);
        };
        let mut out = Vec::new();
        for r in ranges {
            let (s, e) =
                bounds(r, b.len() as u64).map_err(|e| s::StorageError::Other(e.to_string()))?;
            out.push(Ok(b.slice(s as usize..e as usize)));
        }
        Ok(Some(Box::new(out.into_iter())))
    }
    fn size_key(&self, _: &s::StoreKey) -> std::result::Result<Option<u64>, s::StorageError> {
        Ok(self.0.lock().unwrap().as_ref().map(|v| v.len() as u64))
    }
    fn supports_get_partial(&self) -> bool {
        false
    }
}
#[derive(Debug, serde::Serialize)]
pub(crate) struct Fetch {
    pub key: String,
    pub range: crate::zarr_ranges::Group,
}
#[derive(Debug, serde::Serialize)]
pub(crate) struct Batch {
    pub fetches: Vec<Fetch>,
}
impl Batch {
    pub fn bytes(&self) -> usize {
        self.fetches.iter().map(|f| f.range.len() + 1).sum()
    }
    pub fn metadata_bytes(&self) -> usize {
        self.fetches
            .iter()
            .map(|f| 256 + f.key.len() + f.range.members.len() * 96)
            .sum()
    }
}
#[derive(Default, serde::Serialize)]
struct Metrics {
    logical_payloads: u64,
    selected_bytes: u64,
    payload_requests: u64,
    fetched_bytes: u64,
    merged_requests: u64,
    merged_cache_hits: u64,
    prepared_index_hits: u64,
    retained_buffer_peak: usize,
}
pub(crate) struct Reader {
    binding: Arc<Binding>,
    input: Arc<OneChunk>,
    decoded: Array<crate::zarr_decode::Checked<OneChunk>>,
    batch: Option<Arc<Batch>>,
    buffers: Vec<Option<s::Bytes>>,
    metrics: Metrics,
}
impl Reader {
    pub(crate) fn new(binding: Arc<Binding>, budget: &Budget) -> Result<Self> {
        let input = Arc::new(OneChunk::default());
        let decoded = crate::zarr_decode::array_v3(
            input.clone(),
            "chunk",
            binding.info.inner_metadata(),
            budget,
        )?;
        Ok(Self {
            binding,
            input,
            decoded,
            batch: None,
            buffers: Vec::new(),
            metrics: Metrics::default(),
        })
    }
    pub(crate) fn begin_batch(&mut self, batch: Option<Arc<Batch>>) {
        self.end_batch();
        self.buffers = vec![None; batch.as_ref().map_or(0, |b| b.fetches.len())];
        self.batch = batch;
    }
    pub(crate) fn end_batch(&mut self) {
        self.buffers.clear();
        self.batch = None;
        self.input.0.lock().unwrap().take();
    }
    pub(crate) fn metrics(&self) -> Value {
        json!(self.metrics)
    }
    pub(crate) fn read(
        &mut self,
        indices: &[usize],
    ) -> Result<(crate::Pixels, bool, Option<usize>, f64)> {
        let started = std::time::Instant::now();
        let mut transfer = None;
        let payload = if let Some((key, start, len)) = self.binding.span(indices)? {
            self.metrics.logical_payloads += 1;
            self.metrics.selected_bytes += len;
            if self.binding.info.sharded {
                self.metrics.prepared_index_hits += 1;
            }
            let merged = self.batch.as_ref().and_then(|b| {
                b.fetches
                    .iter()
                    .enumerate()
                    .find(|(_, f)| f.key == key && f.range.members.iter().any(|i| i == indices))
            });
            let bytes = if let Some((i, fetch)) = merged {
                if self.buffers[i].is_none() {
                    let body = self
                        .binding
                        .source
                        .ranges(
                            &s::StoreKey::new(key)?,
                            &[ByteRange::FromStart(
                                fetch.range.start,
                                Some(fetch.range.len() as u64),
                            )],
                        )?
                        .ok_or("shard disappeared")?
                        .remove(0);
                    if body.len() != fetch.range.len() {
                        return fail("truncated merged v3 body");
                    }
                    transfer = Some(body.len());
                    self.buffers[i] = Some(body);
                    self.metrics.merged_requests += 1;
                    self.metrics.retained_buffer_peak = self
                        .metrics
                        .retained_buffer_peak
                        .max(self.buffers.iter().flatten().map(|b| b.len()).sum());
                } else {
                    self.metrics.merged_cache_hits += 1;
                }
                let relative = (start - fetch.range.start) as usize;
                self.buffers[i]
                    .as_ref()
                    .unwrap()
                    .slice(relative..relative + len as usize)
            } else {
                let body = self
                    .binding
                    .source
                    .ranges(
                        &s::StoreKey::new(key)?,
                        &[ByteRange::FromStart(start, Some(len))],
                    )?
                    .ok_or("v3 chunk disappeared")?
                    .remove(0);
                if body.len() as u64 != len {
                    return fail("truncated v3 body");
                }
                transfer = Some(body.len());
                body
            };
            Some(bytes)
        } else {
            None
        };
        if let Some(n) = transfer {
            self.metrics.payload_requests += 1;
            self.metrics.fetched_bytes += n as u64;
        }
        let retrieval_seconds = started.elapsed().as_secs_f64();
        let present = payload.is_some();
        *self.input.0.lock().unwrap() = payload;
        let zeros = vec![0; indices.len()];
        let result = if self.binding.info.dtype == "<f4" {
            self.decoded
                .retrieve_chunk_opt::<Vec<f32>>(&zeros, &serial())
                .map(crate::Pixels::Float32)
        } else {
            self.decoded
                .retrieve_chunk_opt::<Vec<u16>>(&zeros, &serial())
                .map(crate::Pixels::UInt16)
        };
        self.input.0.lock().unwrap().take();
        Ok((result?, present, transfer, retrieval_seconds))
    }
}

pub(crate) fn missing(attrs: &Value, dtype: &str) -> Result<Option<f64>> {
    use base64::Engine;
    fn number(v: &Value) -> Result<f64> {
        if let Some(n) = v.as_f64() {
            return Ok(n);
        }
        if v == "NaN" {
            return Ok(f64::NAN);
        }
        if let Some(encoded) = v.as_str() {
            let b = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| "unsupported CF fill attribute")?;
            return match b.len() {
                4 => Ok(f32::from_le_bytes(b.try_into().unwrap()) as f64),
                8 => Ok(f64::from_le_bytes(b.try_into().unwrap())),
                _ => fail("unsupported CF fill representation"),
            };
        }
        fail("CF fill must be a scalar native numeric value")
    }
    let mut markers = Vec::new();
    for key in ["_FillValue", "missing_value"] {
        if let Some(v) = attrs.get(key) {
            markers.push(number(v)?);
        }
    }
    if markers.len() == 2 && !(markers[0] == markers[1] || markers.iter().all(|v| v.is_nan())) {
        return fail("conflicting CF missing markers");
    }
    let Some(n) = markers.first().copied() else {
        return Ok(None);
    };
    if dtype == "<u2" {
        if !n.is_finite() || n.fract() != 0. || !(0. ..=65535.).contains(&n) {
            return fail("CF missing marker outside UInt16");
        }
        Ok(Some(n))
    } else if dtype == "<f8" {
        Ok(Some(n))
    } else {
        let f = n as f32;
        if n.is_finite() && !f.is_finite() {
            return fail("CF marker exceeds Float32");
        }
        Ok(Some(f as f64))
    }
}
fn times(raw: &[i64], attrs: &Value) -> Result<Vec<chrono::DateTime<chrono::Utc>>> {
    if attrs["calendar"] != "proleptic_gregorian" {
        return fail("v3 time requires proleptic_gregorian calendar");
    }
    let units = attrs["units"].as_str().ok_or("missing CF time units")?;
    let (unit, epoch) = units
        .split_once(" since ")
        .ok_or("unsupported CF time units")?;
    let factor: i128 = match unit {
        "seconds" => 1_000_000_000,
        "milliseconds" => 1_000_000,
        "microseconds" => 1000,
        "nanoseconds" => 1,
        "hours" => 3_600_000_000_000,
        "days" => 86_400_000_000_000,
        _ => return fail("unsupported CF time unit"),
    };
    let epoch = epoch.trim_end_matches('Z');
    let epoch = if epoch.len() == 10 {
        format!("{epoch}T00:00:00Z")
    } else {
        format!("{}Z", epoch.replace(' ', "T"))
    };
    let epoch = chrono::DateTime::parse_from_rfc3339(&epoch)
        .map_err(|_| "CF time epoch must be explicit UTC")?;
    let base = epoch.timestamp() as i128 * 1_000_000_000 + epoch.timestamp_subsec_nanos() as i128;
    let out = raw
        .iter()
        .map(|v| {
            let n = (*v as i128)
                .checked_mul(factor)
                .and_then(|n| n.checked_add(base))
                .ok_or("time conversion overflow")?;
            let sec =
                i64::try_from(n.div_euclid(1_000_000_000)).map_err(|_| "time out of range")?;
            chrono::DateTime::from_timestamp(sec, n.rem_euclid(1_000_000_000) as u32)
                .ok_or("time outside supported calendar")
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if out.windows(2).any(|p| p[0] >= p[1]) {
        return fail("v3 time must be ascending unique instants");
    }
    Ok(out)
}
fn inspect(r: &Request, source: Arc<Objects>) -> Result<crate::zarr_acquire::Dataset> {
    let root = attrs(&source, "")?;
    let Variable::Dataset { path } = r.variables.first().ok_or("v3 requires a variable")? else {
        return fail("v3 requires dataset selectors");
    };
    let path = crate::zarr_acquire::name(path)?;
    let first = Info::read(&source, path, &r.budget)?;
    if first.names.len() != 3 || first.names[0] != "time" {
        return fail("v3 profile requires time-first rank-three arrays");
    }
    let axes =
        if first.names.contains(&"latitude".into()) && first.names.contains(&"longitude".into()) {
            ["longitude".to_string(), "latitude".to_string()]
        } else if first.names.contains(&"lat".into()) && first.names.contains(&"lon".into()) {
            ["lon".into(), "lat".into()]
        } else if first.names.contains(&"x".into()) && first.names.contains(&"y".into()) {
            ["x".into(), "y".into()]
        } else {
            return fail("v3 supports time plus x/y, longitude/latitude or lon/lat axes");
        };
    let mut xy = Vec::new();
    for axis in &axes {
        let info = Info::read(&source, axis, &r.budget)?;
        if info.sharded
            || info.dtype != "<f8"
            || info.names != [axis.clone()]
            || info.shape.len() != 1
            || info.shape[0]
                .checked_mul(8)
                .is_none_or(|n| n > r.budget.buffer_bytes / 16)
            || info.inner[0]
                .checked_mul(8)
                .is_none_or(|n| n > r.budget.buffer_bytes / 16)
        {
            return fail("v3 coordinates require bounded ordinary Float64 axes");
        }
        let a = crate::zarr_decode::array_v3(source.clone(), axis, info.metadata, &r.budget)?;
        let values = a.retrieve_array_subset::<Vec<f64>>(&a.subset_all())?;
        let coordinate_marker = missing(&attrs(&source, axis)?, "<f8")?;
        if values.len() < 2
            || values
                .iter()
                .any(|v| !v.is_finite() || coordinate_marker.is_some_and(|m| *v == m))
        {
            return fail("v3 spatial coordinates require finite centers");
        }
        xy.push(values);
    }
    let tm = Info::read(&source, "time", &r.budget)?;
    if tm.sharded
        || tm.dtype != "<i8"
        || tm.names != ["time"]
        || tm.shape.len() != 1
        || tm.shape[0]
            .checked_mul(24)
            .is_none_or(|n| n > r.budget.buffer_bytes / 8)
        || tm.inner[0]
            .checked_mul(8)
            .is_none_or(|n| n > r.budget.buffer_bytes / 8)
    {
        return fail("v3 time requires a bounded ordinary Int64 axis");
    }
    let a = crate::zarr_decode::array_v3(source.clone(), "time", tm.metadata, &r.budget)?;
    let raw_times = a.retrieve_array_subset::<Vec<i64>>(&a.subset_all())?;
    let time_attrs = attrs(&source, "time")?;
    for key in ["_FillValue", "missing_value"] {
        if let Some(marker) = time_attrs.get(key) {
            let marker = marker
                .as_i64()
                .ok_or("Int64 time CF missing marker must be a native integer")?;
            if raw_times.contains(&marker) {
                return fail("required time coordinate contains CF missing values");
            }
        }
    }
    let times = times(&raw_times, &time_attrs)?;
    let va = attrs(&source, path)?;
    if va.get("ancillary_variables").is_some() {
        return fail("external ancillary masks are not admitted; native fill/CF/NaN validity only");
    }
    let mapping_name = va["grid_mapping"].as_str().or_else(|| {
        va["coordinates"]
            .as_str()
            .and_then(|v| v.split_whitespace().find(|v| *v == "spatial_ref"))
    });
    let mapping = if let Some(name) = mapping_name {
        crate::zarr_acquire::name(name)?;
        Some((name.to_string(), attrs(&source, name)?))
    } else {
        None
    };
    let crs = if let Some((_, mapping)) = &mapping {
        mapping["crs_wkt"]
            .as_str()
            .or_else(|| mapping["spatial_ref"].as_str())
            .or_else(|| mapping["epsg_code"].as_str())
            .ok_or("CF mapping requires declared CRS WKT/authority")?
            .to_string()
    } else {
        root["tile_ripper_grid"]["crs"].as_str().ok_or("v3 requires an explicit CF grid mapping or existing native grid metadata; geographic names alone do not declare WGS84")?.to_string()
    };
    if crs.is_empty() {
        return fail("empty native CRS");
    }
    let (x, y) = (xy.remove(0), xy.remove(0));
    let dx = x[1] - x[0];
    let dy = y[1] - y[0];
    for (values, step) in [(&x, dx), (&y, dy)] {
        if !step.is_finite()
            || step == 0.
            || values
                .iter()
                .enumerate()
                .any(|(i, v)| (*v - (values[0] + i as f64 * step)).abs() > step.abs() * 1e-8)
        {
            return fail("v3 requires regular monotonic spatial axes");
        }
    }
    let transform = [x[0] - dx / 2., dx, 0., y[0] - dy / 2., 0., dy];
    if let Some(t) = root["tile_ripper_grid"].get("transform") {
        let stated: [f64; 6] = serde_json::from_value(t.clone())?;
        if stated
            .iter()
            .zip(transform)
            .any(|(a, b)| !a.is_finite() || (*a - b).abs() > dx.abs().max(dy.abs()) * 1e-8)
        {
            return fail("coordinate centers disagree with declared affine");
        }
    }
    Ok(crate::zarr_acquire::Dataset {
        layout: Layout {
            width: x.len(),
            height: y.len(),
            transform,
            crs,
            variable: Variable::Dataset {
                path: String::new(),
            },
            block: [1, 1],
            dtype: "<u2".into(),
            nodata: None,
            scale: None,
            offset: None,
            tiles: vec![],
        },
        times,
        x,
        y,
        root_attrs: root,
        axes,
        raw_times,
        conventional: true,
        mapping,
    })
}

fn validate_index(
    codec: &ShardingCodec,
    info: &Info,
    encoded: &[u8],
    size: u64,
) -> Result<Vec<u64>> {
    let counts: Vec<_> = info
        .outer
        .iter()
        .zip(&info.inner)
        .map(|(o, i)| NonZeroU64::new((o / i) as u64).unwrap())
        .collect();
    let decoded = codec.decode_index(encoded, &counts, &serial())?;
    let data_start = if info.index_start {
        info.index_bytes as u64
    } else {
        0
    };
    let data_end = if info.index_start {
        size
    } else {
        size.checked_sub(info.index_bytes as u64)
            .ok_or("truncated shard index")?
    };
    let mut spans = Vec::new();
    for pair in decoded.chunks_exact(2) {
        if pair[0] == u64::MAX || pair[1] == u64::MAX {
            if pair != [u64::MAX, u64::MAX] {
                return fail("invalid mixed missing-chunk sentinel");
            }
            continue;
        }
        let end = pair[0]
            .checked_add(pair[1])
            .ok_or("shard offset/length overflow")?;
        if pair[1] == 0 || pair[0] < data_start || end > data_end {
            return fail("shard payload offset/length exceeds object or overlaps index");
        }
        spans.push((pair[0], end));
    }
    spans.sort_unstable();
    if spans.windows(2).any(|p| p[0].1 > p[1].0) {
        return fail("overlapping shard payload ranges are not admitted");
    }
    Ok(decoded)
}
impl Context {
    pub(crate) fn plan(
        &self,
        a: &Asset,
        r: &Request,
        p: &mut Plan,
        source: Arc<Objects>,
    ) -> Result<()> {
        if r.budget
            .chunk_edge
            .checked_mul(r.budget.chunk_edge)
            .and_then(|n| n.checked_mul(4))
            .is_none_or(|n| n > r.budget.buffer_bytes / 4)
        {
            return fail("v3 delivery buffer exceeds quarter buffer_bytes");
        }
        let d = Arc::new(inspect(r, source.clone())?);
        self.reserve(
            &source,
            &r.budget,
            d.x.len()
                .checked_add(d.y.len())
                .and_then(|n| n.checked_add(d.times.len().checked_mul(3)?))
                .and_then(|n| n.checked_mul(8))
                .and_then(|n| {
                    n.checked_add(
                        serde_json::to_vec(&d.root_attrs)
                            .ok()?
                            .len()
                            .checked_mul(4)?,
                    )
                })
                .ok_or("coordinate/root-attribute reservation overflow")?,
        )?;
        self.datasets
            .lock()
            .unwrap()
            .insert(a.id.clone(), d.clone());
        let window = crate::spatial::coordinate_window(&d.layout.crs, &r.spatial, &d.x, &d.y)?;
        let mut ti = Vec::new();
        for (i, t) in d.times.iter().enumerate() {
            let mut observation = a.clone();
            observation.datetime = Some(t.to_rfc3339());
            if crate::planner::time_matches(&observation, &r.time)? {
                ti.push(i);
            }
        }
        let mut names = BTreeSet::new();
        for variable in &r.variables {
            let Variable::Dataset { path } = variable else {
                return fail("v3 requires dataset selectors");
            };
            if !names.insert(crate::zarr_acquire::name(path)?) {
                return fail("duplicate v3 dataset selector");
            }
        }
        let Some(window) = window.filter(|_| !ti.is_empty()) else {
            return Ok(());
        };
        let inspect_node = push(p, r, &a.id, vec![], Operation::Inspect { cached: false })?;
        for variable in &r.variables {
            let Variable::Dataset { path } = variable else {
                unreachable!()
            };
            let path = crate::zarr_acquire::name(path)?;
            let info = Info::read(&source, path, &r.budget)?;
            if !matches!(info.dtype.as_str(), "<u2" | "<f4")
                || info.names.len() != 3
                || info.names[0] != "time"
            {
                return fail("v3 data requires UInt16/Float32 time-first rank-three arrays");
            }
            let yi = info
                .names
                .iter()
                .position(|n| n == &d.axes[1])
                .ok_or("missing v3 y dimension")?;
            let xi = info
                .names
                .iter()
                .position(|n| n == &d.axes[0])
                .ok_or("missing v3 x dimension")?;
            if yi == 0
                || xi == 0
                || yi == xi
                || info.shape[0] != d.times.len()
                || info.shape[yi] != d.y.len()
                || info.shape[xi] != d.x.len()
            {
                return fail("v3 variables must share declared coordinate axes");
            }
            let va = attrs(&source, path)?;
            if va.get("ancillary_variables").is_some()
                || ["bounds", "cell_measures"]
                    .iter()
                    .any(|k| va.get(*k).is_some())
            {
                return fail("external mask/bounds/cell-measure references are unsupported in this v3 profile");
            }
            if va
                .get("_ARRAY_DIMENSIONS")
                .is_some_and(|v| *v != json!(info.names))
            {
                return fail("conflicting v3 dimension mappings");
            }
            let expected_mapping = d.mapping.as_ref().map(|(name, _)| name.as_str());
            let stated_mapping = va["grid_mapping"].as_str().or_else(|| {
                va["coordinates"]
                    .as_str()
                    .and_then(|v| v.split_whitespace().find(|n| *n == "spatial_ref"))
            });
            if stated_mapping != expected_mapping {
                return fail("v3 variables must share the explicit CF grid mapping");
            }
            if va.get("coordinates").is_some_and(|v| {
                v.as_str().is_none_or(|v| {
                    v.split_whitespace().any(|n| {
                        n != "time"
                            && n != d.axes[0]
                            && n != d.axes[1]
                            && Some(n) != expected_mapping
                    })
                })
            }) {
                return fail("unsupported auxiliary v3 coordinates");
            }
            let nodata = missing(&va, &info.dtype)?;
            for key in ["scale_factor", "add_offset"] {
                if va
                    .get(key)
                    .is_some_and(|v| v.as_f64().is_none_or(|v| !v.is_finite()))
                {
                    return fail("calibration attributes must be finite numeric scalars");
                }
            }
            // Validate the ordinary inner codec with the same bounded decoder
            // that will receive the selected encoded bytes during execution.
            let _ = crate::zarr_decode::array_v3(
                Arc::new(OneChunk::default()),
                "chunk",
                info.inner_metadata(),
                &r.budget,
            )?;
            let original = Array::new_with_metadata(
                source.clone(),
                &format!("/{path}"),
                serde_json::from_value(info.metadata.clone())?,
            )?;
            let mut logical = BTreeSet::new();
            for t in &ti {
                for y in window.y / info.inner[yi]..=(window.y + window.height - 1) / info.inner[yi]
                {
                    for x in
                        window.x / info.inner[xi]..=(window.x + window.width - 1) / info.inner[xi]
                    {
                        let mut idx = vec![t / info.inner[0], 0, 0];
                        idx[yi] = y;
                        idx[xi] = x;
                        logical.insert(idx);
                        if logical.len() > r.budget.max_nodes {
                            return fail("logical v3 enumeration exceeds max_nodes");
                        }
                    }
                }
            }
            let mut physical = BTreeMap::new();
            for idx in &logical {
                let key = original
                    .chunk_key(&if info.sharded {
                        info.shard_indices(idx)
                    } else {
                        idx.iter().map(|i| *i as u64).collect()
                    })
                    .to_string();
                physical.insert(key, ());
            }
            let keys: Vec<_> = physical.into_keys().collect();
            self.pin_local(&source, &keys)?;
            let sizes = source.sizes(&keys)?;
            let codec = if info.sharded {
                Some(
                    original
                        .codecs()
                        .array_to_bytes_codec()
                        .as_any()
                        .downcast_ref::<ShardingCodec>()
                        .ok_or("unexpected sharding codec")?
                        .clone(),
                )
            } else {
                None
            };
            let reserve = keys
                .len()
                .checked_mul(
                    info.index_bytes
                        .checked_mul(4)
                        .ok_or("index reservation overflow")?,
                )
                .and_then(|n| n.checked_add(logical.len().checked_mul(256)?))
                .and_then(|n| {
                    n.checked_add(
                        serde_json::to_vec(&info.metadata)
                            .ok()?
                            .len()
                            .checked_mul(16)?,
                    )
                })
                .ok_or("prepared v3 metadata reservation overflow")?;
            self.reserve(&source, &r.budget, reserve)?;
            let entries: Vec<_> = keys.iter().cloned().zip(sizes).collect();
            let indexes = if let Some(codec) = &codec {
                crate::storage::parallel_map(&keys, |key| {
                    let size = &entries
                        .iter()
                        .find(|(k, _)| k == key)
                        .ok_or("unplanned shard")?
                        .1;
                    let Some(size) = size else {
                        return Ok((key.to_string(), ShardIndex::Absent));
                    };
                    if *size < info.index_bytes as u64 {
                        return fail("truncated shard index");
                    }
                    let range = if info.index_start {
                        ByteRange::FromStart(0, Some(info.index_bytes as u64))
                    } else {
                        ByteRange::Suffix(info.index_bytes as u64)
                    };
                    let encoded = source
                        .ranges(&s::StoreKey::new(key)?, &[range])?
                        .ok_or("index source disappeared")?
                        .into_iter()
                        .next()
                        .ok_or("empty index range")?;
                    if encoded.len() != info.index_bytes {
                        return fail("truncated shard index");
                    }
                    let decoded = validate_index(codec, &info, &encoded, *size)?;
                    Ok((
                        key.to_string(),
                        ShardIndex::Present {
                            size: *size,
                            encoded,
                            decoded,
                        },
                    ))
                })?
                .into_iter()
                .collect::<BTreeMap<_, _>>()
            } else {
                for (_, size) in &entries {
                    if size.is_some_and(|n| n > r.budget.buffer_bytes as u64 / 2) {
                        return fail("ordinary encoded v3 chunk exceeds half buffer_bytes");
                    }
                }
                BTreeMap::new()
            };
            let indexes = Arc::new(indexes);
            let mut ranges = BTreeMap::new();
            let mut payload = 0u64;
            for idx in &logical {
                let key = original
                    .chunk_key(&if info.sharded {
                        info.shard_indices(idx)
                    } else {
                        idx.iter().map(|i| *i as u64).collect()
                    })
                    .to_string();
                let span = if info.sharded {
                    match &indexes[&key] {
                        ShardIndex::Absent => None,
                        ShardIndex::Present { decoded, .. } => {
                            let n = info.entry(idx) * 2;
                            let (o, l) = (decoded[n], decoded[n + 1]);
                            if o == u64::MAX {
                                None
                            } else {
                                Some((o, l))
                            }
                        }
                    }
                } else {
                    entries
                        .iter()
                        .find(|(k, _)| k == &key)
                        .unwrap()
                        .1
                        .map(|n| (0, n))
                };
                if let Some((_, n)) = span {
                    if n > r.budget.buffer_bytes as u64 / 2 {
                        return fail("selected encoded inner chunk exceeds half buffer_bytes");
                    }
                    payload = payload.checked_add(n).ok_or("payload estimate overflow")?;
                }
                ranges.insert(idx.clone(), (key, span));
            }
            let array = if info.sharded {
                let store = Arc::new(ShardGuard {
                    source: source.clone(),
                    info: info.clone(),
                    indexes: indexes.clone(),
                });
                let array = Array::new_with_metadata(
                    store,
                    &format!("/{path}"),
                    serde_json::from_value(info.metadata.clone())?,
                )?;
                let caches = keys
                    .iter()
                    .map(|key| (key.clone(), ArrayShardedReadableExtCache::new(&array)))
                    .collect();
                SourceArray::Sharded { array, caches }
            } else {
                SourceArray::Ordinary(original)
            };
            let binding = Arc::new(Binding {
                info: info.clone(),
                source: source.clone(),
                array,
                ranges,
                nodata,
            });
            self.arrays
                .lock()
                .unwrap()
                .insert((a.id.clone(), path.into()), binding.clone());
            p.estimates.selected_compressed_payload_bytes = p
                .estimates
                .selected_compressed_payload_bytes
                .and_then(|n| n.checked_add(payload));
            let mut layout = d.layout.clone();
            layout.variable = variable.clone();
            layout.block = [info.inner[xi], info.inner[yi]];
            layout.dtype = info.dtype.clone();
            layout.nodata = nodata;
            layout.scale = va["scale_factor"].as_f64();
            layout.offset = va["add_offset"].as_f64();
            let mut selected_indices = vec![ti.clone(), vec![], vec![]];
            selected_indices[yi] = (window.y..window.y + window.height).collect();
            selected_indices[xi] = (window.x..window.x + window.width).collect();
            p.selected.push(Selected{asset_id:a.id.clone(),datetime:None,timestamps:ti.iter().map(|i|d.times[*i].to_rfc3339()).collect(),dimensions:Some(DimensionSelection{shape:info.shape.clone(),chunk_shape:info.inner.clone(),names:info.names.clone(),indices:selected_indices}),mask_geometry:None,reason:format!("native v3 coordinate subset: {path}"),capabilities:Capabilities{selection:vec!["native bounds, explicit CF CRS, time, variables".into()],reads:if info.sharded{vec!["one cached index range per physical shard; selected encoded inner chunks".into()]}else{vec!["whole ordinary chunk objects".into()]},decoding:"zarrs 0.23.14 with fixed-output codec admission".into(),compression:"bytes/gzip/Zstd/Blosc LZ4 or Zstd; optional CRC32C shard index".into(),materialization:true},layout:Some(layout),window:window.clone()});
            for (idx, (key, _)) in &binding.ranges {
                let read = push(
                    p,
                    r,
                    &a.id,
                    vec![inspect_node],
                    Operation::Read {
                        read: Read::ChunkObject {
                            key: key.clone(),
                            indices: idx.clone(),
                        },
                    },
                )?;
                let decode = push(
                    p,
                    r,
                    &a.id,
                    vec![read],
                    Operation::Decode {
                        reader: "zarrs v3 logical inner chunk".into(),
                    },
                )?;
                push(
                    p,
                    r,
                    &a.id,
                    vec![decode],
                    Operation::Write {
                        output: format!("{}/{path}", a.id),
                    },
                )?;
            }
        }
        p.limitations.push("V3 native rank-three UInt16/Float32, time first, regular explicit spatial axes and declared native CRS; no reprojection/calibration. Storage fill is distinct from CF missingness. Shard keys are physical, node indices/chunk_shape logical. Outer/nested sharding codecs, external masks and unsupported index pipelines are refused; no full-shard fallback. Strong per-object versions are not an atomic dataset snapshot.".into());
        Ok(())
    }
}
