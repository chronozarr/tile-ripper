//! Narrow AlphaEarth Foundations GeoZarr acquisition. Native stored Int8 values,
//! positional channels and pixel-centre coordinates are preserved. This is a
//! standalone stream; it does not extend the rank-three scientific writer.
use crate::{
    fail,
    storage::{Objects, Sources},
    Budget, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use zarrs::{
    array::{codec::ShardingCodec, Array, CodecOptions},
    storage::{self as s, byte_range::ByteRange, ReadableStorageTraits},
};

const INNER: [usize; 4] = [1, 64, 256, 256];
const OUTER: [usize; 4] = [1, 64, 4096, 4096];
const DECODED: usize = 64 * 256 * 256;
const INDEX: usize = 16 * 16 * 16 + 4;
const RANGE_CAP: u64 = 8 * 1024 * 1024;
const METADATA_CAP: usize = 1024 * 1024;

/// One time index and an index window on the independently checked native grid.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub locator: String,
    pub time_index: usize,
    /// [y,x], inclusive start.
    pub start: [usize; 2],
    /// [height,width].
    pub count: [usize; 2],
    /// Positional indices in caller order; omission selects all 64 channels.
    #[serde(default)]
    pub channels: Option<Vec<usize>>,
    /// Hard wire-body budget, including metadata and bounded retries.
    #[serde(default = "default_network")]
    pub network_bytes: u64,
    #[serde(default = "default_requests")]
    pub network_requests: u64,
    #[serde(default = "default_working")]
    pub working_bytes: usize,
}
fn default_network() -> u64 {
    32 * 1024 * 1024
}
fn default_requests() -> u64 {
    128
}
fn default_working() -> usize {
    64 * 1024 * 1024
}

