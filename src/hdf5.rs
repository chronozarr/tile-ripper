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
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Instant,
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
    hits: usize,
    misses: usize,
    coordinate_index_traversals: usize,
    coordinate_decoder_opens: usize,
    science_decoder_opens: usize,
}
impl Metadata {
    fn cached(&self, start: u64, len: usize) -> Option<Vec<u8>> {
        let mut state = self.state.lock().unwrap();
        // Overlapping metadata reads can start before the nearest cached key.
        let found = state
            .ranges
            .range(..=start)
            .rev()
            .find_map(|(offset, bytes)| {
                let a = usize::try_from(start - offset).ok()?;
                bytes.get(a..a.checked_add(len)?).map(|v| v.to_vec())
            });
        if found.is_some() {
            state.hits += 1;
        }
        found
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
        state.misses += 1;
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

#[derive(Clone)]
struct CachedAxis {
    values: Arc<[f64]>,
    description: Value,
}

fn read_time(file: &Hdf5File) -> Result<Option<Value>> {
    // Time remains metadata. CF calendars and filtering are not inferred.
    Ok(match file.dataset("/t") {
        Ok(t) if t.ndim() == 0 && matches!(t.dtype(), Datatype::FloatingPoint { size: 8, .. }) => {
            Some(
                json!({"path":"/t","stored_value":t.read_array::<f64>()?.iter().next(),"attributes":attrs(&t,file)?}),
            )
        }
        Ok(_) => return fail("optional root /t metadata currently requires a Float64 scalar"),
        Err(error) if missing_dataset(&error) => None,
        Err(error) => return Err(error.into()),
    })
}
struct DatasetProfile {
    dataset: Dataset,
    shape: [usize; 2],
    chunk: [usize; 2],
    layout: Layout,
    description: Value,
}
// One source identity, parser, interpretation policy and bounded retained ranges.
// Mutable only during inspection. Sealing prohibits discovering new metadata.
type ProfileKey = (String, [String; 2]);
type AxisKey = (String, [usize; 3]);
struct InspectedFile {
    metadata: Arc<Metadata>,
    parser: Hdf5File,
    root_attributes: Value,
    time: Option<Value>,
    mappings: Mutex<BTreeMap<String, Value>>,
    opening: Value,
    index_policy: String,
    profiles: Mutex<BTreeMap<ProfileKey, Arc<DatasetProfile>>>,
    axes: Mutex<BTreeMap<AxisKey, CachedAxis>>,
    indexes: Mutex<BTreeMap<String, Arc<Vec<Entry>>>>,
    profile_hits: AtomicUsize,
    index_hits: AtomicUsize,
    axis_hits: AtomicUsize,
    index_scans: AtomicUsize,
    coordinate_reads: AtomicUsize,
    profile_seconds: Mutex<f64>,
    index_seconds: Mutex<f64>,
    coordinate_seconds: Mutex<f64>,
}
impl InspectedFile {
    fn new(
        asset: &Asset,
        r: &Request,
        transport: crate::http_transport::Transport,
    ) -> Result<Self> {
        let started = Instant::now();
        let object = ObjectFile::open(asset, &r.budget, transport)?;
        let object_seconds = started.elapsed().as_secs_f64();
        let metadata = Arc::new(Metadata {
            file: object,
            cap: r.budget.buffer_bytes / 32,
            max_reads: r.budget.max_nodes,
            page: metadata_page()?,
            state: Mutex::default(),
        });
        metadata.file.phase("metadata_index_discovery");
        let started = Instant::now();
        let parser = open(metadata.clone())?;
        let parser_seconds = started.elapsed().as_secs_f64();
        let sb = parser.superblock();
        if sb.base_address != 0 || sb.eof_address != metadata.file.size || sb.consistency_flags != 0
        {
            return fail("HDF5 requires closed ordinary single-file storage without a userblock or trailing bytes");
        }
        let started = Instant::now();
        let root_attributes = stored_attrs(parser.root_group()?.attributes()?)?;
        let time = read_time(&parser)?;
        let opening = json!({"source_identity_and_transport_seconds":object_seconds,"superblock_parser_open_seconds":parser_seconds,"root_group_time_metadata_seconds":started.elapsed().as_secs_f64()});
        Ok(Self {
            metadata,
            parser,
            root_attributes,
            time,
            mappings: Mutex::default(),
            opening,
            index_policy: index_policy()?,
            profiles: Mutex::default(),
            axes: Mutex::default(),
            indexes: Mutex::default(),
            profile_hits: AtomicUsize::new(0),
            index_hits: AtomicUsize::new(0),
            axis_hits: AtomicUsize::new(0),
            index_scans: AtomicUsize::new(0),
            coordinate_reads: AtomicUsize::new(0),
            profile_seconds: Mutex::new(0.),
            index_seconds: Mutex::new(0.),
            coordinate_seconds: Mutex::new(0.),
        })
    }
    fn bytes(&self) -> usize {
        self.metadata.bytes() * 4
            + (self.root_attributes.to_string().len()
                + self.time.as_ref().map_or(0, |v| v.to_string().len())
                + self.opening.to_string().len())
                * 4
            + self
                .mappings
                .lock()
                .unwrap()
                .values()
                .map(|v| v.to_string().len() * 4 + 256)
                .sum::<usize>()
            + self
                .profiles
                .lock()
                .unwrap()
                .values()
                .map(|p| p.description.to_string().len() * 4 + 4096)
                .sum::<usize>()
            + self
                .axes
                .lock()
                .unwrap()
                .values()
                .map(|a| a.values.len() * 8 + a.description.to_string().len() * 4 + 256)
                .sum::<usize>()
            + self
                .indexes
                .lock()
                .unwrap()
                .values()
                .map(|e| e.capacity() * 128 + 256)
                .sum::<usize>()
    }
    fn statistics(&self) -> Value {
        let retained = self.bytes();
        let profiles = self.profiles.lock().unwrap().len();
        let state = self.metadata.state.lock().unwrap();
        json!({"source_inspections":1,"metadata_parser_opens":1,"profile_discoveries":profiles,
            "profile_cache_hits":self.profile_hits.load(Ordering::Relaxed),"full_index_scans":self.index_scans.load(Ordering::Relaxed),
            "index_cache_hits":self.index_hits.load(Ordering::Relaxed),"coordinate_axis_reads":self.coordinate_reads.load(Ordering::Relaxed),
            "coordinate_cache_hits":self.axis_hits.load(Ordering::Relaxed),"coordinate_index_traversals":state.coordinate_index_traversals,"coordinate_decoder_opens":state.coordinate_decoder_opens,"science_decoder_opens":state.science_decoder_opens,"metadata_range_hits":state.hits,"metadata_range_misses":state.misses,
            "profile_seconds":*self.profile_seconds.lock().unwrap(),"index_seconds":*self.index_seconds.lock().unwrap(),
            "coordinate_seconds":*self.coordinate_seconds.lock().unwrap(),"retention_reservation_bytes":retained,
            "interpretation":"native-hdf5-stored-v1","index_policy":self.index_policy,"sealed":state.sealed,"opening":self.opening})
    }
    fn profile(&self, path: &str, axes: &[String; 2]) -> Result<Arc<DatasetProfile>> {
        let key = (path.to_owned(), axes.clone());
        let mut cache = self.profiles.lock().unwrap();
        if let Some(p) = cache.get(&key) {
            self.profile_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(p.clone());
        }
        if self.metadata.state.lock().unwrap().sealed {
            return fail("immutable HDF5 inspection lacks requested dataset/axis profile; inspect a fresh source");
        }
        let started = Instant::now();

        let ds = self.parser.dataset(path)?;
        let file = &self.parser;
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
        let attributes = attrs(&ds, file)?;
        for packing in ["scale_factor", "add_offset"] {
            if attributes.get(packing).is_some() && attributes[packing]["value"].as_f64().is_none()
            {
                return fail("HDF5 packing attributes must be finite numeric scalars");
            }
        }
        let mapping = attributes["grid_mapping"]["value"]
            .as_str()
            .map(|name| {
                let parent = path.rsplit_once('/').map_or("", |v| v.0);
                let mapping_path = format!("{parent}/{name}");
                let mut mappings = self.mappings.lock().unwrap();
                if let Some(mapping) = mappings.get(&mapping_path) {
                    return Ok(mapping.clone());
                }
                let mapping = file.dataset(&mapping_path)?;
                let description = json!({"path":mapping_path,"attributes":attrs(&mapping,file)?});
                mappings.insert(mapping_path, description.clone());
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(description)
            })
            .transpose()?;
        let time = &self.time;
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
            variable: Variable::Dataset { path: path.into() },
            block: [chunk[1], chunk[0]],
            dtype: dt.into(),
            nodata,
            scale: number("scale_factor"),
            offset: number("add_offset"),
            tiles: Vec::new(),
        };

        let description = json!({"path":path,"shape":shape,"source_dtype":format!("{:?}",ds.dtype()),"stored_dtype":dt,
            "chunk_shape":chunk,"chunk_index":"v1 B-tree","index_policy":self.index_policy,
            "index_validation_scope":"visited nodes and visible pointers; full policy traverses all nodes",
            "storage_fill":storage_fill.map(|v|if v.is_finite(){json!(v)}else{json!(v.to_string())}),
            "missingness":"CF scalar markers and NaNs; storage initialization alone is valid","attributes":attributes,
            "root_attributes":self.root_attributes,"axis_semantics":"stored coordinate values as exact Float64; apply axis scale/offset explicitly",
            "grid_mapping":mapping,"time":time});
        let result = Arc::new(DatasetProfile {
            dataset: ds,
            shape,
            chunk,
            layout,
            description,
        });
        cache.insert(key, result.clone());
        *self.profile_seconds.lock().unwrap() += started.elapsed().as_secs_f64();
        Ok(result)
    }
    fn entries(
        &self,
        path: &str,
        ds: &Dataset,
        first: &[u64],
        last: &[u64],
        budget: &Budget,
    ) -> Result<Vec<Entry>> {
        if self.index_policy != "full" {
            return entries(&self.parser, ds, first, last);
        }
        let mut cache = self.indexes.lock().unwrap();
        let all = if let Some(all) = cache.get(path) {
            self.index_hits.fetch_add(1, Ordering::Relaxed);
            all.clone()
        } else {
            if self.metadata.state.lock().unwrap().sealed {
                return fail(
                    "immutable HDF5 inspection lacks requested chunk index; inspect a fresh source",
                );
            }
            let chunks = ds.chunks().ok_or("missing chunk shape")?;
            let possible = ds
                .shape()
                .iter()
                .zip(&chunks)
                .try_fold(1usize, |n, (s, c)| {
                    n.checked_mul((*s as usize).div_ceil(*c as usize))
                })
                .ok_or("HDF5 index bound overflow")?;
            // Bound retained parsed entries before asking the library to collect them.
            if possible > budget.max_nodes || possible > budget.buffer_bytes / 4096 {
                return entries(&self.parser, ds, first, last);
            }
            let started = Instant::now();
            let all = Arc::new(entries(
                &self.parser,
                ds,
                &vec![0; ds.ndim()],
                &vec![u64::MAX; ds.ndim()],
            )?);
            self.index_scans.fetch_add(1, Ordering::Relaxed);
            *self.index_seconds.lock().unwrap() += started.elapsed().as_secs_f64();
            cache.insert(path.into(), all.clone());
            all
        };
        let chunks = ds.chunks().ok_or("missing chunk shape")?;
        Ok(all
            .iter()
            .filter(|e| {
                e.offsets
                    .iter()
                    .zip(&chunks)
                    .enumerate()
                    .all(|(i, (offset, chunk))| {
                        let index = (*offset as u64) / (*chunk as u64);
                        index >= first[i] && index <= last[i]
                    })
            })
            .cloned()
            .collect())
    }
    fn axis(
        &self,
        path: &str,
        shape: usize,
        selected: [usize; 3],
        budget: &Budget,
        working: usize,
    ) -> Result<CachedAxis> {
        let [start, count, step] = selected;
        let mut cache = self.axes.lock().unwrap();
        if let Some(axis) = cache.get(&(path.into(), selected)) {
            self.axis_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(axis.clone());
        }
        if let Some(axis) = cache.get(&(path.into(), [0, shape, 1])) {
            self.axis_hits.fetch_add(1, Ordering::Relaxed);
            let values = (0..count)
                .map(|i| {
                    axis.values
                        .get(start + i * step)
                        .copied()
                        .ok_or("coordinate selection outside inspected axis")
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            return Ok(CachedAxis {
                values: values.into(),
                description: axis.description.clone(),
            });
        }
        if self.metadata.state.lock().unwrap().sealed {
            return fail("immutable HDF5 inspection lacks requested coordinate coverage; inspect a fresh source");
        }
        let started = Instant::now();
        let axis = self.parser.dataset(path)?;
        if axis.shape() != [shape as u64] {
            return fail("explicit HDF5 coordinate shape does not match dataset dimension");
        }
        let attributes = attrs(&axis, &self.parser)?;
        let header = self.parser.get_or_parse_header(axis.address())?;
        let chunked = header.messages.iter().any(
            |m| matches!(m,HdfMessage::DataLayout(l) if matches!(l.layout,DataLayout::Chunked{..})),
        );
        let (values, storage) = if chunked {
            chunked_coordinate(
                &self.metadata,
                &self.parser,
                path,
                &axis,
                selected,
                budget,
                working,
            )?
        } else {
            (
                coordinate(&axis, start, count, step)?,
                json!({"layout":"contiguous_or_compact"}),
            )
        };
        validate_coordinate_markers(&axis, &values)?;
        let result = CachedAxis {
            values: values.into(),
            description: json!({"path":path,"dtype":format!("{:?}",axis.dtype()),"attributes":attributes,"storage":storage}),
        };
        self.coordinate_reads.fetch_add(1, Ordering::Relaxed);
        *self.coordinate_seconds.lock().unwrap() += started.elapsed().as_secs_f64();
        cache.insert((path.into(), selected), result.clone());
        Ok(result)
    }
}
/// Nonserializable source/version binding. Its retained metadata and axes are
/// immutable; every preparation creates separate logical bindings and validates
/// the online source before using them. No global or locator-only cache exists.
#[derive(Clone)]
pub(crate) struct GridInspection {
    asset: Asset,
    budget: Budget,
    transport: crate::http_transport::Transport,
    file: Arc<InspectedFile>,
    definition: Value,
    grid: crate::swot_aoi::GridIdentity,
    seconds: f64,
}
impl GridInspection {
    pub(crate) fn inspect(
        request: &Request,
        transport: crate::http_transport::Transport,
    ) -> Result<Self> {
        crate::planner::validate(request)?;
        let Source::Assets { assets } = &request.source else {
            return fail("grid inspection requires one native HDF5 source");
        };
        if assets.len() != 1
            || assets[0].format != Format::Hdf5
            || !matches!(request.sink, Sink::Callback)
            || !matches!(request.grid, Grid::Native)
            || request.time.is_some()
        {
            return fail("grid inspection requires one native HDF5 source, native grid and callback sink without time filtering");
        }
        let Spatial::ArraySlice { axes, .. } = &request.spatial else {
            return fail("grid inspection requires explicit coordinate paths");
        };
        if axes != &["/y".to_owned(), "/x".to_owned()] || index_policy()? != "full" {
            return fail("reusable SWOT grid inspection requires /y,/x and full index validation");
        }
        let started = Instant::now();
        let file = Arc::new(InspectedFile::new(&assets[0], request, transport.clone())?);
        let wse = file.profile(crate::swot::PATHS[0], axes)?;
        crate::swot_aoi::shape_bound(wse.shape)?;
        let coordinate_bytes = wse
            .shape
            .iter()
            .try_fold(0usize, |n, c| {
                c.checked_mul(16).and_then(|v| n.checked_add(v))
            })
            .ok_or("full coordinate reservation overflow")?;
        if coordinate_bytes > request.budget.buffer_bytes / 16 {
            return fail("full inspected coordinate arrays exceed working budget");
        }
        let mut definition = wse.description.clone();
        let mut descriptions = Vec::new();
        for (i, path) in axes.iter().enumerate() {
            let retained = file.bytes() + transport.reservation() + coordinate_bytes;
            let a = file.axis(
                path,
                wse.shape[i],
                [0, wse.shape[i], 1],
                &request.budget,
                retained,
            )?;
            descriptions.push(a.description);
            if file.bytes() + transport.reservation() > request.budget.buffer_bytes / 2 {
                return fail("retained native grid inspection exceeds half buffer_bytes");
            }
        }
        definition["axes"] = json!(descriptions);
        // Admission of CRS/axis metadata uses the existing strict SWOT contract.
        let crs = crate::swot_aoi::source_crs(&definition)?;
        let grid = {
            let axes = file.axes.lock().unwrap();
            crate::swot_aoi::grid_identity(
                &crs,
                &axes[&("/y".into(), [0, wse.shape[0], 1])].values,
                &axes[&("/x".into(), [0, wse.shape[1], 1])].values,
                &definition["grid_mapping"],
            )?
        };
        for path in crate::swot::PATHS {
            let profile = file.profile(path, axes)?;
            let possible = profile
                .shape
                .iter()
                .zip(profile.chunk)
                .try_fold(1usize, |n, (s, c)| n.checked_mul(s.div_ceil(c)))
                .ok_or("grid index bound overflow")?;
            if possible > request.budget.max_nodes || possible > request.budget.buffer_bytes / 4096
            {
                return fail("retained full HDF5 index exceeds max_nodes/working budget");
            }
            file.entries(
                path,
                &profile.dataset,
                &[0, 0],
                &[u64::MAX; 2],
                &request.budget,
            )?;
            if file.bytes() + transport.reservation() > request.budget.buffer_bytes / 2 {
                return fail("retained native grid inspection exceeds half buffer_bytes");
            }
        }
        file.metadata.state.lock().unwrap().sealed = true;
        file.metadata.file.phase("grid_inspection_validation");
        file.metadata.file.validate_cancellable(None)?;
        Ok(Self {
            asset: assets[0].clone(),
            budget: request.budget.clone(),
            transport,
            file,
            definition,
            grid,
            seconds: started.elapsed().as_secs_f64(),
        })
    }
    pub(crate) fn with_grid<T>(
        &self,
        f: impl FnOnce(&Value, &[f64], &[f64]) -> Result<T>,
    ) -> Result<T> {
        let shape: [usize; 2] = serde_json::from_value(self.definition["shape"].clone())?;
        let (y, x) = {
            let axes = self.file.axes.lock().unwrap();
            (
                axes.get(&("/y".into(), [0, shape[0], 1]))
                    .ok_or("missing inspected /y")?
                    .values
                    .clone(),
                axes.get(&("/x".into(), [0, shape[1], 1]))
                    .ok_or("missing inspected /x")?
                    .values
                    .clone(),
            )
        };
        f(&self.definition, &y, &x)
    }
    pub(crate) fn describe(&self) -> Value {
        let scientific=self.file.profiles.lock().unwrap().values().map(|p|json!({"path":p.description["path"],"shape":p.shape,"stored_dtype":p.layout.dtype,"chunk_shape":p.chunk,"chunk_index":p.description["chunk_index"]})).collect::<Vec<_>>();
        json!({"inspection_schema":1,"interpretation":"native-hdf5-stored-v1","source_identity":self.file.metadata.file.identity(),
            "object_size":self.file.metadata.file.size,"scientific_datasets":scientific,
            "admission_status":{"grid":"validated","scientific_profile":"stored definitions and indexes inspected; policy admission required for each selection","execution":"requires newly validated bounded physical plan"},"grid_definition":self.definition,"grid":self.grid,"inspection_seconds":self.seconds,
            "capabilities":{"coordinate_model":"explicit_one_dimensional_stored_centers","axis_order":["y","x"],
            "geographic_selection":"qualified_wgs84_utm_swot_v1","scientific_variables":crate::swot::PATHS,
            "execution":"rebuild_bounded_physical_plan_with_fresh_version_validation","other_crs":"unsupported","other_profiles":"unsupported"},
            "retention":self.file.statistics(),"transport":self.file.metadata.file.stats()})
    }
    pub(crate) fn context(&self, r: &Request) -> Result<Context> {
        crate::planner::validate(r)?;
        let Source::Assets { assets } = &r.source else {
            return fail("inspected source request mismatch");
        };
        if assets.len() != 1
            || assets[0].id != self.asset.id
            || assets[0].locator != self.asset.locator
            || assets[0].format != Format::Hdf5
            || assets[0].datetime != self.asset.datetime
            || r.budget.buffer_bytes != self.budget.buffer_bytes
            || r.budget.max_nodes != self.budget.max_nodes
            || index_policy()? != self.file.index_policy
            || metadata_page()? != self.file.metadata.page
        {
            return fail("inspection source, retention budget or interpretation policy mismatch; inspect a fresh source");
        }
        if let Some(etag) = &assets[0].etag {
            if self.file.metadata.file.identity()["version"]["etag"].as_str() != Some(etag.as_str())
            {
                return fail("inspection source version conflicts with requested ETag");
            }
        }
        if self.file.bytes() + self.transport.reservation() > r.budget.buffer_bytes / 2 {
            return fail("retained inspection exceeds requested memory budget");
        }
        self.file.metadata.file.phase("inspection_reuse_validation");
        self.file.metadata.file.validate_cancellable(None)?;
        let context = Context {
            transport: self.transport.clone(),
            ..Default::default()
        };
        context
            .files
            .lock()
            .unwrap()
            .insert(self.asset.id.clone(), self.file.clone());
        Ok(context)
    }
}

#[derive(Default)]
pub(crate) struct Context {
    pub(crate) transport: crate::http_transport::Transport,
    files: Mutex<BTreeMap<String, Arc<InspectedFile>>>,
    bindings: Mutex<BTreeMap<(String, String), Arc<Binding>>>,
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
        metadata.state.lock().unwrap().coordinate_index_traversals += 1;
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
        metadata.state.lock().unwrap().coordinate_decoder_opens += 1;
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
    fn file(&self, asset: &Asset, r: &Request) -> Result<Arc<InspectedFile>> {
        let mut files = self.files.lock().unwrap();
        if let Some(m) = files.get(&asset.id) {
            return Ok(m.clone());
        }
        let m = Arc::new(InspectedFile::new(asset, r, self.transport.clone())?);
        files.insert(asset.id.clone(), m.clone());
        Ok(m)
    }
    pub(crate) fn with_coordinates<T>(
        &self,
        asset: &str,
        f: impl FnOnce(&[f64], &[f64]) -> Result<T>,
    ) -> Result<T> {
        let binding = self.binding(asset, crate::swot::PATHS[0])?;
        f(&binding.coordinates[0], &binding.coordinates[1])
    }
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
        files.values().map(|m| m.bytes()).sum::<usize>()
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
            m.metadata.state.lock().unwrap().sealed = true;
            m.metadata.file.phase("hdf5_version_validation");
            m.metadata.file.validate_cancellable(cancelled)?;
        }
        Ok(())
    }
    pub fn phase(&self, phase: &'static str) {
        for m in self.files.lock().unwrap().values() {
            m.metadata.file.phase(phase);
        }
    }
    pub fn describe(&self, id: &str) -> Option<Value> {
        let files = self.files.lock().unwrap();
        let inspected = files.get(id)?;
        let m = &inspected.metadata;
        let bindings = self.bindings.lock().unwrap();
        let nodes = m.state.lock().unwrap().index_nodes.len();
        Some(
            json!({"identity":m.file.identity(),"file_bytes":m.file.size,"metadata_cache_bytes":m.bytes(),"index_nodes_visited":nodes,"metadata_page_bytes":m.page,"inspection_reuse":inspected.statistics(),"datasets":bindings.iter().filter(|((a,_),_)|a==id).map(|(_,b)|b.description.clone()).collect::<Vec<_>>()}),
        )
    }
    pub fn add_stats(&self, value: &mut Value) {
        for (id, m) in self.files.lock().unwrap().iter() {
            value[id] = m.metadata.file.stats();
            value[id]["cached_metadata_bytes"] = json!(m.metadata.bytes());
            value[id]["inspection_reuse"] = m.statistics();
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
        let inspected = self.file(asset, r)?;
        let metadata = &inspected.metadata;
        metadata.file.phase("metadata_index_discovery");
        let sb = inspected.parser.superblock();
        for variable in &r.variables {
            let Variable::Dataset { path } = variable else {
                return fail("HDF5 requires absolute dataset paths");
            };
            if !path.starts_with('/') || path.contains("..") {
                return fail("HDF5 dataset path must be absolute and confined");
            }
            let profile = inspected.profile(path, axes)?;
            let ds = &profile.dataset;
            let shape = profile.shape;
            let chunk = profile.chunk;
            let layout = profile.layout.clone();
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
            let coordinate_working_bytes = coordinate_bytes
                .checked_add(self.bytes())
                .ok_or("HDF5 coordinate preparation reservation overflow")?;
            let mut coordinate_attrs = Vec::new();
            let mut coordinates = [Vec::new(), Vec::new()];
            for i in 0..2 {
                let axis = inspected.axis(
                    &axes[i],
                    shape[i],
                    [start[i], count[i], step[i]],
                    &r.budget,
                    coordinate_working_bytes,
                )?;
                coordinates[i] = axis.values.to_vec();
                coordinate_attrs.push(axis.description);
            }
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
                    let mut found = inspected.entries(path, ds, &bounds, &bounds, &r.budget)?;
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
            let mut description = profile.description.clone();
            description["superblock_version"] = json!(sb.version);
            description["axes"] = json!(coordinate_attrs);
            description["range_policy"] = json!(policy);
            description["physical_groups"] = json!(groups);
            description["chunks"] = json!(jobs.iter().map(|j|json!({"node":j.node,"start":j.start,"count":j.count,"step":step,"byte_offset":j.entry.as_ref().map(|e|e.address),"encoded_bytes":j.entry.as_ref().map_or(0,|e|e.size)})).collect::<Vec<_>>());

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
    // The same calculation drives inspection and execution admission. The
    // sufficient estimate leaves room for the largest planned logical tile;
    // it accounts for the cache growing as buffer_bytes / 8.
    pub(crate) fn aligned_reservation(
        &self,
        asset: &Asset,
        request: &Request,
    ) -> Result<(usize, usize, Value)> {
        let bindings = crate::swot::PATHS
            .iter()
            .map(|path| self.binding(&asset.id, path))
            .collect::<Result<Vec<_>>>()?;
        let prepared = self.bytes();
        let cache = request.budget.buffer_bytes / 8;
        let mut work = 0;
        let mut limiting = "";
        let mut chunks = Vec::new();
        let mut coordinate_peak = 0;
        let metadata_budget = self
            .files
            .lock()
            .unwrap()
            .values()
            .try_fold(0usize, |peak, m| {
                m.bytes()
                    .checked_add(64)
                    .and_then(|n| n.checked_mul(32))
                    .map(|n| peak.max(n))
            })
            .ok_or("SWOT metadata budget estimate overflow")?;
        let mut preparation_budget = metadata_budget;
        for b in &bindings {
            let coordinate_budget = b
                .coordinates
                .iter()
                .try_fold(0usize, |n, axis| {
                    axis.len().checked_mul(256).and_then(|v| n.checked_add(v))
                })
                .ok_or("SWOT coordinate budget estimate overflow")?;
            preparation_budget = preparation_budget.max(coordinate_budget).max(
                b.jobs
                    .len()
                    .checked_mul(4096)
                    .ok_or("SWOT chunk plan budget estimate overflow")?,
            );
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
            let decoder = native
                .checked_mul(8)
                .and_then(|n| n.checked_add(encoded.checked_mul(3)?))
                .ok_or("SWOT decoder reservation overflow")?;
            if decoder > work {
                work = decoder;
                limiting = &b.path;
            }
            for axis in b.description["axes"].as_array().into_iter().flatten() {
                coordinate_peak = coordinate_peak.max(
                    axis["storage"]["prepare_reserved_bytes_peak"]
                        .as_u64()
                        .unwrap_or(0) as usize,
                );
                preparation_budget = preparation_budget.max(
                    axis["storage"]["chunks"]
                        .as_array()
                        .map_or(0, Vec::len)
                        .checked_mul(4096)
                        .ok_or("SWOT coordinate plan estimate overflow")?,
                );
            }
            chunks.push(json!({"variable":b.path,"native_chunk_bytes":native,
                "largest_selected_encoded_chunk_bytes":encoded,"decoder_reservation_bytes":decoder}));
        }
        let fixed = prepared
            .checked_add(work)
            .and_then(|n| n.checked_add(65536))
            .ok_or("SWOT reservation overflow")?;
        let base = fixed
            .checked_add(cache)
            .ok_or("SWOT reservation overflow")?;
        let largest_tile = bindings[0]
            .jobs
            .iter()
            .try_fold(1usize, |peak, j| {
                j.count[0].checked_mul(j.count[1]).map(|n| peak.max(n))
            })
            .ok_or("SWOT logical tile size overflow")?;
        let sufficient = largest_tile
            .checked_mul(128)
            .and_then(|n| fixed.checked_add(n))
            .and_then(|n| n.checked_mul(8))
            .map(|n| n.div_ceil(7))
            .and_then(|n| {
                prepared
                    .checked_mul(2)
                    .map(|p| n.max(p).max(coordinate_peak).max(preparation_budget))
            })
            .ok_or("SWOT sufficient budget estimate overflow")?;
        Ok((
            base,
            cache,
            json!({"scope":"working reservations for this prepared selection and metadata strategy; not a process RSS ceiling",
            "budget_bytes":request.budget.buffer_bytes,"prepared_total_bytes":prepared,
            "prepared_metadata_coordinates_plan_bytes":prepared - self.transport.reservation(),
            "authentication_checkpoint_reservation_bytes":self.transport.reservation(),
            "coordinate_prepare_reserved_bytes_peak":coordinate_peak,
            "metadata_preparation_sufficient_buffer_bytes":metadata_budget,
            "physical_chunks":chunks,"limiting_decoder_variable":limiting,"decoder_scratch_bytes":work,
            "encoded_range_retention_bytes":cache,"fixed_stream_overhead_bytes":65536,
            "base_reservation_bytes":base,"assembly_reservation_bytes_per_pixel":128,
            "largest_logical_tile_pixels":largest_tile,"estimated_sufficient_buffer_bytes":sufficient,
            "admits_one_pixel":request.budget.buffer_bytes.saturating_sub(base) >= 128}),
        ))
    }
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
        let (base, cache_cap, memory) = self.aligned_reservation(asset, request)?;
        let available = request
            .budget
            .buffer_bytes
            .checked_sub(base)
            .filter(|n| *n >= 128)
            .ok_or_else(|| format!(
                "SWOT aligned decoder/cache exceeds working budget: requested {} bytes; prepared {} bytes (including authentication/checkpoint), decoder {} bytes limited by {}, encoded cache {} bytes, stream overhead 65536 bytes; estimated sufficient buffer_bytes={} for this prepared selection and metadata strategy (working reservations, not process RSS)",
                request.budget.buffer_bytes, memory["prepared_total_bytes"], memory["decoder_scratch_bytes"],
                memory["limiting_decoder_variable"], cache_cap, memory["estimated_sufficient_buffer_bytes"],
            ))?;
        // 17 bytes of raw values + mask, five copies of coordinates and
        // conservative assembly slack, including narrow one-row/column tiles.
        let max_pixels = available / 128;
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
                            b.metadata.state.lock().unwrap().science_decoder_opens += 1;
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
        binding.metadata.state.lock().unwrap().science_decoder_opens += 1;
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
        metadata.state.lock().unwrap().science_decoder_opens += 1;
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
