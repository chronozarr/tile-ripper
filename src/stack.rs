//! Same-grid time stacking of a completed per-observation output (ADR 0004).
//!
//! Observations of one collection on one exact native grid, dtype, nodata and
//! calibration become `[time, band, y, x]` arrays. Nothing is resampled,
//! mosaicked or merged; ambiguous inputs refuse their stack and say why.
use crate::{output_transaction::Transaction, Result};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use zarrs::{
    array::{data_type, Array, ArrayBytes, ArrayMetadataOptions, ArraySubset, CodecOptions},
    group::Group,
    storage::{StoreKey, WritableStorageTraits},
};
use zarrs_filesystem::{FilesystemStore, FilesystemStoreOptions};
type SourceArray = Array<crate::zarr_decode::Checked<FilesystemStore>>;

/// Default spatial chunk edge. The stack is an intermediate Zarr, not a chronozarr store.
pub const DEFAULT_CHUNK_EDGE: u64 = 512;
/// Default reservation for pixel buffers, decoded source chunks and codec scratch.
/// Metadata and the process runtime are separately bounded, not an RSS guarantee.
pub const DEFAULT_BUFFER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_METADATA_FILE_BYTES: u64 = 1024 * 1024;
const MAX_OBSERVATIONS: usize = 4096;
const CODEC_RESERVATION: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub chunk_edge: u64,
    pub buffer_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            chunk_edge: DEFAULT_CHUNK_EDGE,
            buffer_bytes: DEFAULT_BUFFER_BYTES,
        }
    }
}

/// One per-observation group of the input, as the stacker needs it.
#[derive(Clone, Debug)]
pub(crate) struct Observation {
    group: String,
    asset_id: String,
    item_id: Option<String>,
    href: Option<String>,
    object_version: Option<String>,
    source: Value,
    source_record: Value,
    collection: Option<String>,
    band: String,
    datetime: String,
    time_ms: i64,
    crs: String,
    transform: [f64; 6],
    height: u64,
    width: u64,
    dtype: String,
    nodata: Option<f64>,
    scale: Option<f64>,
    offset: Option<f64>,
    units: Option<String>,
    quality: Vec<String>,
    max_source_chunk_bytes: u64,
}

/// Exact grouping identity. Floats compare by bit pattern: equal grids give
/// bit-equal transforms because windows are integer offsets of one layout.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    collection: Option<String>,
    crs: String,
    transform: [u64; 6],
    height: u64,
    width: u64,
    dtype: String,
    nodata: Option<u64>,
    scale: Option<u64>,
    offset: Option<u64>,
    units: Option<String>,
    quality: Vec<String>,
}

impl Key {
    fn of(o: &Observation) -> Self {
        Self {
            collection: o.collection.clone(),
            crs: o.crs.clone(),
            transform: o.transform.map(f64::to_bits),
            height: o.height,
            width: o.width,
            dtype: o.dtype.clone(),
            nodata: o.nodata.map(f64::to_bits),
            scale: o.scale.map(f64::to_bits),
            offset: o.offset.map(f64::to_bits),
            units: o.units.clone(),
            quality: o.quality.clone(),
        }
    }
    fn without_calibration(&self) -> Self {
        Self {
            scale: None,
            offset: None,
            ..self.clone()
        }
    }
}

#[derive(Debug)]
pub(crate) struct Stack {
    key: Key,
    bands: Vec<String>,
    /// `slots[t][b]` indexes `Plan::observations`.
    slots: Vec<Vec<usize>>,
}

#[derive(Debug)]
pub(crate) struct Plan {
    observations: Vec<Observation>,
    stacks: Vec<Stack>,
    refusals: Vec<Value>,
}

fn refusal(assets: Vec<&str>, reason: String) -> Value {
    json!({"assets":assets,"reason":reason})
}

/// Group observations into stacks; every ambiguity refuses its whole stack.
pub(crate) fn plan_observations(observations: Vec<Observation>, mut refusals: Vec<Value>) -> Plan {
    let mut groups: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    for (i, o) in observations.iter().enumerate() {
        groups.entry(Key::of(o)).or_default().push(i);
    }
    let mut stacks = Vec::new();
    'group: for (key, members) in groups {
        let mut bands: Vec<String> = Vec::new();
        for &i in &members {
            if !bands.contains(&observations[i].band) {
                bands.push(observations[i].band.clone());
            }
        }
        let mut times: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for &i in &members {
            times.entry(observations[i].time_ms).or_default().push(i);
        }
        let assets = |ids: &[usize]| {
            ids.iter()
                .map(|&i| observations[i].asset_id.as_str())
                .collect()
        };
        let mut slots = Vec::with_capacity(times.len());
        for (time, ids) in &times {
            let items: Vec<_> = ids.iter().map(|&i| &observations[i].item_id).collect();
            let mut slot = vec![None; bands.len()];
            for &i in ids {
                let b = bands
                    .iter()
                    .position(|b| *b == observations[i].band)
                    .unwrap();
                if slot[b].is_some() || items.iter().any(|item| *item != items[0]) {
                    refusals.push(refusal(assets(ids), format!(
                        "equal datetime {} (millisecond resolution) for distinct observations; chronozarr needs a strictly increasing time axis and observations are never merged",
                        observations[i].datetime)));
                    continue 'group;
                }
                slot[b] = Some(i);
            }
            if slot.iter().any(Option::is_none) {
                let missing: Vec<_> = bands
                    .iter()
                    .zip(&slot)
                    .filter(|(_, s)| s.is_none())
                    .map(|(b, _)| b.as_str())
                    .collect();
                refusals.push(refusal(assets(ids), format!(
                    "timestep {time} ms lacks band(s) {missing:?} that other timesteps of this grid have; every timestep needs the full band set")));
                continue 'group;
            }
            slots.push(slot.into_iter().map(Option::unwrap).collect());
        }
        stacks.push(Stack { key, bands, slots });
    }
    stacks.sort_by_key(|s| observations[s.slots[0][0]].time_ms);
    Plan {
        observations,
        stacks,
        refusals,
    }
}

impl Plan {
    fn first(&self, s: &Stack) -> &Observation {
        &self.observations[s.slots[0][0]]
    }
    fn last(&self, s: &Stack) -> &Observation {
        &self.observations[s.slots[s.slots.len() - 1][0]]
    }
    fn calibration_events(&self) -> (Vec<Value>, Vec<Value>) {
        // Compare adjacent observations on the combined timeline, rather than
        // comparing the extrema of calibration buckets (A-B-A interleaves).
        let mut families =
            BTreeMap::<(Key, Vec<String>), BTreeMap<i64, Vec<(usize, &Observation)>>>::new();
        for (k, stack) in self.stacks.iter().enumerate() {
            let mut bands = stack.bands.clone();
            bands.sort();
            let timeline = families
                .entry((stack.key.without_calibration(), bands))
                .or_default();
            for slot in &stack.slots {
                let o = &self.observations[slot[0]];
                timeline.entry(o.time_ms).or_default().push((k, o));
            }
        }
        let mut breaks = Vec::new();
        let mut ambiguities = Vec::new();
        for ((_, bands), timeline) in families {
            let mut previous: Option<(usize, &Observation)> = None;
            for variants in timeline.values() {
                if variants.len() != 1 {
                    ambiguities.push(json!({"datetime":variants[0].1.datetime,"bands":bands,
                        "stacks":variants.iter().map(|(k,_)|*k).collect::<Vec<_>>(),
                        "meaning":"distinct calibration variants share an instant; no chronological boundary is inferred"}));
                    previous = None;
                    continue;
                }
                let (k, o) = variants[0];
                if let Some((j, before)) = previous {
                    if before.scale.map(f64::to_bits) != o.scale.map(f64::to_bits)
                        || before.offset.map(f64::to_bits) != o.offset.map(f64::to_bits)
                    {
                        breaks.push(json!({"stacks":[j,k],"bands":bands,
                            "before":{"scale":before.scale,"offset":before.offset,"last":before.datetime},
                            "after":{"scale":o.scale,"offset":o.offset,"first":o.datetime},
                            "meaning":"adjacent observations have different calibration; stacks remain separate and may interleave"}));
                    }
                }
                previous = Some((k, o));
            }
        }
        (breaks, ambiguities)
    }
    #[cfg(test)]
    fn calibration_breaks(&self) -> Vec<Value> {
        self.calibration_events().0
    }
    pub(crate) fn summary(&self) -> Value {
        let (breaks, ambiguities) = self.calibration_events();
        json!({
            "stacks": self.stacks.iter().enumerate().map(|(k, s)| {
                let o = self.first(s);
                json!({"group":format!("stacks/{k}"),"collection":o.collection,"crs":o.crs,
                    "transform":o.transform,"shape":[s.slots.len(),s.bands.len(),o.height,o.width],
                    "dtype":o.dtype,"nodata":o.nodata,"scale":o.scale,"offset":o.offset,"units":o.units,
                    "bands":s.bands,"quality":o.quality,"first":o.datetime,"last":self.last(s).datetime})
            }).collect::<Vec<_>>(),
            "refusals": self.refusals,
            "calibration_breaks": breaks,
            "calibration_ambiguities": ambiguities,
        })
    }
}