#[derive(Clone, Debug, Serialize)]
pub struct Chunk {
    pub year: i32,
    pub time_index: usize,
    pub dimension_names: [String; 4],
    pub channel_indices: Vec<usize>,
    pub start: [usize; 2],
    pub shape: [usize; 4],
    pub logical_indices: [usize; 4],
    pub shard_key: String,
    pub shard_size: Option<u64>,
    pub encoded_range: Option<[u64; 2]>,
    pub present: bool,
    /// C-order native signed bytes, [time,channel,y,x]. No normalization.
    pub values: Vec<i8>,
}
#[derive(Clone)]
struct Span {
    logical: [usize; 4],
    key: String,
    size: Option<u64>,
    range: Option<[u64; 2]>,
}
#[derive(Clone, PartialEq)]
struct LocalVersion {
    len: u64,
    modified: std::time::SystemTime,
}
fn local_version(path: &PathBuf) -> Result<Option<LocalVersion>> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => Ok(Some(LocalVersion {
            len: m.len(),
            modified: m.modified()?,
        })),
        Ok(_) => fail("AlphaEarth local keys must be ordinary files"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Prepared physical source. Any error permanently invalidates this stream.
/// Call `next_chunk` until None to establish final source validation; earlier
/// chunks are provisional until that completion event.
pub struct Prepared {
    request: Request,
    sources: Sources,
    source: Arc<Objects>,
    local: BTreeMap<PathBuf, Option<LocalVersion>>,
    keys: BTreeSet<String>,
    shape: [usize; 4],
    years: Vec<i32>,
    transform: [f64; 6],
    dimension_names: [String; 4],
    channels: Vec<usize>,
    spans: Vec<Span>,
    next: usize,
    failed: bool,
    complete: bool,
    input: Arc<OneChunk>,
    decoded: Array<crate::zarr_decode::Checked<OneChunk>>,
    metadata: Value,
    started: Instant,
    first_seconds: Option<f64>,
    index_bytes: u64,
    payload_bytes: u64,
}
impl Prepared {
    fn consumed(&self) -> (u64, u64) {
        let v = self.sources.stats();
        let s = &v["alphaearth"];
        (
            s["requests"].as_u64().unwrap_or(0),
            s["bytes"].as_u64().unwrap_or(0),
        )
    }
    fn admit(&self, requests: u64, bytes: u64) -> Result<()> {
        let (r, b) = self.consumed();
        if r.checked_add(requests)
            .is_none_or(|v| v > self.request.network_requests)
            || b.checked_add(bytes)
                .is_none_or(|v| v > self.request.network_bytes)
        {
            return fail("AlphaEarth predeclared network byte/request budget exhausted");
        }
        Ok(())
    }
    fn pin(&mut self, key: &str) -> Result<()> {
        self.keys.insert(key.into());
        if self.keys.len() > 128 {
            return fail("AlphaEarth bounded object count exceeded");
        }
        if let Some(path) = self.source.local_path(&s::StoreKey::new(key)?) {
            let now = local_version(&path)?;
            if let Some(expected) = self.local.get(&path) {
                if *expected != now {
                    return fail("AlphaEarth local object changed");
                }
            } else {
                self.local.insert(path, now);
            }
        }
        Ok(())
    }
    fn validate_local(&self) -> Result<()> {
        for (p, v) in &self.local {
            if local_version(p)? != *v {
                return fail("AlphaEarth local object version conflict");
            }
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        self.validate_local()?;
        self.admit(3 * self.keys.len() as u64, 0)?;
        self.sources.validate()?;
        self.validate_local()
    }
    fn read(&mut self, key: &str, start: u64, len: u64) -> Result<s::Bytes> {
        if len == 0 || len > RANGE_CAP {
            return fail("AlphaEarth encoded range exceeds 8 MiB cap");
        }
        self.pin(key)?;
        self.validate_local()?;
        self.admit(3, 3 * (len + 1))?;
        let values = self
            .source
            .ranges(
                &s::StoreKey::new(key)?,
                &[ByteRange::FromStart(start, Some(len))],
            )?
            .ok_or("AlphaEarth selected object absent")?;
        let value = values
            .into_iter()
            .next()
            .ok_or("AlphaEarth omitted range")?;
        if value.len() as u64 != len {
            return fail("AlphaEarth truncated range");
        }
        self.validate_local()?;
        Ok(value)
    }
    fn size(&mut self, key: &str) -> Result<Option<u64>> {
        self.pin(key)?;
        self.admit(3, 0)?;
        self.source.size(key)
    }
    fn json(&mut self, key: &str) -> Result<Value> {
        self.pin(key)?;
        self.admit(3, 3 * (METADATA_CAP as u64 + 1))?;
        let v = self.source.json(key)?;
        self.validate_local()?;
        Ok(v)
    }
    pub fn metadata(&self) -> &Value {
        &self.metadata
    }
    pub fn report(&self) -> Value {
        json!({"profile":"alphaearth_geozarr_int8_v1","complete":self.complete,"failed":self.failed,
            "source_shape":self.shape,"years":self.years,"transform":self.transform,
            "dimension_names":self.dimension_names,"channel_indices":self.channels,
            "logical_chunk_shape":INNER,"physical_shard_shape":OUTER,
            "index_bytes":self.index_bytes,"selected_compressed_payload_bytes":self.payload_bytes,
            "range_coalescing_overfetch_bytes":0,"duplicate_payload_ranges":0,
            "decoded_logical_bytes":self.spans.len()*DECODED,
            "requested_native_bytes":self.request.count[0]*self.request.count[1]*self.channels.len(),
            "decoded_selection_overfetch_bytes":self.spans.len()*DECODED-self.request.count[0]*self.request.count[1]*self.channels.len(),
            "physical_shard_uncompressed_bytes":1024*1024*1024,
            "first_native_chunk_seconds":self.first_seconds,"seconds":self.started.elapsed().as_secs_f64(),
            "working_reservation_bytes":32*1024*1024,"working_budget_bytes":self.request.working_bytes,
            "network_budget_bytes":self.request.network_bytes,"network_budget_requests":self.request.network_requests,
            "transport":self.sources.stats(),"source_identities":self.sources.describe("alphaearth"),
            "output":"standalone_native_stream_only","channel_identifiers":"positional; source supplies no channel coordinate labels",
            "version_semantics":"strong per-object versions; not an atomic dataset snapshot"})
    }
    pub fn next_chunk(&mut self, cancelled: &AtomicBool) -> Result<Option<Chunk>> {
        if self.failed {
            return fail("AlphaEarth stream invalidated by earlier failure; prepare again");
        }
        if self.complete {
            return Ok(None);
        }
        let result = self.next_inner(cancelled);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn next_inner(&mut self, cancelled: &AtomicBool) -> Result<Option<Chunk>> {
        if cancelled.load(Ordering::Acquire) {
            return fail("AlphaEarth acquisition cancelled");
        }
        if self.next == self.spans.len() {
            self.validate()?;
            self.complete = true;
            return Ok(None);
        }
        self.validate_local()?;
        let span = self.spans[self.next].clone();
        let payload = span
            .range
            .map(|[o, n]| self.read(&span.key, o, n))
            .transpose()?;
        *self
            .input
            .0
            .lock()
            .map_err(|_| "AlphaEarth chunk lock poisoned")? = payload;
        let result = self
            .decoded
            .retrieve_chunk_opt::<Vec<i8>>(&[0, 0, 0, 0], &serial());
        self.input
            .0
            .lock()
            .map_err(|_| "AlphaEarth chunk lock poisoned")?
            .take();
        let values = result?;
        if values.len() != DECODED {
            return fail("AlphaEarth decoded native chunk size mismatch");
        }
        if cancelled.load(Ordering::Acquire) {
            return fail("AlphaEarth acquisition cancelled");
        }
        let oy = span.logical[2] * 256;
        let ox = span.logical[3] * 256;
        let y0 = oy.max(self.request.start[0]);
        let x0 = ox.max(self.request.start[1]);
        let y1 = (oy + 256).min(self.request.start[0] + self.request.count[0]);
        let x1 = (ox + 256).min(self.request.start[1] + self.request.count[1]);
        let h = y1 - y0;
        let w = x1 - x0;
        let mut selected = Vec::with_capacity(self.channels.len() * h * w);
        for c in &self.channels {
            for y in y0..y1 {
                let p = c * 256 * 256 + (y - oy) * 256 + (x0 - ox);
                selected.extend_from_slice(&values[p..p + w]);
            }
        }
        self.next += 1;
        self.first_seconds
            .get_or_insert_with(|| self.started.elapsed().as_secs_f64());
        Ok(Some(Chunk {
            year: self.years[self.request.time_index],
            time_index: self.request.time_index,
            dimension_names: self.dimension_names.clone(),
            channel_indices: self.channels.clone(),
            start: [y0, x0],
            shape: [1, self.channels.len(), h, w],
            logical_indices: span.logical,
            shard_key: span.key,
            shard_size: span.size,
            encoded_range: span.range,
            present: span.range.is_some(),
            values: selected,
        }))
    }
}
fn serial() -> CodecOptions {
    let mut c = CodecOptions::default();
    c.set_concurrent_target(1);
    c
}
#[derive(Default)]
struct OneChunk(Mutex<Option<s::Bytes>>);
impl ReadableStorageTraits for OneChunk {
    fn get(&self, _: &s::StoreKey) -> std::result::Result<s::MaybeBytes, s::StorageError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn get_partial_many<'a>(
        &'a self,
        _: &s::StoreKey,
        _: s::byte_range::ByteRangeIterator<'a>,
    ) -> std::result::Result<s::MaybeBytesIterator<'a>, s::StorageError> {
        Err(s::StorageError::Other(
            "whole selected inner chunk only".into(),
        ))
    }
    fn size_key(&self, _: &s::StoreKey) -> std::result::Result<Option<u64>, s::StorageError> {
        Ok(self.0.lock().unwrap().as_ref().map(|v| v.len() as u64))
    }
    fn supports_get_partial(&self) -> bool {
        false
    }
}
fn array_shape<const N: usize>(value: &Value) -> Result<[usize; N]> {
    let v: Vec<usize> = serde_json::from_value(value.clone())?;
    let a: [usize; N] = v.try_into().map_err(|_| "AlphaEarth rank mismatch")?;
    if a.contains(&0) {
        return fail("AlphaEarth nonempty dimensions required");
    }
    Ok(a)
}
fn regular(metadata: &Value) -> Result<()> {
    if metadata["zarr_format"] != 3
        || metadata["node_type"] != "array"
        || metadata["chunk_grid"]["name"] != "regular"
        || metadata["chunk_key_encoding"]
            != json!({"name":"default","configuration":{"separator":"/"}})
        || metadata
            .get("storage_transformers")
            .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return fail("unsupported AlphaEarth regular v3 storage profile");
    }
    Ok(())
}
fn coordinate(meta: &Value, name: &str, n: usize, dtype: &str) -> Result<()> {
    regular(meta)?;
    if meta["shape"] != json!([n])
        || meta["chunk_grid"]["configuration"]["chunk_shape"] != json!([n])
        || meta["dimension_names"] != json!([name])
        || meta["data_type"] != dtype
        || meta["codecs"] != json!([{"name":"bytes","configuration":{"endian":"little"}}])
    {
        return fail(
            "AlphaEarth coordinates require one raw little-endian chunk and explicit axis",
        );
    }
    Ok(())
}
/// Prepare the named, fixed-layout profile. No ambient credentials are read.
pub fn prepare(request: Request) -> Result<Prepared> {
    let started = Instant::now();
    if request.working_bytes < 32 * 1024 * 1024 {
        return fail("AlphaEarth requires a conservative 32 MiB working reservation");
    }
    if request.network_requests < 6 || request.network_bytes < 6 * (METADATA_CAP as u64 + 1) {
        return fail("AlphaEarth network budget too small for bounded metadata admission");
    }
    if request.count.contains(&0) || request.count.iter().any(|v| *v > 512) {
        return fail("AlphaEarth prototype window count must be 1..512 per axis");
    }
    let channels = request
        .channels
        .clone()
        .unwrap_or_else(|| (0..64).collect());
    if channels.is_empty()
        || channels.iter().any(|c| *c >= 64)
        || channels.iter().copied().collect::<BTreeSet<_>>().len() != channels.len()
    {
        return fail("AlphaEarth channels must be unique positional indices in 0..64");
    }
    let asset = crate::Asset {
        id: "alphaearth".into(),
        locator: request.locator.clone(),
        etag: None,
        format: crate::Format::ZarrV3,
        datetime: None,
    };
    let storage_request = crate::Request {
        source: crate::Source::Assets {
            assets: vec![asset.clone()],
        },
        spatial: crate::Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [0., 0., 1., 1.],
        },
        time: None,
        variables: vec![],
        quality: vec![],
        grid: crate::Grid::Native,
        operations: vec![],
        sink: crate::Sink::Callback,
        budget: Budget {
            buffer_bytes: 2 * METADATA_CAP,
            max_nodes: 128,
            chunk_edge: 256,
        },
    };
    let mut local = BTreeMap::new();
    if !request.locator.starts_with("https://") && !request.locator.starts_with("http://") {
        for key in ["zarr.json", ".zgroup"] {
            let p = PathBuf::from(&request.locator).join(key);
            local.insert(p.clone(), local_version(&p)?);
        }
    }
    let sources = Sources::alphaearth(&storage_request)?;
    let source = sources.get(&asset)?;
    let input = Arc::new(OneChunk::default());
    // Create the bounded decoder only after source metadata admission below.
    let dummy = json!({"zarr_format":3,"node_type":"array","shape":INNER,"data_type":"int8","chunk_grid":{"name":"regular","configuration":{"chunk_shape":INNER}},"chunk_key_encoding":{"name":"default","configuration":{"separator":"/"}},"fill_value":-128,"codecs":[{"name":"bytes","configuration":{"endian":"little"}}],"attributes":{},"dimension_names":["time","band","y","x"]});
    let decode_budget = Budget {
        buffer_bytes: request.working_bytes,
        max_nodes: 128,
        chunk_edge: 256,
    };
    let decoded =
        crate::zarr_decode::array_v3_alphaearth(input.clone(), "chunk", dummy, &decode_budget)?;
    let mut p = Prepared {
        request,
        sources,
        source,
        local,
        keys: BTreeSet::from(["zarr.json".into()]),
        shape: [1, 64, 1, 1],
        years: vec![],
        transform: [0.; 6],
        dimension_names: ["time".into(), "band".into(), "y".into(), "x".into()],
        channels,
        spans: vec![],
        next: 0,
        failed: false,
        complete: false,
        input,
        decoded,
        metadata: Value::Null,
        started,
        first_seconds: None,
        index_bytes: 0,
        payload_bytes: 0,
    };
    let root = p.json("zarr.json")?;
    if root["zarr_format"] != 3
        || root["node_type"] != "group"
        || root
            .get("consolidated_metadata")
            .is_some_and(|v| !v.is_null())
    {
        return fail("AlphaEarth requires unconsolidated v3 metadata");
    }
    let attr = &root["attributes"];
    if attr["proj:code"]!="EPSG:4326" || attr["spatial:dimensions"]!=json!(["y","x"]) || attr["spatial:registration"]!="pixel" || attr["spatial:transform_type"]!="affine" || attr["geoemb:dimensions"]!=64 || attr["geoemb:data_type"]!="int8" || attr["geoemb:quantization"]["nodata"]!=-128 || attr["geoemb:model"]!="https://developers.google.com/earth-engine/datasets/catalog/GOOGLE_SATELLITE_EMBEDDING_V1_ANNUAL" {return fail("AlphaEarth GeoZarr identity, CRS or registration mismatch");}
    p.transform = serde_json::from_value(attr["spatial:transform"].clone())?;
    let [a, b, c, d, e, f] = p.transform;
    if p.transform.iter().any(|v| !v.is_finite()) || a <= 0. || e >= 0. || b != 0. || d != 0. {
        return fail("AlphaEarth affine requires eastward x, north-to-south y and no rotation");
    }
    let metadata = p.json("embeddings/zarr.json")?;
    regular(&metadata)?;
    p.shape = array_shape(&metadata["shape"])?;
    if p.shape[2..].iter().any(|n| *n > isize::MAX as usize / 8) {
        return fail("AlphaEarth spatial shape exceeds safe bounded offset arithmetic");
    }
    if p.shape[1] != 64
        || metadata["data_type"] != "int8"
        || metadata["fill_value"] != -128
        || array_shape::<4>(&metadata["chunk_grid"]["configuration"]["chunk_shape"])? != OUTER
        || metadata["dimension_names"] != json!(["time", "band", "y", "x"])
        || attr["spatial:shape"] != json!([p.shape[2], p.shape[3]])
    {
        return fail("AlphaEarth Int8 rank-four band/y/x physical profile mismatch");
    }
    let bbox: [f64; 4] = serde_json::from_value(attr["spatial:bbox"].clone())
        .map_err(|_| "AlphaEarth requires an explicit four-number spatial bbox")?;
    let expected_bbox = [c, f + e * p.shape[2] as f64, c + a * p.shape[3] as f64, f];
    if bbox.iter().zip(expected_bbox).any(|(stored, expected)| {
        !stored.is_finite() || (*stored - expected).abs() > 1e-10_f64.max(expected.abs() * 1e-12)
    }) {
        return fail("AlphaEarth spatial bbox disagrees with affine pixel corners and shape");
    }
    let codecs = &metadata["codecs"];
    let expected = json!([{"name":"sharding_indexed","configuration":{"chunk_shape":INNER,"codecs":[{"name":"bytes","configuration":{"endian":"little"}},{"name":"zstd","configuration":{"level":3,"checksum":false}}],"index_codecs":[{"name":"bytes","configuration":{"endian":"little"}},{"name":"crc32c"}],"index_location":"end"}}]);
    if *codecs != expected {
        return fail(
            "AlphaEarth requires qualified end-index bytes/CRC32C and bytes/Zstd pipeline",
        );
    }
    if p.shape[0] > 64 || p.request.time_index >= p.shape[0] {
        return fail("AlphaEarth time index outside bounded annual coordinate");
    }
    for axis in 0..2 {
        if p.request.start[axis]
            .checked_add(p.request.count[axis])
            .is_none_or(|v| v > p.shape[axis + 2])
        {
            return fail("AlphaEarth spatial window outside source");
        }
    }
    let time = p.json("time/zarr.json")?;
    coordinate(&time, "time", p.shape[0], "int32")?;
    if p.size("time/c/0")? != Some((p.shape[0] * 4) as u64) {
        return fail("AlphaEarth year coordinate size mismatch");
    }
    p.years = p
        .read("time/c/0", 0, (p.shape[0] * 4) as u64)?
        .chunks_exact(4)
        .map(|v| i32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    if p.years.iter().any(|v| !(1900..=2200).contains(v))
        || p.years.windows(2).any(|v| v[1] <= v[0])
    {
        return fail("AlphaEarth years must be explicit increasing integer years");
    }
    let mut axis_evidence = json!({});
    for (name, n, start, count, slope, origin) in [
        (
            "y",
            p.shape[2],
            p.request.start[0],
            p.request.count[0],
            e,
            f,
        ),
        (
            "x",
            p.shape[3],
            p.request.start[1],
            p.request.count[1],
            a,
            c,
        ),
    ] {
        let meta = p.json(&format!("{name}/zarr.json"))?;
        coordinate(&meta, name, n, "float64")?;
        let key = format!("{name}/c/0");
        if p.size(&key)? != Some((n as u64) * 8) {
            return fail("AlphaEarth spatial coordinate object size mismatch");
        }
        let mut samples = BTreeSet::from([0, n - 1, n / 2, start, start + count - 1]);
        for boundary in [
            256,
            4096,
            ((start / 256) + 1) * 256,
            ((start / 4096) + 1) * 4096,
        ] {
            for i in [boundary.saturating_sub(1), boundary] {
                if i < n {
                    samples.insert(i);
                }
            }
        }
        let mut evidence = Vec::new();
        for i in samples {
            let bytes = p.read(&key, (i as u64) * 8, 8)?;
            let value = f64::from_le_bytes(bytes.as_ref().try_into()?);
            let expected = origin + slope * (i as f64 + 0.5);
            if !value.is_finite()
                || (value - expected).abs() > 1e-10_f64.max(expected.abs() * 1e-12)
            {
                return fail("AlphaEarth stored coordinate disagrees with pixel-centre affine");
            }
            evidence.push(json!({"index":i,"stored":value,"affine":expected}));
        }
        axis_evidence[name] = json!(evidence);
    }
    let original = Array::new_with_metadata(
        p.source.clone(),
        "/embeddings",
        serde_json::from_value(metadata.clone())?,
    )?;
    let codec = original
        .codecs()
        .array_to_bytes_codec()
        .as_any()
        .downcast_ref::<ShardingCodec>()
        .ok_or("AlphaEarth sharding decoder missing")?
        .clone();
    let mut indexes: BTreeMap<String, (Option<u64>, Vec<u64>)> = BTreeMap::new();
    for y in p.request.start[0] / 256..=(p.request.start[0] + p.request.count[0] - 1) / 256 {
        for x in p.request.start[1] / 256..=(p.request.start[1] + p.request.count[1] - 1) / 256 {
            let logical = [p.request.time_index, 0, y, x];
            let shard = [
                p.request.time_index as u64,
                0,
                (y / 16) as u64,
                (x / 16) as u64,
            ];
            let key = original.chunk_key(&shard).to_string();
            if !indexes.contains_key(&key) {
                let size = p.size(&key)?;
                let decoded = if let Some(size) = size {
                    if size < INDEX as u64 {
                        return fail("AlphaEarth truncated shard index");
                    }
                    let bytes = p.read(&key, size - INDEX as u64, INDEX as u64)?;
                    p.index_bytes += INDEX as u64;
                    let counts = [1, 1, 16, 16].map(|v| NonZeroU64::new(v).unwrap());
                    let decoded = codec.decode_index(&bytes, &counts, &serial())?;
                    validate_index(&decoded, size)?;
                    decoded
                } else {
                    vec![u64::MAX; 512]
                };
                indexes.insert(key.clone(), (size, decoded));
            }
            let (size, index) = &indexes[&key];
            let entry = ((y % 16) * 16 + x % 16) * 2;
            let range = (index[entry] != u64::MAX).then(|| [index[entry], index[entry + 1]]);
            if let Some([_, n]) = range {
                if n > RANGE_CAP {
                    return fail("AlphaEarth selected encoded chunk exceeds 8 MiB");
                }
                p.payload_bytes = p
                    .payload_bytes
                    .checked_add(n)
                    .ok_or("AlphaEarth payload budget overflow")?;
            }
            p.spans.push(Span {
                logical,
                key,
                size: *size,
                range,
            });
        }
    }
    // At most nine logical chunks for the bounded 512x512 window.
    if p.spans.len() > 9 {
        return fail("AlphaEarth logical window exceeds nine-chunk limit");
    }
    p.admit(
        p.spans.len() as u64 + 6 * p.keys.len() as u64,
        p.payload_bytes,
    )?;
    let mut inner = metadata.clone();
    inner["shape"] = json!(INNER);
    inner["chunk_grid"]["configuration"]["chunk_shape"] = json!(INNER);
    inner["codecs"] = metadata["codecs"][0]["configuration"]["codecs"].clone();
    p.decoded =
        crate::zarr_decode::array_v3_alphaearth(p.input.clone(), "chunk", inner, &decode_budget)?;
    p.metadata = json!({"profile":"alphaearth_geozarr_int8_v1","root":root,"embeddings":metadata,"years":p.years,"coordinate_validation":axis_evidence,"selection":{"time_index":p.request.time_index,"year":p.years[p.request.time_index],"start":p.request.start,"count":p.request.count,"channel_indices":p.channels},"physical_read_granularity":INNER,"channel_identifiers":"positional; source supplies no labels"});
    p.admit(3 * p.keys.len() as u64, 0)?;
    p.sources.seal()?;
    p.validate_local()?;
    p.sources.phase("alphaearth_stream");
    Ok(p)
}
fn validate_index(decoded: &[u64], size: u64) -> Result<()> {
    if decoded.len() != 512 {
        return fail("AlphaEarth shard index entry count mismatch");
    }
    let mut ranges = Vec::new();
    for pair in decoded.chunks_exact(2) {
        if pair[0] == u64::MAX || pair[1] == u64::MAX {
            if pair != [u64::MAX, u64::MAX] {
                return fail("AlphaEarth mixed missing sentinel");
            }
            continue;
        }
        let end = pair[0]
            .checked_add(pair[1])
            .ok_or("AlphaEarth shard offset overflow")?;
        if pair[1] == 0 || end > size - INDEX as u64 {
            return fail("AlphaEarth shard offset exceeds payload or overlaps index");
        }
        ranges.push((pair[0], end));
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|v| v[0].1 > v[1].0) {
        return fail("AlphaEarth overlapping chunk offsets");
    }
    Ok(())
}
