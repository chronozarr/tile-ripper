//! Restricted physical HDF5 acquisition. All format parsing, index traversal and
//! filter/numeric decoding belongs to hdf5-reader; no whole-file read fallback.
use crate::{fail, model::*, storage::ObjectFile, Pixels, Result};
use hdf5_reader::{
    btree_v1::{
        collect_btree_v1_leaves_storage, collect_btree_v1_leaves_storage_selective, BTreeV1Key,
    },
    messages::{layout::DataLayout, HdfMessage},
    Dataset, Datatype, Hdf5File, OpenOptions, SliceInfo, SliceInfoElem, Storage, StorageBuffer,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

struct Metadata {
    file: ObjectFile,
    cap: usize,
    max_reads: usize,
    page: usize,
    state: Mutex<MetadataState>,
}
#[derive(Default)]
struct MetadataState {
    ranges: BTreeMap<u64, Vec<u8>>,
    bytes: usize,
    sealed: bool,
    index_nodes: BTreeSet<u64>,
}
impl Metadata {
    fn cached(&self, start: u64, len: usize) -> Option<Vec<u8>> {
        let state = self.state.lock().unwrap();
        // Overlapping metadata reads can start before the nearest cached key.
        state
            .ranges
            .range(..=start)
            .rev()
            .find_map(|(offset, bytes)| {
                let a = usize::try_from(start - offset).ok()?;
                bytes.get(a..a.checked_add(len)?).map(|v| v.to_vec())
            })
    }
    fn bytes(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.bytes + state.ranges.len() * 96 + state.index_nodes.len() * 64
    }
    fn note_index_node(&self, start: u64, bytes: &[u8]) -> hdf5_reader::error::Result<()> {
        if bytes.len() >= 8 && &bytes[..5] == b"TREE\x01" {
            let mut state = self.state.lock().unwrap();
            if !state.index_nodes.contains(&start) {
                if state.index_nodes.len() >= self.max_reads
                    || state
                        .bytes
                        .checked_add(state.ranges.len() * 96 + (state.index_nodes.len() + 1) * 64)
                        .is_none_or(|n| n > self.cap)
                {
                    return Err(hdf5_reader::error::Error::InvalidData(
                        "HDF5 index node bookkeeping exceeds budget".into(),
                    ));
                }
                state.index_nodes.insert(start);
            }
        }
        Ok(())
    }
}
impl Storage for Metadata {
    fn len(&self) -> u64 {
        self.file.size
    }
    fn read_range(&self, start: u64, len: usize) -> hdf5_reader::error::Result<StorageBuffer> {
        let error = |s: &str| hdf5_reader::error::Error::InvalidData(s.to_owned());
        if len > self.cap || start.checked_add(len as u64).is_none_or(|n| n > self.len()) {
            return Err(error("HDF5 metadata range exceeds explicit bounds"));
        }
        if let Some(bytes) = self.cached(start, len) {
            self.note_index_node(start, &bytes)?;
            return Ok(StorageBuffer::from_vec(bytes));
        }
        let mut state = self.state.lock().unwrap();
        if state.sealed {
            return Err(error(
                "HDF5 decoder attempted an unplanned metadata/payload range",
            ));
        }
        let fetch_start = if self.page > 0 {
            start / self.page as u64 * self.page as u64
        } else {
            start
        };
        let fetch_end = if self.page > 0 {
            (start + len as u64)
                .div_ceil(self.page as u64)
                .saturating_mul(self.page as u64)
                .min(self.len())
        } else {
            start + len as u64
        };
        let fetch_len =
            usize::try_from(fetch_end - fetch_start).map_err(|_| error("metadata overflow"))?;
        let reservation = fetch_len
            .checked_add(96)
            .and_then(|n| n.checked_add(state.bytes))
            .ok_or_else(|| error("metadata overflow"))?;
        if reservation
            .checked_add(state.index_nodes.len() * 64 + 64)
            .is_none_or(|n| n > self.cap)
            || state.ranges.len() >= self.max_reads
        {
            return Err(error("HDF5 metadata cache exceeds buffer/max_nodes budget"));
        }
        let bytes = self
            .file
            .read(fetch_start, fetch_len)
            .map_err(|e| error(&e.to_string()))?;
        state.bytes = reservation;
        let offset =
            usize::try_from(start - fetch_start).map_err(|_| error("metadata overflow"))?;
        let selected = bytes[offset..offset + len].to_vec();
        state.ranges.insert(fetch_start, bytes);
        drop(state);
        self.note_index_node(start, &selected)?;
        Ok(StorageBuffer::from_vec(selected))
    }
}
struct View {
    metadata: Arc<Metadata>,
    start: u64,
    payload: Arc<[u8]>,
}
impl Storage for View {
    fn len(&self) -> u64 {
        self.metadata.len()
    }
    fn read_range(&self, start: u64, len: usize) -> hdf5_reader::error::Result<StorageBuffer> {
        if let Some(a) = start
            .checked_sub(self.start)
            .and_then(|n| usize::try_from(n).ok())
        {
            if let Some(bytes) = a.checked_add(len).and_then(|b| self.payload.get(a..b)) {
                return Ok(StorageBuffer::from_vec(bytes.to_vec()));
            }
        }
        self.metadata
            .cached(start, len)
            .map(StorageBuffer::from_vec)
            .ok_or_else(|| {
                hdf5_reader::error::Error::InvalidData(
                    "HDF5 decoder attempted an unplanned metadata/payload range".into(),
                )
            })
    }
}
fn open(storage: Arc<dyn Storage>) -> Result<Hdf5File> {
    Ok(Hdf5File::from_storage_with_options(
        storage,
        OpenOptions {
            chunk_cache_bytes: 0,
            chunk_cache_slots: 1,
            ..Default::default()
        },
    )?)
}
fn attrs(dataset: &Dataset, file: &Hdf5File) -> Result<Value> {
    let header = file.get_or_parse_header(dataset.address())?;
    if header
        .messages
        .iter()
        .any(|m| matches!(m, HdfMessage::ExternalFiles(_)))
    {
        return fail("external data storage is outside the admitted HDF5 profile");
    }
    let explicit_storage_fill = header
        .messages
        .iter()
        .any(|m| matches!(m, HdfMessage::FillValue(fill) if fill.value.is_some()));
    // Dataset::attributes includes the library's bounded dense-attribute heap
    // traversal. Header messages alone omit real NetCDF-4 scientific attributes.
    let attributes = dataset.attributes();
    for attr in &attributes {
        if attr.name == "_FillValue" && !explicit_storage_fill {
            // The upstream reader substitutes this attribute for a default
            // HDF5 fill. Reject that ambiguous layout instead of changing its
            // stored initialization semantics for unallocated data.
            return fail(
                "CF _FillValue with implicit HDF5 initialization is outside the admitted profile",
            );
        }
    }
    stored_attrs(attributes)
}
fn stored_attrs(attributes: Vec<hdf5_reader::Attribute>) -> Result<Value> {
    use base64::Engine;
    let mut out = serde_json::Map::new();
    for attr in attributes {
        let mut value = json!({"dtype":format!("{:?}",attr.datatype),"shape":attr.shape,
            "stored_bytes_base64":base64::engine::general_purpose::STANDARD.encode(&attr.raw_data)});
        match &attr.datatype {
            Datatype::String { .. }
            | Datatype::VarLen {
                kind: hdf5_reader::VarLenKind::String,
                ..
            } => value["value"] = json!(attr.read_string()?),
            Datatype::FixedPoint { .. } | Datatype::FloatingPoint { .. }
                if attr.num_elements()? == 1 =>
            {
                let number = attr.read_as_f64()?;
                if number.is_finite() {
                    value["value"] = json!(number);
                } else {
                    value["nonfinite"] = json!(number.to_string());
                }
            }
            Datatype::FixedPoint {
                signed: false,
                size: 1 | 2 | 4,
                ..
            } if ["flag_values", "flag_masks"].contains(&attr.name.as_str()) => {
                value["value"] = match &attr.datatype {
                    Datatype::FixedPoint { size: 1, .. } => json!(attr.read_1d::<u8>()?),
                    Datatype::FixedPoint { size: 2, .. } => json!(attr.read_1d::<u16>()?),
                    _ => json!(attr.read_1d::<u32>()?),
                };
            }
            _ => value["uninterpreted"] = json!(true),
        }
        out.insert(attr.name, value);
    }
    Ok(Value::Object(out))
}

fn marker(dataset: &Dataset, name: &str, dtype: &str) -> Result<Option<f64>> {
    let attribute = match dataset.attribute(name) {
        Ok(value) => value,
        Err(hdf5_reader::error::Error::AttributeNotFound(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if attribute.num_elements()? != 1 {
        return fail("HDF5 missing markers must be numeric scalars");
    }
    let value = attribute.read_as_f64()?;
    if ["|u1", "<u2", "<u4"].contains(&dtype) {
        let max = match dtype {
            "|u1" => u8::MAX as f64,
            "<u2" => u16::MAX as f64,
            _ => u32::MAX as f64,
        };
        if !value.is_finite() || value.fract() != 0. || !(0. ..=max).contains(&value) {
            return fail("HDF5 missing marker is not representable in the stored unsigned type");
        }
        Ok(Some(value))
    } else {
        let native = value as f32;
        if value.is_finite() && !native.is_finite() {
            return fail("HDF5 missing marker overflows Float32");
        }
        Ok(Some(native as f64))
    }
}
fn missing_dataset(error: &hdf5_reader::error::Error) -> bool {
    match error {
        hdf5_reader::error::Error::DatasetNotFound(_) => true,
        hdf5_reader::error::Error::Context { source, .. } => missing_dataset(source),
        _ => false,
    }
}

fn dtype(dataset: &Dataset) -> Result<&'static str> {
    match dataset.dtype() {
        Datatype::FixedPoint {
            size: 1,
            signed: false,
            ..
        } => Ok("|u1"),
        Datatype::FixedPoint {
            size: 4,
            signed: false,
            ..
        } => Ok("<u4"),
        Datatype::FixedPoint {
            size: 2,
            signed: false,
            ..
        } => Ok("<u2"),
        Datatype::FloatingPoint { size: 4, .. } => Ok("<f4"),
        _ => fail("native HDF5 supports only stored UInt8, UInt16, UInt32 or IEEE Float32 data"),
    }
}
#[derive(Clone, Debug)]
struct Entry {
    offsets: Vec<usize>,
    address: u64,
    size: usize,
}
fn entries(file: &Hdf5File, dataset: &Dataset, first: &[u64], last: &[u64]) -> Result<Vec<Entry>> {
    let header = file.get_or_parse_header(dataset.address())?;
    let mut layout = None;
    let mut filter_count = 0;
    for msg in &header.messages {
        match msg {
            HdfMessage::DataLayout(l) => layout = Some(&l.layout),
            HdfMessage::FilterPipeline(p) => {
                if p.filters.len() > 3 || p.filters.iter().any(|f| ![1, 2, 3].contains(&f.id)) {
                    return fail("unsupported HDF5 filter; admitted pipeline has at most three deflate/shuffle/Fletcher32 filters");
                }
                filter_count = p.filters.len();
            }
            _ => (),
        }
    }
    let Some(DataLayout::Chunked {
        address,
        dims,
        chunk_indexing: None,
        element_size,
    }) = layout
    else {
        return fail("native HDF5 data requires layout v1-v3 with v1 B-tree chunk index");
    };
    if ![1, 2].contains(&dims.len())
        || dims.len() != dataset.ndim()
        || dataset.chunks().as_deref() != Some(dims.as_slice())
        || usize::try_from(*element_size)? != hdf5_reader::dtype_element_size(dataset.dtype())?
    {
        return fail("unsupported legacy HDF5 chunk dimensionality or element size");
    }
    let sb = file.superblock();
    if *address == u64::MAX {
        return Ok(Vec::new());
    }
    let leaves = if index_policy()? == "full" {
        collect_btree_v1_leaves_storage(
            file.storage(),
            *address,
            sb.offset_size,
            sb.length_size,
            Some(dims.len() as u32),
            dims,
            Some((first, last)),
        )?
    } else {
        collect_btree_v1_leaves_storage_selective(
            file.storage(),
            *address,
            sb.offset_size,
            sb.length_size,
            dims,
            first,
            last,
        )?
    };
    leaves
        .into_iter()
        .map(|(key, address)| {
            let BTreeV1Key::RawData {
                chunk_size,
                offsets,
                filter_mask,
            } = key
            else {
                return fail("invalid HDF5 chunk key");
            };
            if filter_mask >> filter_count != 0 {
                return fail("invalid HDF5 filter mask");
            }
            let size = usize::try_from(chunk_size)?;
            if size == 0
                || address
                    .checked_add(size as u64)
                    .is_none_or(|end| end > file.storage().len())
            {
                return fail("invalid HDF5 compressed chunk extent");
            }
            Ok(Entry {
                offsets: offsets[..dims.len()]
                    .iter()
                    .map(|v| usize::try_from(*v))
                    .collect::<std::result::Result<_, _>>()?,
                address,
                size,
            })
        })
        .collect()
}
fn index_policy() -> Result<String> {
    let policy = std::env::var("TILE_RIPPER_HDF5_INDEX_POLICY").unwrap_or_else(|_| "full".into());
    if !matches!(policy.as_str(), "full" | "selective") {
        return fail("invalid HDF5 index policy");
    }
    Ok(policy)
}
fn metadata_page() -> Result<usize> {
    match std::env::var("TILE_RIPPER_HDF5_METADATA_POLICY")
        .as_deref()
        .unwrap_or("exact")
    {
        "exact" => Ok(0),
        "page4k" => Ok(4096),
        "page16k" => Ok(16384),
        _ => fail("invalid HDF5 metadata policy"),
    }
}
fn selection(start: &[usize; 2], count: &[usize; 2], step: &[usize; 2]) -> Result<SliceInfo> {
    let selections = (0..2)
        .map(|i| {
            let end = count[i]
                .checked_sub(1)
                .and_then(|n| n.checked_mul(step[i]))
                .and_then(|n| n.checked_add(start[i]))
                .and_then(|n| n.checked_add(1))
                .ok_or("HDF5 selection overflow")?;
            Ok(SliceInfoElem::Slice {
                start: start[i] as u64,
                end: end as u64,
                step: step[i] as u64,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SliceInfo { selections })
}
fn decode(dataset: &Dataset, selected: &SliceInfo) -> Result<Pixels> {
    Ok(match dtype(dataset)? {
        "|u1" => Pixels::UInt8(
            dataset
                .read_slice_selective_v1::<u8>(selected)?
                .into_raw_vec_and_offset()
                .0,
        ),
        "<u4" => Pixels::UInt32(
            dataset
                .read_slice_selective_v1::<u32>(selected)?
                .into_raw_vec_and_offset()
                .0,
        ),
        "<u2" => Pixels::UInt16(
            dataset
                .read_slice_selective_v1::<u16>(selected)?
                .into_raw_vec_and_offset()
                .0,
        ),
        _ => Pixels::Float32(
            dataset
                .read_slice_selective_v1::<f32>(selected)?
                .into_raw_vec_and_offset()
                .0,
        ),
    })
}

struct CachedAxis {
    values: Vec<f64>,
    description: Value,
}
#[derive(Default)]
pub(crate) struct Context {
    pub(crate) transport: crate::http_transport::Transport,
    files: Mutex<BTreeMap<String, Arc<Metadata>>>,
    bindings: Mutex<BTreeMap<(String, String), Arc<Binding>>>,
    axes: Mutex<BTreeMap<(String, String), CachedAxis>>,
}
#[derive(Clone, Debug)]
struct Job {
    node: usize,
    start: [usize; 2],
    count: [usize; 2],
    coordinate_start: [usize; 2],
    entry: Option<Entry>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Group {
    pub nodes: Vec<usize>,
    start: u64,
    len: usize,
    selected: usize,
    pub reservation: usize,
}
pub(crate) struct Binding {
    metadata: Arc<Metadata>,
    path: String,
    step: [usize; 2],
    coordinates: [Vec<f64>; 2],
    layout: Layout,
    jobs: Vec<Job>,
    pub groups: Vec<Group>,
    description: Value,
}
fn coordinate(ds: &Dataset, start: usize, count: usize, step: usize) -> Result<Vec<f64>> {
    let end = (count - 1)
        .checked_mul(step)
        .and_then(|n| n.checked_add(start + 1))
        .ok_or("coordinate selection overflow")?;
    let selected = SliceInfo {
        selections: vec![SliceInfoElem::Slice {
            start: start as u64,
            end: end as u64,
            step: step as u64,
        }],
    };
    macro_rules! read {
        ($ty:ty) => {
            ds.read_slice_selective_v1::<$ty>(&selected)?
                .iter()
                .map(|v| *v as f64)
                .collect()
        };
    }
    let values: Vec<f64> =
        match ds.dtype() {
            Datatype::FixedPoint {
                size: 2,
                signed: true,
                ..
            } => read!(i16),
            Datatype::FixedPoint {
                size: 2,
                signed: false,
                ..
            } => read!(u16),
            Datatype::FixedPoint {
                size: 4,
                signed: true,
                ..
            } => read!(i32),
            Datatype::FixedPoint {
                size: 4,
                signed: false,
                ..
            } => read!(u32),
            Datatype::FloatingPoint { size: 4, .. } => read!(f32),
            Datatype::FloatingPoint { size: 8, .. } => read!(f64),
            _ => return fail(
                "unsupported coordinate dtype; requires int16/uint16/int32/uint32/float32/float64",
            ),
        };
    if values.len() != count || values.iter().any(|v| !v.is_finite()) {
        return fail("nonfinite/invalid HDF5 coordinates");
    }
    Ok(values)
}
fn chunk_indices(
    start: usize,
    count: usize,
    step: usize,
    block: usize,
    cap: usize,
) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < count {
        if out.len() >= cap {
            return fail("HDF5 chunk selection exceeds max_nodes");
        }
        let chunk = (start + i * step) / block;
        out.push(chunk);
        i = ((chunk + 1)
            .checked_mul(block)
            .ok_or("chunk bound overflow")?
            - start)
            .div_ceil(step)
            .max(i + 1);
    }
    Ok(out)
}
// Payloads never enter the metadata cache. hdf5-reader owns index/filter parsing;
// a closed View permits only one admitted chunk and already discovered metadata.
fn chunked_coordinate(
    metadata: &Arc<Metadata>,
    file: &Hdf5File,
    path: &str,
    axis: &Dataset,
    selected: [usize; 3],
    budget: &Budget,
    coordinate_bytes: usize,
) -> Result<(Vec<f64>, Value)> {
    let [start, count, step] = selected;
    let chunks = axis.chunks().ok_or("missing coordinate chunk shape")?;
    if chunks.len() != 1 || chunks[0] == 0 {
        return fail("HDF5 chunked coordinates require a nonzero rank-one chunk shape");
    }
    let block = usize::try_from(chunks[0])?;
    let native = block
        .checked_mul(hdf5_reader::dtype_element_size(axis.dtype())?)
        .ok_or("HDF5 coordinate chunk size overflow")?;
    if native > budget.buffer_bytes / 8 {
        return fail("one decoded HDF5 coordinate chunk exceeds working memory budget");
    }
    let indices = chunk_indices(start, count, step, block, budget.max_nodes)?;
    if indices.len() > budget.buffer_bytes / 4096 {
        return fail("HDF5 coordinate chunk-plan bookkeeping exceeds working budget");
    }
    let mut values = Vec::with_capacity(count);
    let mut receipts = Vec::new();
    let mut peak = 0;
    for index in indices {
        metadata.file.phase("hdf5_coordinate_index_discovery");
        let bounds = [index as u64];
        let found = entries(file, axis, &bounds, &bounds)?;
        let offset = index
            .checked_mul(block)
            .ok_or("coordinate chunk overflow")?;
        if found.len() != 1 || found[0].offsets != [offset] {
            return fail("absent or invalid selected HDF5 coordinate chunk");
        }
        let entry = &found[0];
        let reservation = native
            .checked_mul(8)
            .and_then(|n| n.checked_add(entry.size.checked_mul(3)?))
            .and_then(|n| n.checked_add(metadata.bytes().checked_mul(4)?))
            .and_then(|n| n.checked_add(coordinate_bytes))
            .and_then(|n| n.checked_add(receipts.len().checked_mul(2048)?))
            .and_then(|n| n.checked_add(32768))
            .ok_or("HDF5 coordinate working reservation overflow")?;
        if reservation > budget.buffer_bytes {
            return fail("HDF5 coordinate decoding exceeds working memory budget");
        }
        peak = peak.max(reservation);
        let first = offset.saturating_sub(start).div_ceil(step);
        let end = offset
            .checked_add(block)
            .ok_or("coordinate chunk overflow")?
            .min(usize::try_from(axis.shape()[0])?)
            .saturating_sub(start)
            .div_ceil(step)
            .min(count);
        let at = start + first * step;
        metadata.file.phase("hdf5_coordinate_chunks");
        let payload = metadata.file.read(entry.address, entry.size)?;
        let view = open(Arc::new(View {
            metadata: metadata.clone(),
            start: entry.address,
            payload: payload.into(),
        }))?;
        let decoded = coordinate(&view.dataset(path)?, at, end - first, step)?;
        values.extend(decoded);
        receipts.push(json!({"start":at,"count":end-first,"step":step,
            "chunk_offset":offset,"byte_offset":entry.address,"encoded_bytes":entry.size}));
    }
    metadata.file.phase("metadata_index_discovery");
    if values.len() != count {
        return fail("HDF5 coordinate chunk coverage does not match selection");
    }
    Ok((
        values,
        json!({"layout":"chunked","chunk_shape":chunks,
        "chunk_index":"v1 B-tree","index_policy":index_policy()?,
        "decoded_chunk_bytes":native,"prepare_reserved_bytes_peak":peak,"chunks":receipts}),
    ))
}

fn validate_coordinate_markers(axis: &Dataset, values: &[f64]) -> Result<()> {
    for name in ["_FillValue", "missing_value"] {
        let marker = match axis.attribute(name) {
            Ok(attr) => attr,
            Err(hdf5_reader::error::Error::AttributeNotFound(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        if marker.num_elements()? != 1 {
            return fail("HDF5 coordinate missing markers must be numeric scalars");
        }
        let missing = marker.read_as_f64()?;
        if values.contains(&missing) {
            return fail("selected HDF5 coordinates contain a missing-value marker");
        }
    }
    Ok(())
}
impl Context {
    pub fn binding(&self, asset: &str, path: &str) -> Result<Arc<Binding>> {
        self.bindings
            .lock()
            .unwrap()
            .get(&(asset.to_owned(), path.to_owned()))
            .cloned()
            .ok_or_else(|| "missing prepared HDF5 dataset".into())
    }
    pub fn bytes(&self) -> usize {
        let files = self.files.lock().unwrap();
        let bindings = self.bindings.lock().unwrap();
        self.axes
            .lock()
            .unwrap()
            .values()
            .map(|a| a.values.capacity() * 8 + a.description.to_string().len() * 4)
            .sum::<usize>()
            + files.values().map(|m| m.bytes() * 4).sum::<usize>()
            + self.transport.reservation()
            + bindings
                .values()
                .map(|b| {
                    b.coordinates
                        .iter()
                        .map(|v| v.capacity() * 8)
                        .sum::<usize>()
                        + b.jobs.len() * 512
                        + b.groups.len() * 256
                        + b.description.to_string().len() * 4
                })
                .sum::<usize>()
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_cancellable(None)
    }
    pub fn validate_cancellable(
        &self,
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        for m in self.files.lock().unwrap().values() {
            m.state.lock().unwrap().sealed = true;
            m.file.validate_cancellable(cancelled)?;
        }
        Ok(())
    }
    pub fn phase(&self, phase: &'static str) {
        for m in self.files.lock().unwrap().values() {
            m.file.phase(phase);
        }
    }
    pub fn describe(&self, id: &str) -> Option<Value> {
        let files = self.files.lock().unwrap();
        let m = files.get(id)?;
        let bindings = self.bindings.lock().unwrap();
        Some(
            json!({"identity":m.file.identity(),"file_bytes":m.file.size,"metadata_cache_bytes":m.bytes(),"index_nodes_visited":m.state.lock().unwrap().index_nodes.len(),"metadata_page_bytes":m.page,"datasets":bindings.iter().filter(|((a,_),_)|a==id).map(|(_,b)|b.description.clone()).collect::<Vec<_>>()}),
        )
    }
    pub fn add_stats(&self, value: &mut Value) {
        for (id, m) in self.files.lock().unwrap().iter() {
            value[id] = m.file.stats();
            value[id]["cached_metadata_bytes"] = json!(m.bytes());
        }
    }
    pub fn plan(&self, asset: &Asset, r: &Request, p: &mut Plan) -> Result<()> {
        let Spatial::ArraySlice {
            start,
            count,
            step,
            axes,
        } = &r.spatial
        else {
            return fail("HDF5 requires explicit array_slice selection and coordinate paths");
        };
        if !matches!(r.sink, Sink::Callback) || r.time.is_some() {
            return fail("native HDF5 currently supports callback/stream output and explicit index selection without time filtering");
        }
        if r.variables.is_empty() {
            return fail("HDF5 requires dataset selectors");
        }
        let metadata = {
            let mut files = self.files.lock().unwrap();
            if let Some(m) = files.get(&asset.id) {
                m.clone()
            } else {
                let m = Arc::new(Metadata {
                    file: ObjectFile::open(asset, &r.budget, self.transport.clone())?,
                    cap: r.budget.buffer_bytes / 32,
                    max_reads: r.budget.max_nodes,
                    page: metadata_page()?,
                    state: Mutex::default(),
                });
                files.insert(asset.id.clone(), m.clone());
                m
            }
        };
        metadata.file.phase("metadata_index_discovery");
        let file = open(metadata.clone())?;
        let sb = file.superblock();
        if sb.base_address != 0 || sb.eof_address != metadata.file.size || sb.consistency_flags != 0
        {
            return fail("HDF5 requires closed ordinary single-file storage without a userblock or trailing bytes");
        }
        let root_attributes = stored_attrs(file.root_group()?.attributes()?)?;
        for variable in &r.variables {
            let Variable::Dataset { path } = variable else {
                return fail("HDF5 requires absolute dataset paths");
            };
            if !path.starts_with('/') || path.contains("..") {
                return fail("HDF5 dataset path must be absolute and confined");
            }
            let ds = file.dataset(path)?;
            if ds.ndim() != 2 {
                return fail("native HDF5 data must have rank two");
            }
            let dt = dtype(&ds)?;
            let shape = [
                usize::try_from(ds.shape()[0])?,
                usize::try_from(ds.shape()[1])?,
            ];
            let cs = ds.chunks().ok_or("native HDF5 requires chunked data")?;
            let chunk = [usize::try_from(cs[0])?, usize::try_from(cs[1])?];
            if chunk.contains(&0) {
                return fail("invalid HDF5 chunk shape");
            }
            for i in 0..2 {
                let end = (count[i] - 1)
                    .checked_mul(step[i])
                    .and_then(|n| n.checked_add(start[i]))
                    .ok_or("HDF5 selection overflow")?;
                if end >= shape[i] {
                    return fail("HDF5 selection outside source shape");
                }
            }
            let coordinate_bytes = count
                .iter()
                .try_fold(0usize, |n, c| {
                    c.checked_mul(16).and_then(|v| n.checked_add(v))
                })
                .ok_or("coordinate size overflow")?;
            if coordinate_bytes > r.budget.buffer_bytes / 16 {
                return fail("HDF5 selected coordinate arrays exceed working budget");
            }
            let attributes = attrs(&ds, &file)?;
            for packing in ["scale_factor", "add_offset"] {
                if attributes.get(packing).is_some()
                    && attributes[packing]["value"].as_f64().is_none()
                {
                    return fail("HDF5 packing attributes must be finite numeric scalars");
                }
            }
            let mut coordinate_attrs = Vec::new();
            let mut coordinates = [Vec::new(), Vec::new()];
            let coordinate_working_bytes = coordinate_bytes
                .checked_add(self.bytes())
                .ok_or("HDF5 coordinate preparation reservation overflow")?;
            for i in 0..2 {
                let axis = file.dataset(&axes[i])?;
                let axis_header = file.get_or_parse_header(axis.address())?;
                if axis.shape() != [shape[i] as u64] {
                    return fail("explicit HDF5 coordinate shape does not match dataset dimension");
                }
                let key = (asset.id.clone(), axes[i].clone());
                if let Some(axis) = self.axes.lock().unwrap().get(&key) {
                    coordinates[i] = axis.values.clone();
                    coordinate_attrs.push(axis.description.clone());
                    continue;
                }
                let attributes = attrs(&axis, &file)?;
                let chunked = axis_header.messages.iter().any(|m| {
                    matches!(m,HdfMessage::DataLayout(l) if matches!(l.layout,DataLayout::Chunked {..}))
                });
                let storage = if chunked {
                    let (values, storage) = chunked_coordinate(
                        &metadata,
                        &file,
                        &axes[i],
                        &axis,
                        [start[i], count[i], step[i]],
                        &r.budget,
                        coordinate_working_bytes,
                    )?;
                    coordinates[i] = values;
                    storage
                } else {
                    coordinates[i] = coordinate(&axis, start[i], count[i], step[i])?;
                    json!({"layout":"contiguous_or_compact"})
                };
                validate_coordinate_markers(&axis, &coordinates[i])?;
                let description = json!({"path":axes[i],"dtype":format!("{:?}",axis.dtype()),"attributes":attributes,"storage":storage});
                self.axes.lock().unwrap().insert(
                    key,
                    CachedAxis {
                        values: coordinates[i].clone(),
                        description: description.clone(),
                    },
                );
                coordinate_attrs.push(description);
            }
            let mapping = attributes["grid_mapping"]["value"]
                .as_str()
                .map(|name| {
                    let parent = path.rsplit_once('/').map_or("", |v| v.0);
                    let mapping_path = format!("{parent}/{name}");
                    let mapping = file.dataset(&mapping_path)?;
                    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                        json!({"path":mapping_path,"attributes":attrs(&mapping,&file)?}),
                    )
                })
                .transpose()?;
            // Time remains metadata. CF calendars and filtering are not inferred.
            let time = match file.dataset("/t") {
                Ok(t)
                    if t.ndim() == 0
                        && matches!(t.dtype(), Datatype::FloatingPoint { size: 8, .. }) =>
                {
                    Some(
                        json!({"path":"/t","stored_value":t.read_array::<f64>()?.iter().next(),"attributes":attrs(&t,&file)?}),
                    )
                }
                Ok(_) => {
                    return fail("optional root /t metadata currently requires a Float64 scalar")
                }
                Err(error) if missing_dataset(&error) => None,
                Err(error) => return Err(error.into()),
            };
            let number = |name: &str| attributes[name]["value"].as_f64();
            let storage_fill = if let Some(fill) = ds.fill_value().and_then(|f| f.value.as_ref()) {
                Some(match dt {
                    "|u1" => <u8 as hdf5_reader::H5Type>::from_bytes(fill, ds.dtype())? as f64,
                    "<u2" => <u16 as hdf5_reader::H5Type>::from_bytes(fill, ds.dtype())? as f64,
                    "<u4" => <u32 as hdf5_reader::H5Type>::from_bytes(fill, ds.dtype())? as f64,
                    _ => <f32 as hdf5_reader::H5Type>::from_bytes(fill, ds.dtype())? as f64,
                })
            } else {
                None
            };
            let cf_fill = marker(&ds, "_FillValue", dt)?;
            let missing = marker(&ds, "missing_value", dt)?;
            if let (Some(a), Some(b)) = (cf_fill, missing) {
                if a != b && !(a.is_nan() && b.is_nan()) {
                    return fail(
                        "distinct missing_value and _FillValue require a richer validity contract",
                    );
                }
            }
            // HDF5 fill initializes unallocated chunks. Scientific missingness
            // follows CF markers and NaNs, independently of initialization.
            let nodata = cf_fill.or(missing);
            let layout = Layout {
                width: shape[1],
                height: shape[0],
                transform: [0., 1., 0., 0., 0., 1.],
                crs: format!("HDF5:stored_axes:{},{}", axes[1], axes[0]),
                variable: variable.clone(),
                block: [chunk[1], chunk[0]],
                dtype: dt.into(),
                nodata,
                scale: number("scale_factor"),
                offset: number("add_offset"),
                tiles: Vec::new(),
            };
            let rows = chunk_indices(start[0], count[0], step[0], chunk[0], r.budget.max_nodes)?;
            let cols = chunk_indices(start[1], count[1], step[1], chunk[1], r.budget.max_nodes)?;
            // Bound the pending jobs and their duplicated inspection/DAG records
            // before constructing them, including all-unallocated large arrays.
            if rows
                .len()
                .checked_mul(cols.len())
                .is_none_or(|n| n > r.budget.buffer_bytes / 4096)
            {
                return fail("HDF5 chunk-plan bookkeeping exceeds working budget");
            }
            if rows
                .len()
                .checked_mul(cols.len())
                .and_then(|n| n.checked_mul(2))
                .and_then(|n| n.checked_add(p.nodes.len() + 1))
                .is_none_or(|n| n > r.budget.max_nodes)
            {
                return fail("HDF5 logical chunks exceed max_nodes");
            }
            let inspect = p.nodes.len();
            p.nodes.push(Node {
                id: inspect,
                dependencies: vec![],
                asset_id: asset.id.clone(),
                operation: Operation::Inspect { cached: false },
            });
            let mut jobs = Vec::new();
            let mut payload = 0u64;
            metadata.file.phase("hdf5_chunk_index_discovery");
            for row in &rows {
                for col in &cols {
                    let offsets = [row * chunk[0], col * chunk[1]];
                    let mut job_start = [0; 2];
                    let mut job_count = [0; 2];
                    let mut coordinate_start = [0; 2];
                    for i in 0..2 {
                        let first = offsets[i].saturating_sub(start[i]).div_ceil(step[i]);
                        let end = (offsets[i] + chunk[i])
                            .min(shape[i])
                            .saturating_sub(start[i])
                            .div_ceil(step[i])
                            .min(count[i]);
                        coordinate_start[i] = first;
                        job_start[i] = start[i] + first * step[i];
                        job_count[i] = end - first;
                    }
                    let bounds = [*row as u64, *col as u64];
                    let mut found = entries(&file, &ds, &bounds, &bounds)?;
                    if found.len() > 1 || found.first().is_some_and(|e| e.offsets != offsets) {
                        return fail("invalid or duplicate HDF5 chunk index entry");
                    }
                    let entry = found.pop();
                    payload = payload
                        .checked_add(entry.as_ref().map_or(0, |e| e.size as u64))
                        .ok_or("HDF5 payload overflow")?;
                    let id = p.nodes.len();
                    p.nodes.push(Node {
                        id,
                        dependencies: vec![inspect],
                        asset_id: asset.id.clone(),
                        operation: Operation::Read {
                            read: Read::DatasetSlice {
                                path: path.clone(),
                                start: job_start.to_vec(),
                                count: job_count.to_vec(),
                            },
                        },
                    });
                    p.nodes.push(Node {
                        id: id + 1,
                        dependencies: vec![id],
                        asset_id: asset.id.clone(),
                        operation: Operation::Decode {
                            reader: "hdf5-reader 0.9.3 native stored values".into(),
                        },
                    });
                    jobs.push(Job {
                        node: id,
                        start: job_start,
                        count: job_count,
                        coordinate_start,
                        entry,
                    });
                }
            }
            metadata.file.phase("metadata_index_discovery");
            let native = chunk[0]
                .checked_mul(chunk[1])
                .and_then(|n| n.checked_mul(hdf5_reader::dtype_element_size(ds.dtype()).ok()?))
                .ok_or("HDF5 decoded chunk overflow")?;
            if native > r.budget.buffer_bytes / 8 {
                return fail("one decoded HDF5 chunk exceeds working memory budget");
            }
            let policy = std::env::var("TILE_RIPPER_HDF5_RANGE_POLICY")
                .unwrap_or_else(|_| "adjacent".into());
            if !["individual", "adjacent", "bounded"].contains(&policy.as_str()) {
                return fail("HDF5 range policy must be individual, adjacent or bounded");
            }
            // HDF5-specific conservative defaults, independently benchmarked.
            let max_span = (256 * 1024).min(r.budget.buffer_bytes / 16);
            let max_gap = if policy == "bounded" { 16 * 1024 } else { 0 };
            let mut groups: Vec<Group> = Vec::new();
            for job in &jobs {
                if let (Some(last), Some(entry)) = (groups.last_mut(), &job.entry) {
                    let end = last.start + last.len as u64;
                    let span = entry
                        .address
                        .checked_add(entry.size as u64)
                        .and_then(|n| n.checked_sub(last.start))
                        .and_then(|n| usize::try_from(n).ok());
                    if policy != "individual"
                        && last.selected > 0
                        && last.nodes.len() < 32
                        && entry.address >= end
                        && entry.address - end <= max_gap
                        && span.is_some_and(|span| {
                            span <= max_span
                                && span - last.selected - entry.size
                                    <= (last.selected + entry.size) / 4
                        })
                    {
                        last.nodes.push(job.node);
                        last.len = span.unwrap();
                        last.selected += entry.size;
                        continue;
                    }
                }
                groups.push(Group {
                    nodes: vec![job.node],
                    start: job.entry.as_ref().map_or(0, |e| e.address),
                    len: job.entry.as_ref().map_or(0, |e| e.size),
                    selected: job.entry.as_ref().map_or(0, |e| e.size),
                    reservation: 0,
                });
            }
            for group in &mut groups {
                group.reservation = native
                    .checked_mul(8)
                    .and_then(|n| n.checked_add(group.len.checked_mul(3)?))
                    .and_then(|n| n.checked_add(metadata.bytes().checked_mul(4)?))
                    .and_then(|n| n.checked_add(coordinate_bytes))
                    .and_then(|n| n.checked_add(32768))
                    .ok_or("HDF5 working reservation overflow")?;
            }
            let description = json!({"path":path,"shape":shape,"source_dtype":format!("{:?}",ds.dtype()),"stored_dtype":dt,"chunk_shape":chunk,"superblock_version":sb.version,"chunk_index":"v1 B-tree","index_policy":index_policy()?,"index_validation_scope":"visited nodes and visible pointers; full policy traverses all nodes","storage_fill":storage_fill.map(|v|if v.is_finite(){json!(v)}else{json!(v.to_string())}),"missingness":"CF scalar markers and NaNs; storage initialization alone is valid","attributes":attributes,"root_attributes":root_attributes,"axes":coordinate_attrs,"axis_semantics":"stored coordinate values as exact Float64; apply axis scale/offset explicitly","grid_mapping":mapping,"time":time,"range_policy":policy,"physical_groups":groups,"chunks":jobs.iter().map(|j|json!({"node":j.node,"start":j.start,"count":j.count,"step":step,"byte_offset":j.entry.as_ref().map(|e|e.address),"encoded_bytes":j.entry.as_ref().map_or(0,|e|e.size)})).collect::<Vec<_>>()});
            p.selected.push(Selected {
                timestamps: vec![],
                dimensions: None,
                mask_geometry: None,
                asset_id: asset.id.clone(),
                datetime: asset.datetime.clone(),
                reason: "explicit native rank-two array slice; stored axes and values".into(),
                capabilities: Capabilities {
                    selection: vec![
                        "array_slice".into(),
                        "positive_strides".into(),
                        "explicit_coordinate_paths".into(),
                    ],
                    reads: vec![
                        "bounded_v1_btree_index_scan".into(),
                        "conditional_compressed_chunk_ranges".into(),
                    ],
                    decoding: "hdf5-reader 0.9.3".into(),
                    compression: "none/deflate/shuffle/Fletcher32".into(),
                    materialization: true,
                },
                layout: Some(layout.clone()),
                window: Window {
                    x: start[1],
                    y: start[0],
                    width: count[1],
                    height: count[0],
                },
            });
            p.estimates.selected_compressed_payload_bytes = p
                .estimates
                .selected_compressed_payload_bytes
                .and_then(|n| n.checked_add(payload));
            p.limitations.push("HDF5 callbacks only: explicit rank-two slices; actual stored axis centers accompany every chunk; transform is an array-index placeholder, not geolocation. Apply scientific and coordinate scale/offset explicitly. CF time remains metadata.".into());
            self.bindings.lock().unwrap().insert(
                (asset.id.clone(), path.clone()),
                Arc::new(Binding {
                    metadata: metadata.clone(),
                    path: path.clone(),
                    step: *step,
                    coordinates,
                    layout,
                    jobs,
                    groups,
                    description,
                }),
            );
            if self.bytes() > r.budget.buffer_bytes / 2 {
                return fail(
                    "aggregate prepared HDF5 metadata/coordinates/plan exceeds half buffer_bytes",
                );
            }
        }
        Ok(())
    }
}

// An execution-local encoded range cache. It is charged at its fixed cap and
// cannot outlive final identity validation or cross an asset/version boundary.
struct RangeCache {
    cap: usize,
    bytes: usize,
    entries: std::collections::VecDeque<(u64, Arc<[u8]>)>,
    hits: usize,
}
impl RangeCache {
    fn read(
        &mut self,
        metadata: &Metadata,
        entry: Option<&Entry>,
        report: &mut crate::ExecutionReport,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Arc<[u8]>> {
        let Some(entry) = entry else {
            return Ok(Arc::from([]));
        };
        if let Some(index) = self
            .entries
            .iter()
            .position(|(a, b)| *a == entry.address && b.len() == entry.size)
        {
            let item = self.entries.remove(index).unwrap();
            let bytes = item.1.clone();
            self.entries.push_back(item);
            self.hits += 1;
            return Ok(bytes);
        }
        let bytes: Arc<[u8]> = metadata
            .file
            .read_cancellable(entry.address, entry.size, Some(cancelled))?
            .into();
        report.source_data_requests = Some(report.source_data_requests.unwrap_or(0) + 1);
        report.source_data_bytes = Some(report.source_data_bytes.unwrap_or(0) + entry.size as u64);
        if bytes.len() <= self.cap {
            while self.bytes + bytes.len() > self.cap {
                let (_, old) = self.entries.pop_front().unwrap();
                self.bytes -= old.len();
            }
            self.bytes += bytes.len();
            self.entries.push_back((entry.address, bytes.clone()));
        }
        Ok(bytes)
    }
}
fn payload_phase(path: &str) -> &'static str {
    match path {
        "/wse" => "hdf5_wse_payload",
        "/wse_qual" => "hdf5_summary_quality_payload",
        "/wse_qual_bitwise" => "hdf5_bitwise_quality_payload",
        "/wse_uncert" => "hdf5_uncertainty_payload",
        "/n_wse_pix" => "hdf5_pixel_count_payload",
        _ => "hdf5_science_payload",
    }
}
impl Context {
    pub fn execute_aligned(
        &self,
        asset: &Asset,
        request: &Request,
        planning_seconds: f64,
        cancelled: &std::sync::atomic::AtomicBool,
        mut sink: impl FnMut([crate::Chunk; 5]) -> Result<()>,
    ) -> Result<crate::ExecutionReport> {
        use std::sync::atomic::Ordering;
        let started = std::time::Instant::now();
        let bindings = crate::swot::PATHS
            .iter()
            .map(|path| self.binding(&asset.id, path))
            .collect::<Result<Vec<_>>>()?;
        let wse = &bindings[0];
        let cache_cap = request.budget.buffer_bytes / 8;
        let mut work = 0;
        for b in &bindings {
            let native = b.layout.block[0]
                .checked_mul(b.layout.block[1])
                .and_then(|n| n.checked_mul(if b.layout.dtype == "|u1" { 1 } else { 4 }))
                .ok_or("SWOT native chunk overflow")?;
            let encoded = b
                .jobs
                .iter()
                .filter_map(|j| j.entry.as_ref().map(|e| e.size))
                .max()
                .unwrap_or(0);
            work = work.max(
                native
                    .checked_mul(8)
                    .and_then(|n| n.checked_add(encoded.checked_mul(3)?))
                    .ok_or("SWOT decoder reservation overflow")?,
            );
        }
        let base = self
            .bytes()
            .checked_add(cache_cap)
            .and_then(|n| n.checked_add(work))
            .and_then(|n| n.checked_add(65536))
            .ok_or("SWOT reservation overflow")?;
        let available = request
            .budget
            .buffer_bytes
            .checked_sub(base)
            .ok_or("SWOT aligned decoder/cache exceeds working budget")?;
        // 17 bytes of raw values + mask, five copies of coordinates and
        // conservative assembly slack, including narrow one-row/column tiles.
        let max_pixels = available / 128;
        if max_pixels == 0 {
            return fail("SWOT aligned output exceeds working budget");
        }
        let mut cache = RangeCache {
            cap: cache_cap,
            bytes: 0,
            entries: Default::default(),
            hits: 0,
        };
        let mut report = crate::execute::empty_report(planning_seconds);
        let mut tiles = 0usize;
        let mut peak = base;
        for job in &wse.jobs {
            let width = job.count[1].min(max_pixels);
            let height = job.count[0].min(max_pixels / width);
            for row in (0..job.count[0]).step_by(height) {
                for col in (0..job.count[1]).step_by(width) {
                    if cancelled.load(Ordering::Acquire) {
                        return fail("acquisition cancelled");
                    }
                    tiles += 1;
                    if tiles > request.budget.max_nodes {
                        return fail("SWOT aligned tiles exceed max_nodes");
                    }
                    let count = [
                        height.min(job.count[0] - row),
                        width.min(job.count[1] - col),
                    ];
                    let first = [job.coordinate_start[0] + row, job.coordinate_start[1] + col];
                    let last = [first[0] + count[0], first[1] + count[1]];
                    let n = count[0] * count[1];
                    peak = peak.max(base + 18 * n + 40 * (count[0] + count[1]));
                    let source_start = [
                        job.start[0] + row * wse.step[0],
                        job.start[1] + col * wse.step[1],
                    ];
                    let mut layers = Vec::new();
                    for b in &bindings {
                        if cancelled.load(Ordering::Acquire) {
                            return fail("acquisition cancelled");
                        }
                        let mut pixels = match b.layout.dtype.as_str() {
                            "|u1" => Pixels::UInt8(vec![0; n]),
                            "<u4" => Pixels::UInt32(vec![0; n]),
                            "<f4" => Pixels::Float32(vec![0.; n]),
                            _ => return fail("unqualified SWOT stored type"),
                        };
                        let mut covered = 0;
                        for part in &b.jobs {
                            let a = [
                                first[0].max(part.coordinate_start[0]),
                                first[1].max(part.coordinate_start[1]),
                            ];
                            let z = [
                                last[0].min(part.coordinate_start[0] + part.count[0]),
                                last[1].min(part.coordinate_start[1] + part.count[1]),
                            ];
                            if a[0] >= z[0] || a[1] >= z[1] {
                                continue;
                            }
                            if cancelled.load(Ordering::Acquire) {
                                return fail("acquisition cancelled");
                            }
                            b.metadata.file.phase(payload_phase(&b.path));
                            let clock = std::time::Instant::now();
                            let payload = cache.read(
                                &b.metadata,
                                part.entry.as_ref(),
                                &mut report,
                                cancelled,
                            )?;
                            let view = Arc::new(View {
                                metadata: b.metadata.clone(),
                                start: part.entry.as_ref().map_or(0, |e| e.address),
                                payload,
                            });
                            let file = open(view)?;
                            let at = [
                                part.start[0] + (a[0] - part.coordinate_start[0]) * b.step[0],
                                part.start[1] + (a[1] - part.coordinate_start[1]) * b.step[1],
                            ];
                            let size = [z[0] - a[0], z[1] - a[1]];
                            let values =
                                decode(&file.dataset(&b.path)?, &selection(&at, &size, &b.step)?)?;
                            for y in 0..size[0] {
                                pixels.copy(
                                    (a[0] - first[0] + y) * count[1] + a[1] - first[1],
                                    &values,
                                    y * size[1],
                                    size[1],
                                )?;
                            }
                            covered += size[0] * size[1];
                            report.read_decode_seconds += clock.elapsed().as_secs_f64();
                            if part.entry.is_some() {
                                report.decoded_source_chunks += 1;
                            } else {
                                report.filled_source_chunks += 1;
                            }
                        }
                        if covered != n {
                            return fail(
                                "SWOT logical alignment has missing or duplicate coverage",
                            );
                        }
                        report.chunks += 1;
                        report.decoded_bytes += pixels.byte_len();
                        report.non_nodata_pixels +=
                            (0..n).filter(|i| pixels.valid(*i, b.layout.nodata)).count();
                        layers.push(crate::Chunk {
                            node_id: job.node,
                            asset_id: asset.id.clone(),
                            datetime: asset.datetime.clone(),
                            variable: b.layout.variable.clone(),
                            indices: vec![],
                            window: Window {
                                x: source_start[1],
                                y: source_start[0],
                                width: count[1],
                                height: count[0],
                            },
                            transform: [
                                source_start[1] as f64 - b.step[1] as f64 / 2.,
                                b.step[1] as f64,
                                0.,
                                source_start[0] as f64 - b.step[0] as f64 / 2.,
                                0.,
                                b.step[0] as f64,
                            ],
                            crs: b.layout.crs.clone(),
                            nodata: b.layout.nodata,
                            scale: b.layout.scale,
                            offset: b.layout.offset,
                            pixels,
                            coordinates: Some((
                                b.coordinates[1][first[1]..last[1]].to_vec(),
                                b.coordinates[0][first[0]..last[0]].to_vec(),
                            )),
                        });
                    }
                    report
                        .first_output_seconds
                        .get_or_insert(started.elapsed().as_secs_f64());
                    let clock = std::time::Instant::now();
                    sink(
                        layers
                            .try_into()
                            .map_err(|_| "missing aligned SWOT layers")?,
                    )?;
                    report.sink_seconds += clock.elapsed().as_secs_f64();
                }
            }
        }
        report.scheduler = json!({"profile":"bounded aligned SWOT tiles","workers":1,"queue_capacity":0,"reserved_bytes_peak":peak,"budget_bytes":request.budget.buffer_bytes,"encoded_cache_cap_bytes":cache_cap,"encoded_cache_hits":cache.hits,"tiles":tiles,"max_tile_pixels":max_pixels,"redecoded_intersections":report.decoded_source_chunks});
        Ok(report)
    }
}

pub(crate) struct SelectedReader {
    binding: Arc<Binding>,
    view: Arc<View>,
    file: Hdf5File,
}
impl SelectedReader {
    pub fn new(
        asset: &Asset,
        selected: &Selected,
        context: &Context,
        nodes: &[usize],
    ) -> Result<Self> {
        let Variable::Dataset { path } = &selected
            .layout
            .as_ref()
            .ok_or("missing HDF5 layout")?
            .variable
        else {
            return fail("missing HDF5 dataset path");
        };
        let binding = context.binding(&asset.id, path)?;
        let group = binding
            .groups
            .iter()
            .find(|g| g.nodes == nodes)
            .ok_or("missing HDF5 physical group")?;
        binding.metadata.file.phase(payload_phase(&binding.path));
        let payload = binding.metadata.file.read(group.start, group.len)?;
        let view = Arc::new(View {
            metadata: binding.metadata.clone(),
            start: group.start,
            payload: payload.into(),
        });
        let file = open(view.clone())?;
        Ok(Self {
            binding,
            view,
            file,
        })
    }
    pub fn execute_node(
        &mut self,
        asset: &Asset,
        node: &Node,
        report: &mut crate::ExecutionReport,
        sink: &mut impl FnMut(crate::Chunk) -> Result<()>,
    ) -> Result<()> {
        let job = self
            .binding
            .jobs
            .iter()
            .find(|j| j.node == node.id)
            .ok_or("unplanned HDF5 chunk")?;
        let clock = std::time::Instant::now();
        let ds = self.file.dataset(&self.binding.path)?;
        let pixels = decode(&ds, &selection(&job.start, &job.count, &self.binding.step)?)?;
        report.read_decode_seconds += clock.elapsed().as_secs_f64();
        if job.entry.is_some() {
            report.decoded_source_chunks += 1;
        } else {
            report.filled_source_chunks += 1;
        }
        report.chunks += 1;
        report.decoded_bytes += pixels.byte_len();
        report.non_nodata_pixels += (0..pixels.len())
            .filter(|i| pixels.valid(*i, self.binding.layout.nodata))
            .count();
        let xy = (
            self.binding.coordinates[1]
                [job.coordinate_start[1]..job.coordinate_start[1] + job.count[1]]
                .to_vec(),
            self.binding.coordinates[0]
                [job.coordinate_start[0]..job.coordinate_start[0] + job.count[0]]
                .to_vec(),
        );
        let chunk = crate::Chunk {
            node_id: node.id,
            asset_id: asset.id.clone(),
            datetime: asset.datetime.clone(),
            variable: self.binding.layout.variable.clone(),
            indices: vec![],
            window: Window {
                x: job.start[1],
                y: job.start[0],
                width: job.count[1],
                height: job.count[0],
            },
            transform: [
                job.start[1] as f64 - self.binding.step[1] as f64 / 2.,
                self.binding.step[1] as f64,
                0.,
                job.start[0] as f64 - self.binding.step[0] as f64 / 2.,
                0.,
                self.binding.step[0] as f64,
            ],
            crs: self.binding.layout.crs.clone(),
            nodata: self.binding.layout.nodata,
            scale: self.binding.layout.scale,
            offset: self.binding.layout.offset,
            pixels,
            coordinates: Some(xy),
        };
        let clock = std::time::Instant::now();
        sink(chunk)?;
        report.sink_seconds += clock.elapsed().as_secs_f64();
        Ok(())
    }
    pub fn finish(&self, report: &mut crate::ExecutionReport) {
        report.source_data_requests = Some(
            report.source_data_requests.unwrap_or(0) + u64::from(!self.view.payload.is_empty()),
        );
        report.source_data_bytes =
            Some(report.source_data_bytes.unwrap_or(0) + self.view.payload.len() as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit bounded real-product experiment; TILE_RIPPER_HDF5_PROBE supplies URL"]
    fn standalone_physical_probe() -> Result<()> {
        let locator = std::env::var("TILE_RIPPER_HDF5_PROBE")?;
        let asset = Asset {
            etag: None,
            id: "goes19".into(),
            locator,
            format: Format::Hdf5,
            datetime: None,
        };
        let budget = Budget {
            buffer_bytes: 32 * 1024 * 1024,
            max_nodes: 4096,
            chunk_edge: 256,
        };
        let metadata = Arc::new(Metadata {
            file: ObjectFile::open(&asset, &budget, Default::default())?,
            cap: 2 * 1024 * 1024,
            max_reads: 4096,
            page: metadata_page()?,
            state: Mutex::default(),
        });
        let file = open(metadata.clone())?;
        let ds = file.dataset("/RRQPE")?;
        let chunks = ds.chunks().ok_or("missing chunks")?;
        let at = [2500, 2500];
        let count = [64, 64];
        let step = [1, 1];
        let first = [
            at[0] as u64 / chunks[0] as u64,
            at[1] as u64 / chunks[1] as u64,
        ];
        let last = [
            (at[0] + count[0] - 1) as u64 / chunks[0] as u64,
            (at[1] + count[1] - 1) as u64 / chunks[1] as u64,
        ];
        let resolved = entries(&file, &ds, &first, &last)?;
        if resolved.len() != 1 {
            return fail("probe expected one chunk");
        }
        let e = &resolved[0];
        let x = coordinate(&file.dataset("/x")?, at[1], count[1], step[1])?;
        let y = coordinate(&file.dataset("/y")?, at[0], count[0], step[0])?;
        metadata.state.lock().unwrap().sealed = true;
        metadata.file.phase("payload");
        let payload = metadata.file.read(e.address, e.size)?;
        let reader = open(Arc::new(View {
            metadata: metadata.clone(),
            start: e.address,
            payload: payload.into(),
        }))?;
        let output = decode(&reader.dataset("/RRQPE")?, &selection(&at, &count, &step)?)?;
        metadata.file.validate_cancellable(None)?;
        let receipt = json!({"shape":ds.shape(),"dtype":dtype(&ds)?,"chunks":chunks,"chunk_offset":e.offsets,"byte_range":[e.address,e.size],"attributes":attrs(&ds,&file)?,"metadata_cached_bytes":metadata.bytes(),"identity":metadata.file.identity(),"transport":metadata.file.stats(),"values":output.as_u16(),"coordinates":[x,y]});
        std::fs::write(
            "prototype-output/native-hdf5/rust-probe.json",
            serde_json::to_vec_pretty(&receipt)?,
        )?;
        Ok(())
    }
}