fn input_store(input: &Path) -> Result<Arc<FilesystemStore>> {
    Ok(Arc::new(FilesystemStore::new(input).map_err(|e| {
        format!("cannot open stack input {}: {e}", input.display())
    })?))
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "{} exceeds the {limit}-byte stack metadata limit",
            path.display()
        )
        .into());
    }
    Ok(bytes)
}

fn preflight_metadata(input: &Path, node: &str, total: &mut u64) -> Result<()> {
    for name in ["zarr.json", ".zgroup", ".zarray", ".zattrs"] {
        let path = input.join(node).join(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", path.display()).into()),
        };
        if !metadata.is_file() || metadata.len() > MAX_METADATA_FILE_BYTES {
            return Err(format!(
                "{} must be a regular metadata file of at most {MAX_METADATA_FILE_BYTES} bytes",
                path.display()
            )
            .into());
        }
        *total = total
            .checked_add(metadata.len())
            .ok_or("stack metadata byte count overflow")?;
        if *total > MAX_METADATA_BYTES {
            return Err(format!(
                "stack metadata exceeds {MAX_METADATA_BYTES} bytes; narrow the input"
            )
            .into());
        }
    }
    Ok(())
}

/// Unsupported observation models are reported individually. Malformed metadata
/// in an admitted raster is an integrity error, never a silently removed band.
fn read_input(input: &Path, options: Options, cancelled: &AtomicBool) -> Result<(u64, Plan)> {
    let path = input.join("manifest.json");
    let bytes = read_bounded(&path, MAX_MANIFEST_BYTES).map_err(|e| {
        format!(
            "{} is not a readable completed tile-ripper output: {e}",
            input.display()
        )
    })?;
    let manifest: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    if manifest["document_type"] != "acquisition_result" || manifest["status"] != "complete" {
        return Err(format!(
            "{} is not a complete acquisition_result (document_type {}, status {})",
            path.display(),
            manifest["document_type"],
            manifest["status"]
        )
        .into());
    }
    let records = manifest["outputs"]
        .as_array()
        .ok_or_else(|| format!("{} has no outputs list", path.display()))?;
    if records.len() > MAX_OBSERVATIONS {
        return Err(format!(
            "stack input has {} records; at most {MAX_OBSERVATIONS} are admitted",
            records.len()
        )
        .into());
    }
    let store = input_store(input)?;
    let mut observations = Vec::new();
    let mut refusals = Vec::new();
    let mut metadata_bytes = bytes.len() as u64;
    for record in records {
        check_cancelled(cancelled)?;
        let asset = record["asset_id"].as_str().unwrap_or("?");
        let unsupported = if record.get("time_coordinate_role").is_some() {
            Some("annual-summary observations carry time bounds; this stack version does not stack them")
        } else if record["datetime"].is_null() {
            Some("undated observation; a time axis needs a datetime")
        } else if record["dimensions"]["names"].is_array()
            && record["dimensions"]["names"] != json!(["time", "y", "x"])
            || record["dimensions"]["shape"][0]
                .as_u64()
                .is_some_and(|n| n > 1)
        {
            Some("unsupported observation model; expected one [time, y, x] raster")
        } else {
            None
        };
        if let Some(reason) = unsupported {
            refusals.push(refusal(vec![asset], reason.into()));
            continue;
        }
        observations.push(
            observation(
                &store,
                input,
                record,
                options,
                &mut metadata_bytes,
                cancelled,
            )
            .map_err(|e| format!("asset {asset}: {e}"))?,
        );
    }
    Ok((
        bytes.len() as u64,
        plan_observations(observations, refusals),
    ))
}

fn dtype_bytes(dtype: &str) -> Result<u64> {
    match dtype {
        "uint8" => Ok(1),
        "uint16" | "int16" => Ok(2),
        "float32" | "int32" => Ok(4),
        "float64" | "int64" => Ok(8),
        _ => Err(format!("unsupported stack dtype {dtype}").into()),
    }
}

fn validate_array(
    array: &Array<FilesystemStore>,
    shape: &[u64],
    dtype: &str,
    dimensions: &[&str],
    options: Options,
) -> Result<u64> {
    let expected_type = match dtype {
        "uint8" => data_type::uint8(),
        "uint16" => data_type::uint16(),
        "int16" => data_type::int16(),
        "float32" => data_type::float32(),
        "float64" => data_type::float64(),
        "int32" => data_type::int32(),
        "int64" => data_type::int64(),
        _ => return Err(format!("{}: unsupported dtype {dtype}", array.path()).into()),
    };
    if array.shape() != shape || array.data_type() != &expected_type {
        return Err(format!("{}: actual shape {:?} and dtype {:?} disagree with expected shape {shape:?} and dtype {dtype}", array.path(), array.shape(), array.data_type()).into());
    }
    let dims = json!(dimensions);
    if let Some(mapping) = array.attributes().get("grid_mapping") {
        if mapping.as_str() != Some("spatial_ref") {
            return Err(format!("{}: grid_mapping must refer to spatial_ref", array.path()).into());
        }
    }
    let declared = array.attributes().get("_ARRAY_DIMENSIONS");
    if declared.is_some_and(|d| *d != dims)
        || array
            .dimension_names()
            .as_ref()
            .is_some_and(|d| json!(d) != dims)
        || (declared.is_none() && array.dimension_names().is_none())
    {
        return Err(format!(
            "{}: dimension metadata disagrees with {dimensions:?}",
            array.path()
        )
        .into());
    }
    let metadata = serde_json::to_value(array.metadata())?;
    if !metadata["chunks"].is_array() && metadata["chunk_grid"]["name"] != "regular" {
        return Err(format!(
            "{}: only regular source chunk grids are admitted",
            array.path()
        )
        .into());
    }
    let chunk_bytes = array
        .chunk_shape(&vec![0; shape.len()])?
        .iter()
        .try_fold(dtype_bytes(dtype)?, |bytes, n| bytes.checked_mul(n.get()))
        .ok_or_else(|| format!("{}: decoded source chunk size overflow", array.path()))?;
    let scratch = chunk_bytes
        .checked_mul(3)
        .and_then(|n| n.checked_add((options.buffer_bytes / 8).checked_mul(2)?))
        .and_then(|n| n.checked_add(CODEC_RESERVATION))
        .ok_or("stack source reservation overflow")?;
    if scratch > options.buffer_bytes {
        return Err(format!("{}: decoded source chunk requires {scratch} reserved bytes, exceeding buffer_bytes {}; raise --buffer-mib or use smaller source chunks", array.path(), options.buffer_bytes).into());
    }
    Ok(chunk_bytes)
}

fn number_attr(attrs: &Map<String, Value>, key: &str, group: &str) -> Result<Option<f64>> {
    attrs
        .get(key)
        .map(|v| {
            v.as_f64()
                .filter(|n| n.is_finite() && (key != "scale_factor" || *n != 0.))
                .ok_or_else(|| {
                    format!(
                        "{group}/values: {key} must be a finite number{}",
                        if key == "scale_factor" {
                            " other than zero"
                        } else {
                            ""
                        }
                    )
                    .into()
                })
        })
        .transpose()
}

fn coordinate_matches(
    array: &SourceArray,
    origin: f64,
    step: f64,
    cancelled: &AtomicBool,
) -> Result<()> {
    let length = array.shape()[0];
    for start in (0..length).step_by(DEFAULT_CHUNK_EDGE as usize) {
        check_cancelled(cancelled)?;
        let count = DEFAULT_CHUNK_EDGE.min(length - start);
        let subset = ArraySubset::new_with_start_shape(vec![start], vec![count])?;
        let values = array
            .retrieve_array_subset_opt::<Vec<f64>>(&subset, &serial())
            .map_err(|e| {
                format!(
                    "{} coordinate range {start}..{}: {e}",
                    array.path(),
                    start + count
                )
            })?;
        for (i, value) in values.iter().enumerate() {
            let j = start + i as u64;
            let expected = origin + (j as f64 + 0.5) * step;
            if value.to_bits() != expected.to_bits() {
                return Err(format!("{} coordinate {j} is {value}, inconsistent with native_grid transform ({expected})", array.path()).into());
            }
        }
    }
    Ok(())
}

