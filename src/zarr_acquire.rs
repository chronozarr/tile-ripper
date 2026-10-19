//! Narrow local CF-style Zarr v2 acquisition. Codecs and storage belong to zarrs.
use crate::{cog, fail, model::*, planner::push, Result};
use std::{fs, path::Path, sync::Arc, time::Instant};
use zarrs::{array::Array, array_subset::ArraySubset, filesystem::FilesystemStore};
type Store = FilesystemStore;
fn store(a: &Asset) -> Result<Arc<Store>> {
    if cog::remote(&a.locator) || a.etag.is_some() {
        return fail("Zarr supports immutable local directory stores only");
    }
    Ok(Arc::new(Store::new(&a.locator)?))
}
fn json(root: &Path, key: &str) -> Result<serde_json::Value> {
    let path = root.join(key);
    if fs::metadata(&path)?.len() > 1024 * 1024 {
        return fail("Zarr metadata exceeds 1 MiB");
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn name(path: &str) -> Result<&str> {
    let s = path.strip_prefix('/').unwrap_or(path);
    if s.is_empty()
        || s == "."
        || s == ".."
        || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || ["x", "y", "time"].contains(&s)
    {
        return fail("variables must be top-level named data arrays");
    }
    Ok(s)
}
fn meta(root: &Path, path: &str, dims: &[&str], dtype: &str) -> Result<serde_json::Value> {
    let m = json(root, &format!("{path}/.zarray"))?;
    let attrs = json(root, &format!("{path}/.zattrs"))?;
    if m["zarr_format"] != 2
        || m["dtype"] != dtype
        || m["order"] != "C"
        || m["filters"] != serde_json::Value::Null
        || attrs["_ARRAY_DIMENSIONS"] != serde_json::json!(dims)
    {
        return fail(
            "unsupported Zarr array: require v2 C-order, defined dtype/dimensions and no filters",
        );
    }
    for key in [
        "grid_mapping",
        "ancillary_variables",
        "bounds",
        "cell_measures",
    ] {
        if attrs.get(key).is_some() {
            return fail("auxiliary CF object references are unsupported");
        }
    }
    if attrs.get("coordinates").is_some_and(|v| {
        v.as_str().is_none_or(|s| {
            s.split_whitespace()
                .any(|c| !["time", "y", "x"].contains(&c))
        })
    }) {
        return fail("auxiliary coordinates are unsupported");
    }
    let codec = m["compressor"]["id"].as_str();
    if !m["compressor"].is_null() && codec != Some("gzip") {
        return fail("supported codecs: none or gzip");
    }
    let shape = sizes(&m, "shape")?;
    let chunks = sizes(&m, "chunks")?;
    if shape.len() != dims.len()
        || chunks.len() != dims.len()
        || shape.contains(&0)
        || chunks.contains(&0)
    {
        return fail("invalid nonempty shape/chunks");
    }
    Ok(m)
}
fn sizes(m: &serde_json::Value, k: &str) -> Result<Vec<usize>> {
    m[k].as_array()
        .ok_or("missing shape/chunks")?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| "invalid dimension size".into())
        })
        .collect()
}
struct Dataset {
    layout: Layout,
    times: Vec<i64>,
    x: Vec<f64>,
    y: Vec<f64>,
    root_attrs: serde_json::Value,
}
fn inspect(a: &Asset, r: &Request) -> Result<Dataset> {
    let root = Path::new(&a.locator);
    let s = store(a)?;
    if json(root, ".zgroup")?["zarr_format"] != 2 {
        return fail("requires a Zarr v2 group");
    }
    let attrs = json(root, ".zattrs")?;
    let grid = &attrs["tile_ripper_grid"];
    let transform: [f64; 6] = serde_json::from_value(grid["transform"].clone())?;
    if !transform.iter().all(|x| x.is_finite())
        || transform[1] <= 0.
        || transform[5] >= 0.
        || transform[2] != 0.
        || transform[4] != 0.
    {
        return fail("requires finite north-up affine grid");
    }
    let crs = grid["crs"].as_str().ok_or("missing CRS")?.to_owned();
    gdal::spatial_ref::SpatialRef::from_definition(&crs)?;
    let mut coordinates = Vec::new();
    for (axis, dtype) in [("x", "<f8"), ("y", "<f8"), ("time", "<i8")] {
        let m = meta(root, axis, &[axis], dtype)?;
        let count = sizes(&m, "shape")?[0];
        // Coordinates and their native chunks must also fit the explicit working budget.
        if count
            .checked_mul(8)
            .is_none_or(|n| n > r.budget.buffer_bytes / 16)
            || sizes(&m, "chunks")?[0]
                .checked_mul(8)
                .is_none_or(|n| n > r.budget.buffer_bytes / 16)
        {
            return fail("coordinate axis exceeds buffer budget");
        }
        coordinates.push(Array::open(s.clone(), &format!("/{axis}"))?);
    }
    let x = coordinates[0].retrieve_array_subset_elements::<f64>(&coordinates[0].subset_all())?;
    let y = coordinates[1].retrieve_array_subset_elements::<f64>(&coordinates[1].subset_all())?;
    let times =
        coordinates[2].retrieve_array_subset_elements::<i64>(&coordinates[2].subset_all())?;
    let ta = json(root, "time/.zattrs")?;
    if ta["units"] != "seconds since 1970-01-01 00:00:00"
        || ta["calendar"] != "proleptic_gregorian"
        || times.windows(2).any(|w| w[0] >= w[1])
    {
        return fail("time must be ascending unique i64 Unix seconds with explicit proleptic_gregorian calendar");
    }
    for (values, origin, step) in [
        (&x, transform[0], transform[1]),
        (&y, transform[3], transform[5]),
    ] {
        for (i, v) in values.iter().enumerate() {
            let expected = origin + (i as f64 + 0.5) * step;
            if !v.is_finite() || (v - expected).abs() > step.abs() * 1e-8 {
                return fail("coordinates disagree with affine pixel centers");
            }
        }
    }
    Ok(Dataset {
        layout: Layout {
            width: x.len(),
            height: y.len(),
            transform,
            crs,
            variable: Variable::Dataset { path: "".into() },
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
        root_attrs: attrs,
    })
}
pub(crate) fn plan_asset(a: &Asset, r: &Request, p: &mut Plan) -> Result<()> {
    if r.budget
        .chunk_edge
        .checked_mul(r.budget.chunk_edge)
        .and_then(|n| n.checked_mul(2))
        .is_none_or(|n| n > r.budget.buffer_bytes / 4)
    {
        return fail("Zarr output buffer exceeds quarter buffer_bytes");
    }
    let d = inspect(a, r)?;
    let root = Path::new(&a.locator);
    let Spatial::Bounds { .. } = &r.spatial else {
        return fail("Zarr path supports native bounds only; polygon masking is unsupported");
    };
    let Some(window) = crate::spatial::window(&d.layout, &r.spatial)? else {
        return Ok(());
    };
    let mut ti = Vec::new();
    for (i, t) in d.times.iter().enumerate() {
        let datetime = chrono::DateTime::from_timestamp(*t, 0)
            .ok_or("time outside supported timestamp range")?
            .to_rfc3339();
        let mut observation = a.clone();
        observation.datetime = Some(datetime);
        if crate::planner::time_matches(&observation, &r.time)? {
            ti.push(i);
        }
    }
    if ti.is_empty() {
        return Ok(());
    }
    let inspect = push(p, r, &a.id, vec![], Operation::Inspect { cached: false })?;
    let mut variable_names = std::collections::HashSet::new();
    for v in &r.variables {
        let Variable::Dataset { path } = v else {
            return fail("Zarr requires dataset variable selectors");
        };
        let path = name(path)?;
        if !variable_names.insert(path) {
            return fail("duplicate dataset path alias");
        }
        let m = meta(root, path, &["time", "y", "x"], "<u2")?;
        let shape = sizes(&m, "shape")?;
        let chunks = sizes(&m, "chunks")?;
        if shape != vec![d.times.len(), d.y.len(), d.x.len()] {
            return fail("variables must share coordinate dimensions");
        }
        let chunk_bytes = chunks
            .iter()
            .try_fold(2usize, |n, x| n.checked_mul(*x))
            .ok_or("chunk size overflow")?;
        if chunk_bytes > r.budget.buffer_bytes / 2 {
            return fail("decoded source chunk exceeds half buffer_bytes; increase budget");
        }
        let sep = m["dimension_separator"].as_str().unwrap_or(".");
        if sep != "." && sep != "/" {
            return fail("unsupported chunk separator");
        }
        let mut keys = std::collections::BTreeMap::new();
        let mut payload = 0u64;
        for t in &ti {
            for cy in window.y / chunks[1]..=(window.y + window.height - 1) / chunks[1] {
                for cx in window.x / chunks[2]..=(window.x + window.width - 1) / chunks[2] {
                    let key = format!("{path}/{}{sep}{cy}{sep}{cx}", t / chunks[0]);
                    if keys
                        .insert(key.clone(), vec![t / chunks[0], cy, cx])
                        .is_some()
                    {
                        continue;
                    }
                    if keys.len() > r.budget.max_nodes {
                        return fail("source chunk enumeration exceeds max_nodes");
                    }
                    let bytes = match fs::metadata(root.join(&key)) {
                        Ok(m) => m.len(),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
                        Err(e) => return Err(e.into()),
                    };
                    if bytes > r.budget.buffer_bytes as u64 / 2 {
                        return fail("encoded source chunk exceeds half buffer_bytes");
                    }
                    payload = payload
                        .checked_add(bytes)
                        .ok_or("payload estimate overflow")?;
                }
            }
        }
        p.estimates.selected_compressed_payload_bytes = p
            .estimates
            .selected_compressed_payload_bytes
            .and_then(|n| n.checked_add(payload));
        let fill = m["fill_value"]
            .as_u64()
            .filter(|v| *v <= 65535)
            .ok_or("requires explicit UInt16 fill_value")?;
        let attrs = json(root, &format!("{path}/.zattrs"))?;
        if attrs
            .get("_FillValue")
            .is_some_and(|v| v.as_u64() != Some(fill))
        {
            return fail("_FillValue and storage fill_value disagree");
        }
        let mut layout = d.layout.clone();
        layout.variable = v.clone();
        layout.block = [chunks[2], chunks[1]];
        layout.nodata = Some(fill as f64);
        layout.scale = attrs["scale_factor"].as_f64();
        layout.offset = attrs["add_offset"].as_f64();
        p.selected.push(Selected {
            timestamps: ti
                .iter()
                .map(|i| {
                    chrono::DateTime::from_timestamp(d.times[*i], 0)
                        .expect("validated timestamp")
                        .to_rfc3339()
                })
                .collect(),
            dimensions: Some(DimensionSelection {
                shape: shape.clone(),
                chunk_shape: chunks.clone(),
                names: vec!["time".into(), "y".into(), "x".into()],
                indices: vec![
                    ti.clone(),
                    (window.y..window.y + window.height).collect(),
                    (window.x..window.x + window.width).collect(),
                ],
            }),
            mask_geometry: None,
            asset_id: a.id.clone(),
            datetime: None,
            reason: format!("native coordinate selection: {path}"),
            capabilities: Capabilities {
                selection: vec!["native bounds, time, variables".into()],
                reads: vec!["chunk_object".into()],
                decoding: "zarrs".into(),
                compression: "none/gzip".into(),
                materialization: true,
            },
            layout: Some(layout),
            window: window.clone(),
        });
        for (key, indices) in keys {
            let read = push(
                p,
                r,
                &a.id,
                vec![inspect],
                Operation::Read {
                    read: Read::ChunkObject { key, indices },
                },
            )?;
            let decode = push(
                p,
                r,
                &a.id,
                vec![read],
                Operation::Decode {
                    reader: "zarrs".into(),
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

    p.limitations.push("Zarr: immutable local v2 group; UInt16 time/y/x arrays; native bounds; none/gzip; each selected source chunk retrieved once; missing chunks require a zero-byte lookup. buffer_bytes bounds coordinates, one decoded chunk and returned buffer separately, not RSS/codec overhead. Missing chunks retain fill_value. No calibration.".into());
    Ok(())
}
pub(crate) fn execute_selected(
    r: &Request,
    p: &Plan,
    a: &Asset,
    selected: &Selected,
    report: &mut crate::ExecutionReport,
    sink: &mut impl FnMut(crate::Chunk) -> Result<()>,
) -> Result<()> {
    let t = Instant::now();
    let s = store(a)?;
    let d = inspect(a, r)?;
    let layout = selected.layout.as_ref().ok_or("missing layout")?;
    let Variable::Dataset { path } = &layout.variable else {
        return fail("missing dataset variable");
    };
    let path = name(path)?;
    let trace = Arc::new(std::sync::Mutex::new(ReadTrace::default()));
    let logged = Arc::new(
        zarrs::storage::storage_adapter::usage_log::UsageLogStorageAdapter::new(
            s,
            trace.clone(),
            String::new,
        ),
    );
    let array = Array::open(logged, &format!("/{path}"))?;
    *trace.lock().map_err(|_| "trace lock poisoned")? = ReadTrace::default();
    report.source_open_seconds += t.elapsed().as_secs_f64();
    let dims = selected
        .dimensions
        .as_ref()
        .ok_or("missing native dimensions")?;
    let cs = &dims.chunk_shape;
    let w = &selected.window;
    for node in &p.nodes {
        let Operation::Read {
            read: Read::ChunkObject { key, indices },
        } = &node.operation
        else {
            continue;
        };
        if node.asset_id != a.id || !key.starts_with(&format!("{path}/")) {
            continue;
        }
        let t = Instant::now();
        let native = array.retrieve_chunk_elements_opt::<u16>(
            &indices.iter().map(|v| *v as u64).collect::<Vec<_>>(),
            &serial(),
        )?;
        report.read_decode_seconds += t.elapsed().as_secs_f64();
        report.native_materialized_bytes =
            Some(report.native_materialized_bytes.unwrap_or(0) + native.len() * 2);
        let origin: Vec<_> = indices.iter().zip(cs).map(|(i, c)| i * c).collect();
        let x0 = w.x.max(origin[2]);
        let x1 = (w.x + w.width).min(origin[2] + cs[2]);
        let y0 = w.y.max(origin[1]);
        let y1 = (w.y + w.height).min(origin[1] + cs[1]);
        for ti in dims.indices[0]
            .iter()
            .copied()
            .filter(|t| *t >= origin[0] && *t < origin[0] + cs[0])
        {
            for y in (y0..y1).step_by(r.budget.chunk_edge) {
                for x in (x0..x1).step_by(r.budget.chunk_edge) {
                    let width = (x1 - x).min(r.budget.chunk_edge);
                    let height = (y1 - y).min(r.budget.chunk_edge);
                    let mut pixels = Vec::with_capacity(width * height);
                    for sy in y..y + height {
                        let offset =
                            ((ti - origin[0]) * cs[1] + sy - origin[1]) * cs[2] + x - origin[2];
                        pixels.extend_from_slice(&native[offset..offset + width]);
                    }
                    report.decoded_bytes += pixels.len() * 2;
                    report.non_nodata_pixels += pixels
                        .iter()
                        .filter(|v| Some(**v as f64) != layout.nodata)
                        .count();
                    let window = Window {
                        x,
                        y,
                        width,
                        height,
                    };
                    let chunk = crate::Chunk {
                        node_id: node.id,
                        asset_id: a.id.clone(),
                        datetime: Some(
                            chrono::DateTime::from_timestamp(d.times[ti], 0)
                                .ok_or("invalid time")?
                                .to_rfc3339(),
                        ),
                        variable: layout.variable.clone(),
                        indices: vec![ti],
                        transform: crate::spatial::window_transform(layout, &window),
                        crs: layout.crs.clone(),
                        nodata: layout.nodata,
                        scale: layout.scale,
                        offset: layout.offset,
                        window,
                        pixels,
                    };
                    let t = Instant::now();
                    sink(chunk)?;
                    report.sink_seconds += t.elapsed().as_secs_f64();
                    report.chunks += 1;
                }
            }
        }
    }
    let trace = trace.lock().map_err(|_| "trace lock poisoned")?;
    report.source_data_requests = Some(report.source_data_requests.unwrap_or(0) + trace.requests);
    report.source_data_bytes = Some(report.source_data_bytes.unwrap_or(0) + trace.bytes);
    for (key, (requests, bytes)) in &trace.keys {
        report.source_reads.push(crate::execute::SourceRead {
            asset_id: a.id.clone(),
            key: key.clone(),
            requests: *requests,
            bytes: *bytes,
        });
    }
    Ok(())
}
#[derive(Default)]
struct ReadTrace {
    pending: Vec<u8>,
    requests: u64,
    bytes: u64,
    keys: std::collections::BTreeMap<String, (u64, u64)>,
}
impl std::io::Write for ReadTrace {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(b);
        while let Some(end) = self.pending.iter().position(|v| *v == b'\n') {
            let line = String::from_utf8_lossy(&self.pending[..end]);
            if line.starts_with("get(") || line.starts_with("get_partial_many(") {
                self.requests += 1;
                let mut bytes = 0;
                if let Some((_, v)) = line.split_once("len=") {
                    bytes = v
                        .split(|c: char| !c.is_ascii_digit())
                        .filter(|v| !v.is_empty())
                        .filter_map(|v| v.parse::<u64>().ok())
                        .sum::<u64>();
                }
                self.bytes += bytes;
                if let Some(key) = line
                    .split_once('(')
                    .and_then(|(_, v)| v.split([',', ')']).next())
                {
                    let entry = self.keys.entry(key.into()).or_default();
                    entry.0 += 1;
                    entry.1 += bytes;
                }
            }
            self.pending.drain(..=end);
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn output_array(
    source: &Path,
    target: Arc<Store>,
    path: &str,
    shape: Vec<usize>,
    chunks: Vec<usize>,
) -> Result<Array<Store>> {
    let mut m = json(source, &format!("{path}/.zarray"))?;
    m["shape"] = serde_json::json!(shape);
    m["chunks"] = serde_json::json!(chunks);
    // zarrs owns metadata validation, chunk encoding and writing.
    let mut array =
        Array::new_with_metadata(target, &format!("/{path}"), serde_json::from_value(m)?)?;
    *array.attributes_mut() = serde_json::from_value(json(source, &format!("{path}/.zattrs"))?)?;
    array.store_metadata_opt(
        &zarrs::array::ArrayMetadataOptions::default().with_include_zarrs_metadata(false),
    )?;
    Ok(array)
}
pub(crate) fn fetch(r: &Request, cache: &mut cog::LayoutCache) -> Result<crate::ExecutionReport> {
    let Sink::ZarrDirectory { path } = &r.sink else {
        unreachable!()
    };
    let _ = path;
    let t = Instant::now();
    let p = crate::plan(r, cache)?;
    let planning = t.elapsed().as_secs_f64();
    fetch_plan(r, &p, planning)
}
pub(crate) fn fetch_plan(r: &Request, p: &Plan, planning: f64) -> Result<crate::ExecutionReport> {
    let Sink::ZarrDirectory { path } = &r.sink else {
        return fail("Zarr requires a native Zarr directory sink");
    };
    let root = Path::new(path);
    if p.selected.iter().any(|s| s.dimensions.is_none()) {
        return fail("Zarr directory sink requires supported Zarr group assets");
    }
    fs::create_dir(root)?;
    let result = (|| -> Result<crate::ExecutionReport> {
        let Source::Assets { assets } = &r.source else {
            unreachable!()
        };
        let mut outputs = std::collections::HashMap::new();
        for a in assets {
            let selected: Vec<_> = p.selected.iter().filter(|s| s.asset_id == a.id).collect();
            if selected.is_empty() {
                continue;
            }
            let d = inspect(a, r)?;
            let s = Arc::new(Store::new(root.join(format!("{}.zarr", a.id)))?);
            let w = &selected[0].window;
            let indices = &selected[0].dimensions.as_ref().unwrap().indices[0];
            let mut group = zarrs::group::Group::new_with_metadata(
                s.clone(),
                "/",
                serde_json::from_value(serde_json::json!({"zarr_format":2}))?,
            )?;
            let mut attrs = d.root_attrs;
            attrs["tile_ripper_grid"]["transform"] =
                serde_json::json!(crate::spatial::window_transform(&d.layout, w));
            *group.attributes_mut() = serde_json::from_value(attrs)?;
            group.store_metadata()?;
            let source = Path::new(&a.locator);
            for (axis, values) in [
                ("x", &d.x[w.x..w.x + w.width]),
                ("y", &d.y[w.y..w.y + w.height]),
            ] {
                let ar = output_array(
                    source,
                    s.clone(),
                    axis,
                    vec![values.len()],
                    vec![values.len()],
                )?;
                ar.store_array_subset_elements_opt(&ar.subset_all(), values, &serial())?;
            }
            let times: Vec<_> = indices.iter().map(|i| d.times[*i]).collect();
            let ar = output_array(
                source,
                s.clone(),
                "time",
                vec![times.len()],
                vec![times.len()],
            )?;
            ar.store_array_subset_elements_opt(&ar.subset_all(), &times, &serial())?;
            for sel in selected {
                let Variable::Dataset { path } = &sel.layout.as_ref().unwrap().variable else {
                    unreachable!()
                };
                let n = name(path)?;
                let ar = output_array(
                    source,
                    s.clone(),
                    n,
                    vec![indices.len(), w.height, w.width],
                    vec![
                        1,
                        r.budget.chunk_edge.min(w.height),
                        r.budget.chunk_edge.min(w.width),
                    ],
                )?;
                outputs.insert(
                    (a.id.clone(), n.to_string()),
                    (ar, w.clone(), indices.clone()),
                );
            }
        }
        let report = crate::execute::execute_plan(r, p, planning, |c| {
            let Variable::Dataset { path } = &c.variable else {
                return fail("unexpected variable");
            };
            let (ar, w, indices) = outputs
                .get(&(c.asset_id.clone(), name(path)?.to_string()))
                .ok_or("output missing")?;
            let ti = indices
                .iter()
                .position(|i| *i == c.indices[0])
                .ok_or("time missing")?;
            ar.store_array_subset_elements_opt(
                &ArraySubset::new_with_start_shape(
                    vec![
                        ti as u64,
                        (c.window.y - w.y) as u64,
                        (c.window.x - w.x) as u64,
                    ],
                    vec![1, c.window.height as u64, c.window.width as u64],
                )?,
                &c.pixels,
                &serial(),
            )?;
            Ok(())
        })?;
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"plan":p,"observed":report,"sources":assets,"decisions":"native raw values; no calibration; missing chunks fill; coordinates subset in source order; local sources must remain immutable during operation"}),
            )?,
        )?;
        Ok(report)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(root);
    }
    result
}

fn serial() -> zarrs::array::codec::CodecOptions {
    let mut o = zarrs::array::codec::CodecOptions::default();
    o.set_concurrent_target(1);
    o
}

#[cfg(test)]
mod tests {
    use super::ReadTrace;
    use std::io::Write;
    #[test]
    fn trace_counts_full_partial_and_absent_reads() {
        let mut t = ReadTrace::default();
        t.write_all(b"get(red/0.0.0) -> len=Ok(12)\nget_partial_many(red/0.1.0, [0..8, 20..24]) -> len=[8, 4]\nget_partial_many(red/1.1.1, [0..]) -> len=[]\n").unwrap();
        assert_eq!((t.requests, t.bytes), (3, 24));
    }
}