fn observation(
    store: &Arc<FilesystemStore>,
    input: &Path,
    record: &Value,
    options: Options,
    metadata_bytes: &mut u64,
    cancelled: &AtomicBool,
) -> Result<Observation> {
    let group = record["group"]
        .as_str()
        .filter(|g| !g.is_empty())
        .ok_or("output record has no group")?
        .to_owned();
    if Path::new(&group)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("{group}: group must be a relative path without traversal").into());
    }
    let datetime = record["datetime"]
        .as_str()
        .ok_or("datetime must be an RFC3339 string")?
        .to_owned();
    let recorded_time = chrono::DateTime::parse_from_rfc3339(&datetime)
        .map_err(|e| format!("{group}: datetime {datetime}: {e}"))?;
    let time_ms = recorded_time.timestamp_millis();
    let shape = &record["dimensions"]["shape"];
    let (height, width) = match (shape[0].as_u64(), shape[1].as_u64(), shape[2].as_u64()) {
        (Some(1), Some(h), Some(w))
            if h > 0
                && w > 0
                && shape.as_array().is_some_and(|s| s.len() == 3)
                && record["dimensions"]["names"] == json!(["time", "y", "x"]) =>
        {
            (h, w)
        }
        _ => {
            return Err(format!(
            "{group}: expected one positive [time, y, x] raster observation, found dimensions {}",
            record["dimensions"]
        )
            .into())
        }
    };
    let grid = &record["native_grid"];
    let crs = grid["crs"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{group}: no native CRS"))?
        .to_owned();
    let t: Vec<f64> = serde_json::from_value(grid["transform"].clone())
        .map_err(|e| format!("{group}: native transform: {e}"))?;
    let transform: [f64; 6] = t
        .try_into()
        .map_err(|_| format!("{group}: native transform needs six terms"))?;
    if transform.iter().any(|n| !n.is_finite())
        || transform[1] <= 0.
        || transform[5] >= 0.
        || transform[2] != 0.
        || transform[4] != 0.
    {
        return Err(format!("{group}: native transform must be finite and north-up").into());
    }
    preflight_metadata(input, &group, metadata_bytes)?;
    let attrs = Group::open(store.clone(), &format!("/{group}"))
        .map_err(|e| format!("{group}: group metadata: {e}"))?
        .attributes()
        .clone();
    let source = attrs
        .get("source")
        .filter(|s| s.is_object())
        .ok_or_else(|| format!("{group}: source metadata is missing or is not an object"))?
        .clone();
    let asset_id = record["asset_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("output record has no asset_id")?
        .to_owned();
    for (holder, fields) in [("group", &attrs), ("source", source.as_object().unwrap())] {
        if let Some(value) = fields.get("asset_id") {
            if value.as_str() != Some(&asset_id) {
                return Err(format!(
                    "{group}: {holder} asset_id disagrees with output record asset_id {asset_id}"
                )
                .into());
            }
        }
        if let Some(value) = fields.get("datetime") {
            let text = value
                .as_str()
                .ok_or_else(|| format!("{group}: {holder} datetime must be RFC3339 text"))?;
            let actual = chrono::DateTime::parse_from_rfc3339(text)
                .map_err(|e| format!("{group}: {holder} datetime: {e}"))?;
            if actual != recorded_time {
                return Err(format!("{group}: {holder} datetime {text} disagrees with output record datetime {datetime}").into());
            }
        }
    }
    let required = |key: &str| -> Result<String> {
        source[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{group}: source has no {key} identity").into())
    };
    let band = required("requested_band")?;
    let item_id = Some(required("item_id")?);
    let collection = Some(required("collection")?);
    let version = source["object_version"].as_str();
    let etag = source["object_identity"]["etag"].as_str();
    if version.zip(etag).is_some_and(|(a, b)| a != b) {
        return Err(
            format!("{group}: source object_version conflicts with object_identity.etag").into(),
        );
    }
    let object_version = version
        .or(etag)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let href = source["href"].as_str().map(str::to_owned);
    if href
        .as_ref()
        .is_some_and(|h| h.starts_with("http://") || h.starts_with("https://"))
        && object_version.is_none()
    {
        return Err(format!(
            "{group}: remote source has no object_version or object_identity.etag pin"
        )
        .into());
    }
    if let Some(value) = attrs.get("crs") {
        if value.as_str() != Some(&crs) {
            return Err(format!("{group}: group CRS disagrees with native_grid.crs").into());
        }
    }
    if let Some(value) = attrs.get("transform") {
        let found: [f64; 6] = serde_json::from_value(value.clone())
            .map_err(|e| format!("{group}: group transform: {e}"))?;
        if found.map(f64::to_bits) != transform.map(f64::to_bits) {
            return Err(
                format!("{group}: group transform disagrees with native_grid.transform").into(),
            );
        }
    }
    let dtype = record["dtype"]
        .as_str()
        .ok_or_else(|| format!("{group}: output record has no dtype"))?
        .to_owned();
    if !["uint8", "uint16", "int16", "float32"].contains(&dtype.as_str()) {
        return Err(format!("{group}: unsupported COG observation dtype {dtype}").into());
    }
    let mut quality = Vec::new();
    if let Some(masks) = record.get("derived_masks") {
        for mask in masks
            .as_array()
            .ok_or_else(|| format!("{group}: derived_masks must be an array"))?
        {
            let name = mask["dataset"]
                .as_str()
                .filter(|s| s.starts_with("quality_") && !s.contains('/'))
                .ok_or_else(|| format!("{group}: derived mask has no valid quality_ dataset name"))?
                .to_owned();
            if quality.contains(&name) {
                return Err(format!("{group}: duplicate derived mask {name}").into());
            }
            quality.push(name);
        }
    }
    quality.sort();
    let mut max_source_chunk_bytes = 0;
    let mut open = |name: &str, shape: &[u64], dtype: &str, dims: &[&str]| -> Result<SourceArray> {
        let relative = format!("{group}/{name}");
        preflight_metadata(input, &relative, metadata_bytes)?;
        let array = Array::open(store.clone(), &format!("/{relative}"))
            .map_err(|e| format!("{relative}: {e}"))?;
        max_source_chunk_bytes =
            max_source_chunk_bytes.max(validate_array(&array, shape, dtype, dims, options)?);
        crate::zarr_decode::local_observation(
            store.clone(),
            &relative,
            serde_json::to_value(array.metadata())?,
            json!(array.attributes()),
            usize::try_from(options.buffer_bytes)
                .map_err(|e| format!("buffer_bytes exceeds platform address space: {e}"))?,
        )
        .map_err(|e| format!("{relative}: bounded local source: {e}").into())
    };
    let values = open("values", &[1, height, width], &dtype, &["time", "y", "x"])?;
    let nodata = record
        .get("nodata")
        .filter(|n| !n.is_null())
        .map(|n| {
            n.as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| format!("{group}: nodata must be a finite number or null"))
        })
        .transpose()?;
    if let Some(nodata) = nodata {
        let metadata = serde_json::to_value(values.metadata())?;
        if metadata["fill_value"].as_f64() != Some(nodata) {
            return Err(format!(
                "{group}/values: fill_value disagrees with output record nodata {nodata}"
            )
            .into());
        }
    }
    for name in ["valid", "aoi_mask"]
        .into_iter()
        .chain(quality.iter().map(String::as_str))
    {
        open(name, &[1, height, width], "uint8", &["time", "y", "x"])?;
    }
    for (axis, len, origin, step) in [
        ("x", width, transform[0], transform[1]),
        ("y", height, transform[3], transform[5]),
    ] {
        let array = open(axis, &[len], "float64", &[axis])?;
        coordinate_matches(&array, origin, step, cancelled)?;
    }
    let spatial = open("spatial_ref", &[], "int32", &[])?;
    let spatial_attrs = spatial.attributes();
    if !["crs", "crs_wkt", "spatial_ref"]
        .iter()
        .any(|key| spatial_attrs.get(*key).and_then(Value::as_str) == Some(&crs))
    {
        return Err(format!("{group}/spatial_ref: CRS disagrees with native_grid.crs").into());
    }
    if let Some(value) = spatial_attrs.get("GeoTransform") {
        let terms = value
            .as_str()
            .ok_or_else(|| format!("{group}/spatial_ref: GeoTransform must be text"))?
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| format!("{group}/spatial_ref: GeoTransform: {e}"))?;
        if terms.len() != 6
            || !terms
                .iter()
                .zip(transform)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        {
            return Err(format!(
                "{group}/spatial_ref: GeoTransform disagrees with native_grid.transform"
            )
            .into());
        }
    }
    // Earlier synthetic/legacy observations may omit time; when present it
    // must agree with the label used to sort and group the pixels.
    let time_path = input.join(&group).join("time");
    let has_time = match fs::symlink_metadata(&time_path) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(format!("{}: {e}", time_path.display()).into()),
    };
    if has_time {
        let time = open("time", &[1], "int64", &["time"])?;
        let expected = match time.attributes().get("units").and_then(Value::as_str) {
            Some("nanoseconds since 1970-01-01 00:00:00") => recorded_time
                .timestamp_nanos_opt()
                .ok_or("time is outside nanosecond range")?,
            Some("milliseconds since 1970-01-01 00:00:00") => time_ms,
            other => return Err(format!("{group}/time: unsupported time units {other:?}").into()),
        };
        let stored = time
            .retrieve_array_subset_opt::<Vec<i64>>(&time.subset_all(), &serial())
            .map_err(|e| format!("{group}/time: {e}"))?;
        if stored != [expected] {
            return Err(format!(
                "{group}/time: stored time disagrees with output record datetime {datetime}"
            )
            .into());
        }
    }
    let v = values.attributes();
    let scale = number_attr(v, "scale_factor", &group)?;
    let offset = number_attr(v, "add_offset", &group)?;
    let units = v
        .get("units")
        .map(|u| {
            u.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{group}/values: units must be text"))
        })
        .transpose()?;
    Ok(Observation {
        group,
        asset_id,
        item_id,
        href,
        object_version,
        source,
        source_record: record.clone(),
        collection,
        band,
        datetime,
        time_ms,
        crs,
        transform,
        height,
        width,
        dtype,
        nodata,
        scale,
        offset,
        units,
        quality,
        max_source_chunk_bytes,
    })
}

/// Read metadata and coordinates to check stack keys, refusals and calibration
/// transitions. Pixel arrays and the time-invariant AOI check run only in [`write`].
pub fn plan(input: &Path) -> Result<Value> {
    let options = Options::default();
    let (manifest_bytes, plan) = read_input(input, options, &AtomicBool::new(false))?;
    let peak = admit_stacks(&plan, options)?;
    let mut summary = plan.summary();
    summary["input"] = json!({"path":input,"manifest_bytes":manifest_bytes});
    summary["aoi_check"] = json!("not run; write checks aoi_mask identity per stack");
    summary["working_memory"] = memory_summary(options, peak);
    Ok(summary)
}

fn serial() -> CodecOptions {
    let mut options = CodecOptions::default();
    options.set_concurrent_target(1);
    options.set_store_empty_chunks(true);
    options
}

fn admit_stacks(plan: &Plan, options: Options) -> Result<u64> {
    if options.chunk_edge == 0 || options.buffer_bytes == 0 {
        return Err("stack chunk_edge and buffer_bytes must be positive".into());
    }
    let mut peak = 0;
    for stack in &plan.stacks {
        let o = plan.first(stack);
        let area = options
            .chunk_edge
            .min(o.height)
            .checked_mul(options.chunk_edge.min(o.width))
            .ok_or("stack tile area overflow")?;
        let band_bytes = dtype_bytes(&o.dtype)?
            .checked_add(2)
            .and_then(|n| n.checked_mul(stack.bands.len() as u64))
            .and_then(|n| n.checked_add(2))
            .ok_or("stack band reservation overflow")?;
        let source_chunk = stack
            .slots
            .iter()
            .flatten()
            .map(|&i| plan.observations[i].max_source_chunk_bytes)
            .max()
            .unwrap_or(0);
        let reservation = area
            .checked_mul(band_bytes)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_add(source_chunk.checked_mul(3)?))
            .and_then(|n| n.checked_add((options.buffer_bytes / 8).checked_mul(2)?))
            .and_then(|n| n.checked_add(CODEC_RESERVATION))
            .ok_or("stack working reservation overflow")?;
        if reservation > options.buffer_bytes {
            return Err(format!("stack {} on {} requires {reservation} reserved bytes, exceeding buffer_bytes {}; reduce --chunk-edge or raise --buffer-mib", o.group, o.crs, options.buffer_bytes).into());
        }
        usize::try_from(reservation)
            .map_err(|e| format!("stack reservation exceeds platform address space: {e}"))?;
        peak = peak.max(reservation);
    }
    Ok(peak)
}

fn memory_summary(options: Options, peak: u64) -> Value {
    json!({"buffer_bytes":options.buffer_bytes,"working_reservation_bytes":peak,
        "encoded_source_object_limit_bytes":options.buffer_bytes/8,
        "metadata_limit_bytes":MAX_METADATA_BYTES,"manifest_limit_bytes":MAX_MANIFEST_BYTES,
        "scope":"serial copying, decoded source chunk, tile buffers and codec allowance; metadata separately bounded; not process RSS"})
}

fn numeric_codecs() -> Value {
    json!([{"name":"bytes","configuration":{"endian":"little"}},{"name":"zstd","configuration":{"level":5,"checksum":false}}])
}

#[allow(clippy::too_many_arguments)]
fn create(
    store: &Arc<FilesystemStore>,
    path: &str,
    shape: &[u64],
    chunks: &[u64],
    data_type: &str,
    fill: Value,
    dimensions: &[&str],
    mut attrs: Value,
) -> Result<Array<FilesystemStore>> {
    attrs["_ARRAY_DIMENSIONS"] = json!(dimensions);
    let codecs = if data_type == "string" {
        // An explicit empty configuration: zarr-python rejects the bare-name codec form.
        json!([{"name":"vlen-utf8","configuration":{}},{"name":"zstd","configuration":{"level":5,"checksum":false}}])
    } else {
        numeric_codecs()
    };
    let metadata = json!({"zarr_format":3,"node_type":"array","shape":shape,"data_type":data_type,
        "chunk_grid":{"name":"regular","configuration":{"chunk_shape":chunks}},
        "chunk_key_encoding":{"name":"default","configuration":{"separator":"/"}},
        "fill_value":fill,"codecs":codecs,"dimension_names":dimensions,"attributes":attrs});
    let array = Array::new_with_metadata(store.clone(), path, serde_json::from_value(metadata)?)?;
    array
        .store_metadata_opt(&ArrayMetadataOptions::default().with_include_zarrs_metadata(false))?;
    Ok(array)
}

fn fill(dtype: &str, nodata: Option<f64>) -> Value {
    match (dtype, nodata) {
        ("float32" | "float64", Some(n)) if n.is_finite() => json!(n),
        ("float32" | "float64", _) => json!(0.0),
        (_, Some(n)) => json!(n as i64),
        (_, None) => json!(0),
    }
}

fn source_array(input: &Arc<FilesystemStore>, path: &str, options: Options) -> Result<SourceArray> {
    let array = Array::open(input.clone(), path).map_err(|e| format!("{path}: {e}"))?;
    crate::zarr_decode::local_observation(
        input.clone(),
        path,
        serde_json::to_value(array.metadata())?,
        json!(array.attributes()),
        usize::try_from(options.buffer_bytes)
            .map_err(|e| format!("buffer_bytes exceeds platform address space: {e}"))?,
    )
    .map_err(|e| format!("{path}: bounded local source: {e}").into())
}

fn bytes_of(array: &SourceArray, subset: &ArraySubset) -> Result<Vec<u8>> {
    Ok(array
        .retrieve_array_subset_opt::<ArrayBytes<'static>>(subset, &serial())
        .map_err(|e| format!("{} subset {subset:?}: {e}", array.path()))?
        .into_fixed()?
        .to_vec())
}

fn mask_bytes(array: &SourceArray, subset: &ArraySubset) -> Result<Vec<u8>> {
    let bytes = bytes_of(array, subset)?;
    if bytes.iter().any(|b| *b > 1) {
        return Err(format!(
            "{} subset {subset:?}: mask must contain only 0 or 1",
            array.path()
        )
        .into());
    }
    Ok(bytes)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        return Err("stack cancelled; nothing was published".into());
    }
    Ok(())
}

fn tiles(height: u64, width: u64, edge: u64) -> impl Iterator<Item = (u64, u64, u64, u64)> {
    (0..height.div_ceil(edge)).flat_map(move |r| {
        (0..width.div_ceil(edge)).map(move |c| {
            let (y, x) = (r * edge, c * edge);
            (y, x, edge.min(height - y), edge.min(width - x))
        })
    })
}

fn plane(y: u64, x: u64, h: u64, w: u64) -> Result<ArraySubset> {
    Ok(ArraySubset::new_with_start_shape(
        vec![0, y, x],
        vec![1, h, w],
    )?)
}

/// The AOI mask is a property of the request on one grid; differing masks mean
/// the observations are not one series.
fn aoi_identical(
    store: &Arc<FilesystemStore>,
    plan: &Plan,
    s: &Stack,
    options: Options,
    cancelled: &AtomicBool,
) -> Result<Option<String>> {
    let edge = options.chunk_edge;
    let first = plan.first(s);
    let reference = source_array(store, &format!("/{}/aoi_mask", first.group), options)?;
    for &i in s.slots.iter().flatten().skip(1) {
        check_cancelled(cancelled)?;
        let array = source_array(
            store,
            &format!("/{}/aoi_mask", plan.observations[i].group),
            options,
        )?;
        for (y, x, h, w) in tiles(first.height, first.width, edge) {
            check_cancelled(cancelled)?;
            let subset = plane(y, x, h, w)?;
            let want = mask_bytes(&reference, &subset)?;
            if mask_bytes(&array, &subset)? != want {
                return Ok(Some(format!(
                    "aoi_mask of {} differs from {} on the same grid; the AOI must be time-invariant within a stack",
                    plan.observations[i].group, first.group
                )));
            }
        }
    }
    Ok(None)
}

/// Rewrite a group's zarr.json with inline consolidated metadata for its
/// children, so HTTP readers that cannot list can open the stack group.
fn consolidate(stage: &Path, output: &Arc<FilesystemStore>, group: &str) -> Result<()> {
    let dir = stage.join(group);
    let mut children = Map::new();
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: directory entry: {e}", dir.display()))?;
        if !entry
            .file_type()
            .map_err(|e| format!("{}: {e}", entry.path().display()))?
            .is_dir()
        {
            continue;
        }
        let path = entry.path().join("zarr.json");
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                names.push(entry.file_name().into_string().map_err(|_| {
                    format!(
                        "{}: metadata child name is not UTF-8",
                        entry.path().display()
                    )
                })?)
            }
            Ok(_) => return Err(format!("{} is not a metadata file", path.display()).into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", path.display()).into()),
        }
    }
    names.sort();
    for name in names {
        let path = dir.join(&name).join("zarr.json");
        let child: Value = serde_json::from_slice(&read_bounded(&path, MAX_METADATA_BYTES)?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        children.insert(name, child);
    }
    let path = dir.join("zarr.json");
    let mut metadata: Value = serde_json::from_slice(&read_bounded(&path, MAX_METADATA_BYTES)?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    metadata["consolidated_metadata"] =
        json!({"kind":"inline","must_understand":false,"metadata":children});
    // Use the transaction's synchronized writer handle, just like array/group
    // metadata, so delayed write errors surface before that handle closes.
    output
        .set(
            &StoreKey::new(format!("{group}/zarr.json"))?,
            serde_json::to_vec_pretty(&metadata)?.into(),
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(())
}

fn copy_coordinate(
    input: &Arc<FilesystemStore>,
    output: &Arc<FilesystemStore>,
    from: &str,
    to: &str,
    dimension: &str,
    options: Options,
    cancelled: &AtomicBool,
) -> Result<()> {
    let source = source_array(input, from, options)?;
    let shape = source.shape().to_vec();
    let mut attrs = source.attributes().clone();
    let data_type = if shape.is_empty() { "int32" } else { "float64" };
    let dimensions: Vec<&str> = if shape.is_empty() {
        vec![]
    } else {
        vec![dimension]
    };
    attrs.remove("_ARRAY_DIMENSIONS");
    let chunks: Vec<u64> = shape.iter().map(|n| DEFAULT_CHUNK_EDGE.min(*n)).collect();
    let array = create(
        output,
        to,
        &shape,
        &chunks,
        data_type,
        json!(0),
        &dimensions,
        Value::Object(attrs),
    )?;
    if shape.is_empty() {
        check_cancelled(cancelled)?;
        let bytes = bytes_of(&source, &source.subset_all())?;
        array.store_array_subset_opt(
            &array.subset_all(),
            ArrayBytes::new_flen(bytes),
            &serial(),
        )?;
    } else {
        for start in (0..shape[0]).step_by(DEFAULT_CHUNK_EDGE as usize) {
            check_cancelled(cancelled)?;
            let count = DEFAULT_CHUNK_EDGE.min(shape[0] - start);
            let subset = ArraySubset::new_with_start_shape(vec![start], vec![count])?;
            let bytes = bytes_of(&source, &subset)?;
            array.store_array_subset_opt(&subset, ArrayBytes::new_flen(bytes), &serial())?;
        }
    }
    Ok(())
}

fn write_stack(
    input: &Arc<FilesystemStore>,
    output: &Arc<FilesystemStore>,
    plan: &Plan,
    s: &Stack,
    base: &str,
    options: Options,
    cancelled: &AtomicBool,
) -> Result<()> {
    let edge = options.chunk_edge;
    let first = plan.first(s);
    let (n_time, n_band, height, width) = (
        s.slots.len() as u64,
        s.bands.len() as u64,
        first.height,
        first.width,
    );
    let observations: Vec<Value> = s
        .slots
        .iter()
        .enumerate()
        .flat_map(|(t, slot)| {
            slot.iter().map(move |&i| {
                let o = &plan.observations[i];
                json!({"t":t,"band":o.band,"asset_id":o.asset_id,"item_id":o.item_id,"href":o.href,
                "object_version":o.object_version,"source":o.source,"source_record":o.source_record,
                "datetime":o.datetime,"source_group":o.group})
            })
        })
        .collect();
    let mut group = Group::new_with_metadata(
        output.clone(),
        base,
        serde_json::from_value(json!({"zarr_format":3,"node_type":"group"}))?,
    )?;
    *group.attributes_mut() = serde_json::from_value(json!({
        "bands":s.bands,"collection":first.collection,"observations":observations,
        "layout":"one observation per (time, band) slot; exact native grid; no resampling, mosaic or composite"}))?;
    group.store_metadata()?;
    copy_coordinate(
        input,
        output,
        &format!("/{}/x", first.group),
        &format!("{base}/x"),
        "x",
        options,
        cancelled,
    )?;
    copy_coordinate(
        input,
        output,
        &format!("/{}/y", first.group),
        &format!("{base}/y"),
        "y",
        options,
        cancelled,
    )?;
    copy_coordinate(
        input,
        output,
        &format!("/{}/spatial_ref", first.group),
        &format!("{base}/spatial_ref"),
        "",
        options,
        cancelled,
    )?;
    let time = create(
        output,
        &format!("{base}/time"),
        &[n_time],
        &[n_time],
        "int64",
        json!(0),
        &["time"],
        json!({"standard_name":"time","units":"milliseconds since 1970-01-01 00:00:00","calendar":"proleptic_gregorian"}),
    )?;
    let times: Vec<i64> = s
        .slots
        .iter()
        .map(|slot| plan.observations[slot[0]].time_ms)
        .collect();
    time.store_array_subset_opt(&time.subset_all(), &times, &serial())?;
    let band = create(
        output,
        &format!("{base}/band"),
        &[n_band],
        &[n_band],
        "string",
        json!(""),
        &["band"],
        json!({}),
    )?;
    band.store_array_subset_opt(&band.subset_all(), &s.bands, &serial())?;
    let ancillary = ["valid", "valid_all", "aoi_mask"]
        .iter()
        .map(|s| (*s).to_owned())
        .chain(first.quality.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let mut value_attrs = json!({"grid_mapping":"spatial_ref","ancillary_variables":ancillary,
        "long_name":"native stored values","source_nodata":first.nodata});
    for (key, value) in [
        ("scale_factor", first.scale.map(Value::from)),
        ("add_offset", first.offset.map(Value::from)),
        ("units", first.units.clone().map(Value::from)),
    ] {
        if let Some(value) = value {
            value_attrs[key] = value;
        }
    }
    let cube = [n_time, n_band, height, width];
    let cube_chunks = [1, n_band, edge.min(height), edge.min(width)];
    let flag = |name: &str| json!({"grid_mapping":"spatial_ref","long_name":name,"flag_values":[0,1],"flag_meanings":"excluded included"});
    let values = create(
        output,
        &format!("{base}/values"),
        &cube,
        &cube_chunks,
        &first.dtype,
        fill(&first.dtype, first.nodata),
        &["time", "band", "y", "x"],
        value_attrs,
    )?;
    let valid = create(
        output,
        &format!("{base}/valid"),
        &cube,
        &cube_chunks,
        "uint8",
        json!(0),
        &["time", "band", "y", "x"],
        flag("per band, verbatim from each observation's valid"),
    )?;
    let valid_all = create(
        output,
        &format!("{base}/valid_all"),
        &[n_time, height, width],
        &[1, edge.min(height), edge.min(width)],
        "uint8",
        json!(0),
        &["time", "y", "x"],
        flag("valid in every band: AND of valid over band"),
    )?;
    let aoi = create(
        output,
        &format!("{base}/aoi_mask"),
        &[height, width],
        &[edge.min(height), edge.min(width)],
        "uint8",
        json!(0),
        &["y", "x"],
        flag("AOI pixel centre inclusion; identical for every observation of this stack"),
    )?;
    let quality = first.quality.iter().map(|name| {
        create(output, &format!("{base}/{name}"), &cube, &cube_chunks, "uint8", json!(0), &["time", "band", "y", "x"],
            json!({"grid_mapping":"spatial_ref","flag_values":[0,1],"flag_meanings":"rejected accepted",
                "long_name":format!("{name} per band, verbatim from each observation")}))
    }).collect::<Result<Vec<_>>>()?;
    let reference_aoi = source_array(input, &format!("/{}/aoi_mask", first.group), options)?;
    for (y, x, h, w) in tiles(height, width, edge) {
        check_cancelled(cancelled)?;
        let subset = plane(y, x, h, w)?;
        aoi.store_array_subset_opt(
            &ArraySubset::new_with_start_shape(vec![y, x], vec![h, w])?,
            ArrayBytes::new_flen(mask_bytes(&reference_aoi, &subset)?),
            &serial(),
        )?;
    }
    for (t, slot) in s.slots.iter().enumerate() {
        let open = |name: &str| {
            slot.iter()
                .map(|&i| {
                    source_array(
                        input,
                        &format!("/{}/{name}", plan.observations[i].group),
                        options,
                    )
                })
                .collect::<std::result::Result<Vec<_>, _>>()
        };
        let (source_values, source_valid) = (open("values")?, open("valid")?);
        let source_quality = first
            .quality
            .iter()
            .map(|name| open(name))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let t = t as u64;
        for (y, x, h, w) in tiles(height, width, edge) {
            check_cancelled(cancelled)?;
            let subset = plane(y, x, h, w)?;
            let cube_subset =
                ArraySubset::new_with_start_shape(vec![t, 0, y, x], vec![1, n_band, h, w])?;
            let mut buffer = Vec::new();
            for array in &source_values {
                buffer.extend(bytes_of(array, &subset)?);
            }
            values.store_array_subset_opt(&cube_subset, ArrayBytes::new_flen(buffer), &serial())?;
            let mut masks = Vec::new();
            let mut all = vec![1u8; (h * w) as usize];
            for array in &source_valid {
                let mask = mask_bytes(array, &subset)?;
                for (a, m) in all.iter_mut().zip(&mask) {
                    *a &= u8::from(*m != 0);
                }
                masks.extend(mask);
            }
            valid.store_array_subset_opt(&cube_subset, ArrayBytes::new_flen(masks), &serial())?;
            valid_all.store_array_subset_opt(
                &ArraySubset::new_with_start_shape(vec![t, y, x], vec![1, h, w])?,
                ArrayBytes::new_flen(all),
                &serial(),
            )?;
            for (output, sources) in quality.iter().zip(&source_quality) {
                let mut buffer = Vec::new();
                for array in sources {
                    buffer.extend(mask_bytes(array, &subset)?);
                }
                output.store_array_subset_opt(
                    &cube_subset,
                    ArrayBytes::new_flen(buffer),
                    &serial(),
                )?;
            }
        }
    }
    Ok(())
}

/// Stack a completed per-observation output into a new store. The input is
/// read only; the output is published by atomic no-replace rename.
pub fn write(
    input: &Path,
    output: &Path,
    chunk_edge: u64,
    cancelled: Arc<AtomicBool>,
) -> Result<Value> {
    write_with_options(
        input,
        output,
        Options {
            chunk_edge,
            ..Options::default()
        },
        cancelled,
    )
}

fn reject_nested_output(input: &Path, output: &Path) -> Result<()> {
    let input =
        fs::canonicalize(input).map_err(|e| format!("stack input {}: {e}", input.display()))?;
    let absolute = if output.is_absolute() {
        output.to_owned()
    } else {
        std::env::current_dir()?.join(output)
    };
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| format!("cannot resolve stack output {}", output.display()))?
                        .to_owned(),
                );
                ancestor = ancestor
                    .parent()
                    .ok_or("stack output has no existing ancestor")?;
            }
            Err(e) => return Err(format!("stack output {}: {e}", ancestor.display()).into()),
        }
    }
    let mut resolved = fs::canonicalize(ancestor)
        .map_err(|e| format!("stack output ancestor {}: {e}", ancestor.display()))?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    if resolved.starts_with(&input) {
        return Err(format!(
            "stack output {} must be outside input {}; input is read only",
            output.display(),
            input.display()
        )
        .into());
    }
    Ok(())
}

/// Like [`write`], with an explicit working-buffer reservation. It does not
/// bound process RSS; metadata, runtime and filesystem caches have separate costs.
pub fn write_with_options(
    input: &Path,
    output: &Path,
    options: Options,
    cancelled: Arc<AtomicBool>,
) -> Result<Value> {
    check_cancelled(&cancelled)?;
    if options.chunk_edge == 0 || options.buffer_bytes == 0 {
        return Err("stack chunk_edge and buffer_bytes must be positive".into());
    }
    reject_nested_output(input, output)?;
    let (manifest_bytes, mut plan) = read_input(input, options, &cancelled)?;
    let peak = admit_stacks(&plan, options)?;
    let source = input_store(input)?;
    let mut kept = Vec::new();
    for s in std::mem::take(&mut plan.stacks) {
        check_cancelled(&cancelled)?;
        match aoi_identical(&source, &plan, &s, options, &cancelled)? {
            None => kept.push(s),
            Some(reason) => {
                let assets = s
                    .slots
                    .iter()
                    .flatten()
                    .map(|&i| plan.observations[i].asset_id.as_str())
                    .collect();
                plan.refusals.push(refusal(assets, reason));
            }
        }
    }
    plan.stacks = kept;
    if plan.stacks.is_empty() {
        return Err(format!(
            "no stackable observations in {}: {}",
            input.display(),
            json!(plan.refusals)
        )
        .into());
    }
    let transaction = Transaction::new(output)?;
    let mut filesystem_options = FilesystemStoreOptions::default();
    filesystem_options.defer_device_flush(true);
    let target = Arc::new(FilesystemStore::new_with_options(
        transaction.stage(),
        filesystem_options,
    )?);
    let mut summary = plan.summary();
    summary["input"] = json!({"path":input,"manifest_bytes":manifest_bytes});
    summary["working_memory"] = memory_summary(options, peak);
    let mut root = Group::new_with_metadata(
        target.clone(),
        "/",
        serde_json::from_value(json!({"zarr_format":3,"node_type":"group"}))?,
    )?;
    *root.attributes_mut() = serde_json::from_value(json!({"title":"Same-grid time stacks",
        "status":"incomplete","stack_version":1,"summary":summary,
        "processing":"native stored values copied byte for byte; observations on one exact grid, dtype, nodata and calibration share a time axis; no resampling, mosaic or composite",
        "software":{"name":"tile-ripper","version":env!("CARGO_PKG_VERSION")}}))?;
    root.store_metadata()?;
    Group::new_with_metadata(
        target.clone(),
        "/stacks",
        serde_json::from_value(json!({"zarr_format":3,"node_type":"group"}))?,
    )?
    .store_metadata()?;
    for (k, s) in plan.stacks.iter().enumerate() {
        let base = format!("/stacks/{k}");
        write_stack(&source, &target, &plan, s, &base, options, &cancelled)
            .map_err(|e| format!("stack {base}: {e}"))?;
        consolidate(transaction.stage(), &target, &format!("stacks/{k}"))?;
    }
    check_cancelled(&cancelled)?;
    root.attributes_mut()
        .insert("status".into(), json!("complete"));
    root.store_metadata()?;
    let manifest = json!({"schema_version":1,"document_type":"stack_result","status":"complete",
        "format":"zarr_v3","chunk_edge":options.chunk_edge,"summary":summary,
        "completion":{"publication":"atomic no-replace sibling rename","input":"read only; never modified"}});
    target.set(
        &StoreKey::new("manifest.json")?,
        serde_json::to_vec_pretty(&manifest)?.into(),
    )?;
    drop(target);
    transaction.publish(&cancelled)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, sync::atomic::AtomicU64};

    fn root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tile-ripper-stack-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn obs(asset: &str, item: &str, band: &str, datetime: &str) -> Observation {
        Observation {
            group: format!("observations/{asset}"),
            asset_id: asset.into(),
            item_id: Some(item.into()),
            href: None,
            object_version: None,
            source: json!({}),
            source_record: json!({}),
            collection: Some("sentinel-2-c1-l2a".into()),
            band: band.into(),
            datetime: datetime.into(),
            time_ms: chrono::DateTime::parse_from_rfc3339(datetime)
                .unwrap()
                .timestamp_millis(),
            crs: "EPSG:32718".into(),
            transform: [500000., 10., 0., 9100000., 0., -10.],
            height: 4,
            width: 3,
            dtype: "uint16".into(),
            nodata: Some(0.),
            scale: Some(1e-4),
            offset: Some(-0.1),
            units: None,
            quality: vec![],
            max_source_chunk_bytes: 24,
        }
    }

    fn series() -> Vec<Observation> {
        let mut out = Vec::new();
        for (d, day) in ["2024-01-03", "2024-01-08", "2024-01-13"]
            .iter()
            .enumerate()
        {
            for band in ["B04", "B08"] {
                out.push(obs(
                    &format!("{d}{band}"),
                    &format!("item{d}"),
                    band,
                    &format!("{day}T15:10:00Z"),
                ));
            }
        }
        out
    }

    #[test]
    fn one_grid_becomes_one_stack_in_time_order() {
        let mut input = series();
        input.reverse();
        let plan = plan_observations(input, vec![]);
        assert!(plan.refusals.is_empty());
        assert_eq!(plan.stacks.len(), 1);
        let s = &plan.stacks[0];
        assert_eq!(s.bands, ["B08", "B04"]);
        let times: Vec<_> = s
            .slots
            .iter()
            .map(|slot| &plan.observations[slot[0]].datetime)
            .collect();
        assert_eq!(
            times,
            [
                "2024-01-03T15:10:00Z",
                "2024-01-08T15:10:00Z",
                "2024-01-13T15:10:00Z"
            ]
        );
        for slot in &s.slots {
            assert_eq!(plan.observations[slot[0]].band, "B08");
            assert_eq!(plan.observations[slot[1]].band, "B04");
        }
    }

    #[test]
    fn equal_datetime_refuses_the_stack() {
        let mut input = series();
        // A second item at the same millisecond (sub-millisecond difference only).
        input.push(obs("dup", "other-item", "B04", "2024-01-08T15:10:00.0004Z"));
        let plan = plan_observations(input, vec![]);
        assert!(plan.stacks.is_empty());
        assert_eq!(plan.refusals.len(), 1);
        assert!(plan.refusals[0]["reason"]
            .as_str()
            .unwrap()
            .contains("equal datetime"));
    }

    #[test]
    fn distinct_items_are_never_merged_into_one_timestep() {
        // B04 from one item and B08 from another at the same instant would
        // otherwise fill one slot each and pass as a single observation.
        let mut input = series();
        for o in input.iter_mut().filter(|o| o.asset_id == "1B08") {
            o.item_id = Some("other-item".into());
        }
        let plan = plan_observations(input, vec![]);
        assert!(plan.stacks.is_empty());
        assert!(plan.refusals[0]["reason"]
            .as_str()
            .unwrap()
            .contains("equal datetime"));
    }

    #[test]
    fn same_item_bands_at_one_time_are_not_a_tie() {
        let plan = plan_observations(series(), vec![]);
        assert_eq!(plan.stacks[0].slots.len(), 3);
    }

    #[test]
    fn missing_band_refuses_the_stack() {
        let mut input = series();
        input.retain(|o| o.asset_id != "1B08");
        let plan = plan_observations(input, vec![]);
        assert!(plan.stacks.is_empty());
        let reason = plan.refusals[0]["reason"].as_str().unwrap();
        assert!(reason.contains("B08"), "{reason}");
    }

    #[test]
    fn offset_change_splits_and_reports_a_calibration_break() {
        let mut input = series();
        for o in input
            .iter_mut()
            .filter(|o| o.datetime.starts_with("2024-01-13"))
        {
            o.offset = Some(0.);
        }
        let plan = plan_observations(input, vec![]);
        assert!(plan.refusals.is_empty());
        assert_eq!(plan.stacks.len(), 2);
        assert_eq!(plan.stacks[0].slots.len(), 2);
        assert_eq!(plan.stacks[1].slots.len(), 1);
        let breaks = plan.calibration_breaks();
        assert_eq!(breaks.len(), 1);
        assert_eq!(breaks[0]["before"]["last"], "2024-01-08T15:10:00Z");
        assert_eq!(breaks[0]["after"]["first"], "2024-01-13T15:10:00Z");
        assert_eq!(breaks[0]["after"]["offset"], 0.0);
    }

    #[test]
    fn simultaneous_calibrations_are_ambiguous_not_chronological_breaks() {
        let mut input = series();
        let mut variants: Vec<_> = input
            .iter()
            .filter(|o| o.datetime.starts_with("2024-01-08"))
            .cloned()
            .collect();
        for o in &mut variants {
            o.offset = Some(0.);
            o.asset_id.push_str("-reprocessed");
            o.item_id = Some("reprocessed-item".into());
        }
        input.extend(variants);
        let plan = plan_observations(input, vec![]);
        let summary = plan.summary();
        assert!(summary["calibration_breaks"].as_array().unwrap().is_empty());
        assert_eq!(
            summary["calibration_ambiguities"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            summary["calibration_ambiguities"][0]["datetime"],
            "2024-01-08T15:10:00Z"
        );
    }

    #[test]
    fn different_grids_split_without_a_calibration_break() {
        let mut input = series();
        for o in input
            .iter_mut()
            .filter(|o| o.datetime.starts_with("2024-01-13"))
        {
            o.transform[0] += 10.;
        }
        let plan = plan_observations(input, vec![]);
        assert_eq!(plan.stacks.len(), 2);
        assert!(plan.calibration_breaks().is_empty());
    }

    #[test]
    fn grids_differ_by_any_bit_of_the_transform() {
        let mut input = series();
        input[0].transform[1] = f64::from_bits(10f64.to_bits() + 1);
        input[1].transform[1] = input[0].transform[1];
        let plan = plan_observations(input, vec![]);
        assert_eq!(plan.stacks.len(), 2);
    }

    // On-disk fixtures in the per-observation layout the scientific writer publishes (Zarr v2).
    struct Fixture<'a> {
        band: &'a str,
        item: &'a str,
        datetime: &'a str,
        values: Vec<u16>,
        valid: Vec<u8>,
        aoi: Vec<u8>,
    }

    fn v2(
        store: &Arc<FilesystemStore>,
        path: &str,
        shape: &[u64],
        dtype: &str,
        attrs: Value,
    ) -> Array<FilesystemStore> {
        let metadata = json!({"zarr_format":2,"shape":shape,"chunks":shape,"dtype":dtype,"order":"C",
            "filters":null,"compressor":null,"fill_value":0});
        let mut array = Array::new_with_metadata(
            store.clone(),
            path,
            serde_json::from_value(metadata).unwrap(),
        )
        .unwrap();
        *array.attributes_mut() = serde_json::from_value(attrs).unwrap();
        array.store_metadata().unwrap();
        array
    }

    fn write_fixture(dir: &Path, fixtures: &[Fixture], status: &str) {
        let (h, w) = (4u64, 3u64);
        let store = Arc::new(FilesystemStore::new(dir).unwrap());
        let group = |path: &str, attrs: Value| {
            let mut g = Group::new_with_metadata(
                store.clone(),
                path,
                serde_json::from_value(json!({"zarr_format":2})).unwrap(),
            )
            .unwrap();
            *g.attributes_mut() = serde_json::from_value(attrs).unwrap();
            g.store_metadata().unwrap();
        };
        group("/", json!({}));
        group("/observations", json!({}));
        let mut records = Vec::new();
        for (i, f) in fixtures.iter().enumerate() {
            let base = format!("/observations/{i:06}");
            group(
                &base,
                json!({"source":{"requested_band":f.band,"item_id":f.item,"collection":"sentinel-2-c1-l2a","href":format!("https://example.test/{}/{}.tif",f.item,f.band),"object_version":format!("\"etag-{i}\"")}}),
            );
            let values = v2(
                &store,
                &format!("{base}/values"),
                &[1, h, w],
                "<u2",
                json!({"_ARRAY_DIMENSIONS":["time","y","x"],"scale_factor":1e-4,"add_offset":-0.1}),
            );
            values
                .store_array_subset(&values.subset_all(), &f.values)
                .unwrap();
            for (name, data) in [("valid", &f.valid), ("aoi_mask", &f.aoi)] {
                let a = v2(
                    &store,
                    &format!("{base}/{name}"),
                    &[1, h, w],
                    "|u1",
                    json!({"_ARRAY_DIMENSIONS":["time","y","x"]}),
                );
                a.store_array_subset(&a.subset_all(), data).unwrap();
            }
            for (axis, len, origin, step) in [("x", w, 500000., 10.), ("y", h, 9100000., -10.)] {
                let a = v2(
                    &store,
                    &format!("{base}/{axis}"),
                    &[len],
                    "<f8",
                    json!({"_ARRAY_DIMENSIONS":[axis],"units":"m"}),
                );
                let centres: Vec<f64> =
                    (0..len).map(|j| origin + (j as f64 + 0.5) * step).collect();
                a.store_array_subset(&a.subset_all(), &centres).unwrap();
            }
            let crs = v2(
                &store,
                &format!("{base}/spatial_ref"),
                &[],
                "<i4",
                json!({"_ARRAY_DIMENSIONS":[],"crs_wkt":"EPSG:32718"}),
            );
            crs.store_array_subset(&crs.subset_all(), &[0i32]).unwrap();
            records.push(json!({"group":&base[1..],"asset_id":format!("{}-{}",f.item,f.band),"datetime":f.datetime,
                "dtype":"uint16","nodata":0.0,"native_grid":{"crs":"EPSG:32718","transform":[500000.0,10.0,0.0,9100000.0,0.0,-10.0]},
                "dimensions":{"names":["time","y","x"],"shape":[1,h,w]}}));
        }
        let manifest =
            json!({"document_type":"acquisition_result","status":status,"outputs":records});
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn fixtures() -> Vec<Fixture<'static>> {
        let mut out = Vec::new();
        for (d, (item, datetime)) in [("a", "2024-01-03T15:10:00Z"), ("b", "2024-01-08T15:10:00Z")]
            .into_iter()
            .enumerate()
        {
            for (b, band) in ["B04", "B08"].into_iter().enumerate() {
                let base = (100 * d + 10 * b) as u16;
                let mut valid = vec![1u8; 12];
                valid[b] = 0; // B04 invalid at pixel 0, B08 at pixel 1
                out.push(Fixture {
                    band,
                    item,
                    datetime,
                    values: (0..12).map(|p| base + p + 1).collect(),
                    valid,
                    aoi: vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0],
                });
            }
        }
        out
    }

    fn read<T: zarrs::array::ElementOwned>(dir: &Path, path: &str) -> Vec<T> {
        let store = Arc::new(FilesystemStore::new(dir).unwrap());
        let a = Array::open(store, path).unwrap();
        a.retrieve_array_subset::<Vec<T>>(&a.subset_all()).unwrap()
    }

    #[test]
    fn written_stack_copies_values_and_masks_exactly() {
        let dir = root();
        let input = dir.join("in.zarr");
        fs::create_dir_all(&input).unwrap();
        let f = fixtures();
        write_fixture(&input, &f, "complete");
        let output = dir.join("out.zarr");
        // Chunk edge 2 forces several tiles, including partial edge tiles.
        let manifest = write(&input, &output, 2, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(manifest["status"], "complete");
        let values: Vec<u16> = read(&output, "/stacks/0/values");
        let expected: Vec<u16> = f.iter().flat_map(|f| f.values.clone()).collect();
        assert_eq!(
            values, expected,
            "time, band order equals the per-observation sources"
        );
        let valid: Vec<u8> = read(&output, "/stacks/0/valid");
        assert_eq!(
            valid,
            f.iter().flat_map(|f| f.valid.clone()).collect::<Vec<_>>()
        );
        let all: Vec<u8> = read(&output, "/stacks/0/valid_all");
        assert_eq!(&all[..3], &[0, 0, 1]);
        assert_eq!(&all[12..15], &[0, 0, 1]);
        let aoi: Vec<u8> = read(&output, "/stacks/0/aoi_mask");
        assert_eq!(aoi, f[0].aoi);
        let time: Vec<i64> = read(&output, "/stacks/0/time");
        assert_eq!(time, [1704294600000, 1704726600000]);
        let band: Vec<String> = read(&output, "/stacks/0/band");
        assert_eq!(band, ["B04", "B08"]);
        let x: Vec<f64> = read(&output, "/stacks/0/x");
        assert_eq!(x, [500005., 500015., 500025.]);
        let group: Value =
            serde_json::from_slice(&fs::read(output.join("stacks/0/zarr.json")).unwrap()).unwrap();
        let children = group["consolidated_metadata"]["metadata"]
            .as_object()
            .unwrap();
        for name in [
            "values",
            "valid",
            "valid_all",
            "aoi_mask",
            "time",
            "band",
            "x",
            "y",
            "spatial_ref",
        ] {
            assert!(
                children.contains_key(name),
                "consolidated metadata lacks {name}"
            );
        }
        let values_meta = &children["values"];
        assert_eq!(values_meta["attributes"]["scale_factor"], 1e-4);
        assert_eq!(
            values_meta["dimension_names"],
            json!(["time", "band", "y", "x"])
        );
        assert_eq!(
            group["attributes"]["observations"][3]["object_version"],
            "\"etag-3\""
        );
        assert!(
            fs::metadata(input.join("manifest.json")).is_ok(),
            "input is untouched"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn differing_aoi_refuses_at_write() {
        let dir = root();
        let input = dir.join("in.zarr");
        fs::create_dir_all(&input).unwrap();
        let mut f = fixtures();
        f[3].aoi[0] = 0;
        write_fixture(&input, &f, "complete");
        let output = dir.join("out.zarr");
        let error = write(&input, &output, 2, Arc::new(AtomicBool::new(false)))
            .unwrap_err()
            .to_string();
        assert!(error.contains("aoi_mask"), "{error}");
        assert!(
            !output.exists(),
            "nothing is published when every stack is refused"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nonbinary_validity_is_an_integrity_error() {
        let dir = root();
        let input = dir.join("in.zarr");
        fs::create_dir_all(&input).unwrap();
        let mut f = fixtures();
        f[0].valid[0] = 2;
        write_fixture(&input, &f, "complete");
        let output = dir.join("out.zarr");
        let error = write(&input, &output, 2, Arc::new(AtomicBool::new(false)))
            .unwrap_err()
            .to_string();
        assert!(error.contains("observations/000000/valid"), "{error}");
        assert!(error.contains("only 0 or 1"), "{error}");
        assert!(!output.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn incomplete_input_is_rejected() {
        let dir = root();
        let input = dir.join("in.zarr");
        fs::create_dir_all(&input).unwrap();
        write_fixture(&input, &fixtures(), "incomplete");
        let error = plan(&input).unwrap_err().to_string();
        assert!(
            error.contains("not a complete acquisition_result"),
            "{error}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn undated_observation_is_refused_alone() {
        let dir = root();
        let input = dir.join("in.zarr");
        fs::create_dir_all(&input).unwrap();
        write_fixture(&input, &fixtures(), "complete");
        let path = input.join("manifest.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut extra = manifest["outputs"][0].clone();
        extra["datetime"] = Value::Null;
        extra["asset_id"] = json!("undated");
        manifest["outputs"].as_array_mut().unwrap().push(extra);
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let summary = plan(&input).unwrap();
        assert_eq!(summary["stacks"].as_array().unwrap().len(), 1);
        assert_eq!(summary["refusals"][0]["assets"], json!(["undated"]));
        fs::remove_dir_all(dir).unwrap();
    }
}
