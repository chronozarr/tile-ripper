//! Narrow local CF-style Zarr v2 acquisition. Codecs and storage belong to zarrs.
use crate::{cog, fail, model::*, planner::push, Result};
use std::{fs, path::Path, sync::Arc, time::Instant};
use zarrs::{array::Array, array_subset::ArraySubset, filesystem::FilesystemStore};
type Store = FilesystemStore;
type Metrics =
    zarrs::storage::storage_adapter::performance_metrics::PerformanceMetricsStorageAdapter<Store>;
type OutputStore = crate::zarr_io::Timed<Metrics>;
/// Zarr v2 consolidated metadata is an adapter-owned view over generic objects.
pub(crate) fn metadata_view(
    source: Arc<crate::storage::Objects>,
    budget: &Budget,
) -> Result<Arc<crate::storage::Objects>> {
    use zarrs::storage::ReadableStorageTraits;
    let Some(bytes) = source.get(&zarrs::storage::StoreKey::new(".zmetadata")?)? else {
        return Ok(source);
    };
    let doc: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "malformed consolidated metadata")?;
    if doc["zarr_consolidated_format"] != 1 {
        return fail("unsupported consolidated metadata version");
    }
    let entries = doc["metadata"]
        .as_object()
        .ok_or("malformed consolidated metadata map")?;
    if entries.len() > budget.max_nodes {
        return fail("consolidated metadata exceeds max_nodes");
    }
    let mut view = std::collections::BTreeMap::new();
    let mut total = 0usize;
    for (key, value) in entries {
        let encoded = serde_json::to_vec(value)?;
        total = total
            .checked_add(encoded.len())
            .ok_or("metadata size overflow")?;
        if total > budget.buffer_bytes / 2 {
            return fail("expanded consolidated metadata exceeds half buffer_bytes");
        }
        view.insert(key.clone(), encoded);
    }
    let entries = view;
    source.discard_cached(".zmetadata");
    source.reserve_overlay_cache(total)?;
    Ok(Arc::new(crate::storage::Objects::Overlay {
        origin: source,
        entries,
    }))
}
fn open_v2(
    source: Arc<crate::storage::Objects>,
    path: &str,
    budget: &Budget,
) -> Result<Array<crate::zarr_decode::Checked<crate::storage::Objects>>> {
    if source
        .admission_probe(&format!("{path}/zarr.json"))?
        .is_some()
    {
        return fail("Zarr v3 or hybrid v2/v3 arrays are unsupported");
    }
    crate::zarr_decode::array(
        source.clone(),
        path,
        json(&source, &format!("{path}/.zarray"))?,
        json(&source, &format!("{path}/.zattrs"))?,
        budget,
    )
}
fn store(a: &Asset) -> Result<Arc<crate::storage::Objects>> {
    crate::storage::Sources::default().get(a)
}
fn json(source: &crate::storage::Objects, key: &str) -> Result<serde_json::Value> {
    source.json(key)
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
fn meta(
    root: &crate::storage::Objects,
    path: &str,
    dims: &[&str],
    dtype: &str,
) -> Result<serde_json::Value> {
    let m = json(root, &format!("{path}/.zarray"))?;
    let attrs = json(root, &format!("{path}/.zattrs"))?;
    if m["zarr_format"] != 2
        || m["dtype"] != dtype
        || m["order"] != "C"
        || attrs["_ARRAY_DIMENSIONS"] != serde_json::json!(dims)
    {
        return fail("unsupported Zarr array: require v2 C-order and admitted dtype/dimensions");
    }
    if !m["filters"].is_null() {
        let filters = m["filters"].as_array().ok_or("invalid filter list")?;
        if dtype != "<f4"
            || filters.len() != 1
            || filters[0]["id"] != "fixedscaleoffset"
            || filters[0]["dtype"] != "<f4"
            || filters[0]["astype"] != "<f4"
            || filters[0]["scale"].as_f64().is_none_or(|v| {
                !v.is_finite() || v <= 0. || !(v as f32).is_finite() || (v as f32) <= 0.
            })
            || filters[0]["offset"]
                .as_f64()
                .is_none_or(|v| !v.is_finite() || !(v as f32).is_finite())
        {
            return fail("only Float32-to-Float32 fixedscaleoffset filter supported");
        }
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
                .any(|c| !["time", "y", "x", "latitude", "longitude", "lat", "lon"].contains(&c))
        })
    }) {
        return fail("auxiliary coordinates are unsupported");
    }
    let codec = m["compressor"]["id"].as_str();
    if !m["compressor"].is_null() && !matches!(codec, Some("gzip" | "blosc")) {
        return fail("supported codecs: none, gzip or Blosc/LZ4");
    }
    if codec == Some("gzip") && m["compressor"]["level"].as_u64().is_none_or(|v| v > 9) {
        return fail("gzip level must be an integer from 0 to 9");
    }
    if codec == Some("blosc")
        && (!matches!(m["compressor"]["cname"].as_str(), Some("lz4" | "zstd"))
            || !matches!(m["compressor"]["shuffle"].as_u64(), Some(0..=2))
            || m["compressor"]["clevel"].as_u64().is_none_or(|v| v > 9)
            || m["compressor"]["blocksize"].as_u64().is_none_or(|v| v != 0))
    {
        return fail("only Blosc LZ4/Zstd shuffle 0..2 automatic blocksize supported");
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
    for dims in [&shape, &chunks] {
        dims.iter()
            .try_fold(8usize, |n, v| n.checked_mul(*v))
            .ok_or("array size overflow")?;
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
    axes: [String; 2],
    raw_times: Vec<i64>,
    conventional: bool,
}
fn inspect_on(r: &Request, s: Arc<crate::storage::Objects>) -> Result<Dataset> {
    let root = s.as_ref();
    if root.admission_probe("zarr.json")?.is_some() {
        return fail("Zarr v3 or hybrid v2/v3 stores are unsupported");
    }
    if json(root, ".zgroup")?["zarr_format"] != 2 {
        return fail("requires a Zarr v2 group");
    }
    let attrs = json(root, ".zattrs")?;
    let conventional = !attrs["tile_ripper_grid"].is_object();
    let axes: [String; 2] = if conventional {
        let Variable::Dataset { path } = &r.variables[0] else {
            return fail("Zarr needs dataset variables");
        };
        let va = json(root, &format!("{}/.zattrs", name(path)?))?;
        let dims = va["_ARRAY_DIMENSIONS"]
            .as_array()
            .ok_or("missing coordinate dimensions")?;
        if dims.contains(&serde_json::json!("lat")) && dims.contains(&serde_json::json!("lon")) {
            ["lon".into(), "lat".into()]
        } else {
            ["longitude".into(), "latitude".into()]
        }
    } else {
        ["x".into(), "y".into()]
    };
    let mut transform: [f64; 6] = [0.; 6];
    let crs = if conventional {
        "EPSG:4326".to_string()
    } else {
        let grid = &attrs["tile_ripper_grid"];
        transform = serde_json::from_value(grid["transform"].clone())?;
        if !transform.iter().all(|v| v.is_finite())
            || transform[1] <= 0.
            || transform[5] >= 0.
            || transform[2] != 0.
            || transform[4] != 0.
        {
            return fail("requires finite north-up affine grid");
        }
        grid["crs"].as_str().ok_or("missing CRS")?.to_owned()
    };
    gdal::spatial_ref::SpatialRef::from_definition(&crs)?;
    let mut coordinates = Vec::new();
    for (axis, dtype) in [
        (axes[0].as_str(), "<f8"),
        (axes[1].as_str(), "<f8"),
        ("time", "<i8"),
    ] {
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
        coordinates.push(open_v2(s.clone(), axis, &r.budget)?);
    }
    let x = coordinates[0].retrieve_array_subset_elements::<f64>(&coordinates[0].subset_all())?;
    let y = coordinates[1].retrieve_array_subset_elements::<f64>(&coordinates[1].subset_all())?;
    let times =
        coordinates[2].retrieve_array_subset_elements::<i64>(&coordinates[2].subset_all())?;
    let raw_times = times;
    let ta = json(root, "time/.zattrs")?;
    if ta["calendar"] != "proleptic_gregorian" {
        return fail("requires proleptic_gregorian time calendar");
    }
    let (factor, epoch) = match ta["units"].as_str() {
        Some("seconds since 1970-01-01 00:00:00") => (1i64, 0i64),
        Some("hours since 1959-01-01") => (3600, -347155200),
        Some("days since 1998-01-01 00:00:00") => (86400, 883612800),
        _ => {
            return fail("unsupported time units; supported Unix seconds or hours since 1959-01-01")
        }
    };
    let times: Vec<i64> = raw_times
        .iter()
        .map(|v| {
            v.checked_mul(factor)
                .and_then(|v| v.checked_add(epoch))
                .ok_or("time conversion overflow")
        })
        .collect::<std::result::Result<_, _>>()?;
    if times.windows(2).any(|w| w[0] >= w[1])
        || times
            .iter()
            .any(|v| chrono::DateTime::from_timestamp(*v, 0).is_none())
    {
        return fail("time must be ascending unique supported timestamps");
    }
    if conventional {
        for (i, values) in [&x, &y].iter().enumerate() {
            let axis = &axes[i];
            let ca = json(root, &format!("{axis}/.zattrs"))?;
            let unit = if i == 0 {
                "degrees_east"
            } else {
                "degrees_north"
            };
            if ca.get("units").is_some_and(|v| v != unit)
                || ca.get("standard_name").is_some_and(|v| {
                    v.as_str().is_none_or(|v| {
                        !v.eq_ignore_ascii_case(if i == 0 { "longitude" } else { "latitude" })
                    })
                })
                || ca
                    .get("axis")
                    .is_some_and(|v| v != if i == 0 { "X" } else { "Y" })
            {
                return fail("ambiguous geographic coordinate attributes");
            }
            if values.len() < 2 {
                return fail("geographic axes need at least two centers");
            }
            let nominal = attrs[if i == 0 {
                "geospatial_lon_resolution"
            } else {
                "geospatial_lat_resolution"
            }]
            .as_f64();
            let step = if let Some(n) = nominal {
                if !n.is_finite() || n <= 0. {
                    return fail("invalid declared geographic resolution");
                }
                n.copysign(values[values.len() - 1] - values[0])
            } else {
                values[1] - values[0]
            };
            if !step.is_finite()
                || step == 0.
                || values.iter().any(|v| {
                    !v.is_finite()
                        || if i == 0 {
                            *v < -180. || *v > 360.
                        } else {
                            *v < -90. || *v > 90.
                        }
                })
            {
                return fail("invalid geographic coordinate range/spacing");
            }
            transform[if i == 0 { 0 } else { 3 }] = values[0] - step / 2.;
            transform[if i == 0 { 1 } else { 5 }] = step;
        }
        if (x[x.len() - 1] - x[0]).abs() >= 360. {
            return fail("duplicated/wrapped longitude unsupported");
        }
    }
    for (axis_index, (values, origin, step)) in [
        (&x, transform[0], transform[1]),
        (&y, transform[3], transform[5]),
    ]
    .into_iter()
    .enumerate()
    {
        let declared_rounding = conventional
            && attrs
                .get(if axis_index == 0 {
                    "geospatial_lon_resolution"
                } else {
                    "geospatial_lat_resolution"
                })
                .is_some();
        let tolerance = if declared_rounding {
            step.abs() * 1e-8
                + ((f32::EPSILON as f64) * values.iter().map(|v| v.abs()).fold(1., f64::max))
                    .min(step.abs() * 1e-3)
        } else {
            step.abs() * 1e-8
        };
        for (i, v) in values.iter().enumerate() {
            let expected = origin + (i as f64 + 0.5) * step;
            if !v.is_finite() || (v - expected).abs() > tolerance {
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
        axes,
        raw_times,
        conventional,
    })
}
pub(crate) fn plan_asset(a: &Asset, r: &Request, p: &mut Plan) -> Result<()> {
    plan_asset_on(a, r, p, store(a)?)
}
pub(crate) fn plan_asset_on(
    a: &Asset,
    r: &Request,
    p: &mut Plan,
    source: Arc<crate::storage::Objects>,
) -> Result<()> {
    if r.budget
        .chunk_edge
        .checked_mul(r.budget.chunk_edge)
        .and_then(|n| n.checked_mul(4))
        .is_none_or(|n| n > r.budget.buffer_bytes / 4)
    {
        return fail("Zarr output buffer exceeds quarter buffer_bytes");
    }
    let d = inspect_on(r, source.clone())?;
    if d.conventional {
        p.limitations.push("Conventional geographic profile: EPSG:4326 is the profile's interpretation of named latitude/longitude centers in degrees, not a source-declared datum. Coordinates are preserved without transformation; precise datum-dependent uses require provider verification.".into());
    }
    if d.root_attrs.get("geospatial_lon_resolution").is_some() {
        p.limitations.push("Declared regular spacing may have Float32-rounded coordinate centers. Native coordinate arrays and callback centers are authoritative; affine is approximate. Storage filters are decoded once; output stores logical Float32 values without re-quantization.".into());
    }
    let root = source.as_ref();
    let Spatial::Bounds { .. } = &r.spatial else {
        return fail("Zarr path supports native bounds only; polygon masking is unsupported");
    };
    let selected_window = if d.conventional {
        crate::spatial::coordinate_window(&d.layout.crs, &r.spatial, &d.x, &d.y)?
    } else {
        crate::spatial::window(&d.layout, &r.spatial)?
    };
    let Some(window) = selected_window else {
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
        let va = json(root, &format!("{path}/.zattrs"))?;
        let names: Vec<String> = serde_json::from_value(va["_ARRAY_DIMENSIONS"].clone())?;
        let expected_names = vec!["time".to_string(), d.axes[1].clone(), d.axes[0].clone()];
        let alternate = vec!["time".to_string(), d.axes[0].clone(), d.axes[1].clone()];
        if names != expected_names && (!d.conventional || names != alternate) {
            return fail("requires time first and two geographic axes only");
        }
        let yi = names.iter().position(|v| v == &d.axes[1]).unwrap();
        let xi = 3 - yi;
        let dtype = if d.conventional { "<f4" } else { "<u2" };
        let m = meta(
            root,
            path,
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
            dtype,
        )?;
        // Resolve every library metadata probe while the snapshot is inspectable.

        let shape = sizes(&m, "shape")?;
        let chunks = sizes(&m, "chunks")?;
        if shape[0] != d.times.len() || shape[yi] != d.y.len() || shape[xi] != d.x.len() {
            return fail("variables must share coordinate dimensions");
        }
        let chunk_bytes = chunks
            .iter()
            .try_fold(if d.conventional { 4usize } else { 2usize }, |n, x| {
                n.checked_mul(*x)
            })
            .ok_or("chunk size overflow")?;
        if chunk_bytes > r.budget.buffer_bytes / 2 {
            return fail("decoded source chunk exceeds half buffer_bytes; increase budget");
        }
        let _ = open_v2(source.clone(), path, &r.budget)?;
        let sep = m["dimension_separator"].as_str().unwrap_or(".");
        if sep != "." && sep != "/" {
            return fail("unsupported chunk separator");
        }
        let mut keys = std::collections::BTreeMap::new();
        let mut payload = 0u64;
        for t in &ti {
            for cy in window.y / chunks[yi]..=(window.y + window.height - 1) / chunks[yi] {
                for cx in window.x / chunks[xi]..=(window.x + window.width - 1) / chunks[xi] {
                    let coords = if yi == 1 {
                        [t / chunks[0], cy, cx]
                    } else {
                        [t / chunks[0], cx, cy]
                    };
                    let key = format!("{path}/{}{sep}{}{sep}{}", coords[0], coords[1], coords[2]);
                    if keys.insert(key.clone(), coords.to_vec()).is_some() {
                        continue;
                    }
                    if keys.len() > r.budget.max_nodes {
                        return fail("source chunk enumeration exceeds max_nodes");
                    }
                }
            }
        }
        let chunk_keys = keys.keys().cloned().collect::<Vec<_>>();
        let sizes = source.sizes(&chunk_keys)?;
        for bytes in sizes.into_iter().flatten() {
            if bytes > r.budget.buffer_bytes as u64 / 2 {
                return fail("encoded source chunk exceeds half buffer_bytes");
            }
            payload = payload
                .checked_add(bytes)
                .ok_or("payload estimate overflow")?;
        }
        p.estimates.selected_compressed_payload_bytes = p
            .estimates
            .selected_compressed_payload_bytes
            .and_then(|n| n.checked_add(payload));
        let attrs = json(root, &format!("{path}/.zattrs"))?;
        let fill = if d.conventional {
            if m["fill_value"] != "NaN" || attrs.get("_FillValue").is_some_and(|v| v != "NaN") {
                return fail("Float32 profile requires NaN fill");
            }
            f64::NAN
        } else {
            let fill = m["fill_value"]
                .as_u64()
                .filter(|v| *v <= 65535)
                .ok_or("requires explicit UInt16 fill_value")?;
            if attrs
                .get("_FillValue")
                .is_some_and(|v| v.as_u64() != Some(fill))
            {
                return fail("_FillValue and storage fill_value disagree");
            }
            fill as f64
        };
        let mut layout = d.layout.clone();
        layout.variable = v.clone();
        layout.block = [chunks[xi], chunks[yi]];
        layout.dtype = dtype.into();
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
                names: names.clone(),
                indices: if yi == 1 {
                    vec![
                        ti.clone(),
                        (window.y..window.y + window.height).collect(),
                        (window.x..window.x + window.width).collect(),
                    ]
                } else {
                    vec![
                        ti.clone(),
                        (window.x..window.x + window.width).collect(),
                        (window.y..window.y + window.height).collect(),
                    ]
                },
            }),
            mask_geometry: None,
            asset_id: a.id.clone(),
            datetime: None,
            reason: format!("native coordinate selection: {path}"),
            capabilities: Capabilities {
                selection: vec!["native bounds, time, variables".into()],
                reads: vec!["chunk_object".into()],
                decoding: "zarrs".into(),
                compression: "none/gzip/Blosc LZ4 or Zstd; shuffle/bitshuffle; admitted Float32 fixedscaleoffset".into(),
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

    p.limitations.push("Zarr v2: custom UInt16 grid or conventional Float32 geographic axes; time first; native bounds; validated bounded codecs; selected source chunks retrieved once; native fill. No calibration, mosaicking or reprojection. Buffer budget is not a process RSS ceiling. Geographic output dimensions normalize to time/latitude/longitude with source axis order recorded in plan.".into());
    Ok(())
}
type LoggedObjects = zarrs::storage::storage_adapter::usage_log::UsageLogStorageAdapter<
    crate::zarr_io::Timed<crate::storage::Objects>,
>;

/// Prepared reader for one selected Zarr array. Its storage handles and usage
/// trace stay on the worker that owns it; each planned source chunk is decoded
/// at most once as the scheduler calls `execute_node`.
pub(crate) struct SelectedReader {
    asset_id: String,
    key_prefix: String,
    array: Array<crate::zarr_decode::Checked<LoggedObjects>>,
    timed: Arc<crate::zarr_io::Timed<crate::storage::Objects>>,
    trace: Arc<std::sync::Mutex<ReadTrace>>,
    dataset: Dataset,
    layout: Layout,
    window: Window,
    dimensions: DimensionSelection,
    y_index: usize,
    x_index: usize,
}

impl SelectedReader {
    pub(crate) fn new(
        r: &Request,
        a: &Asset,
        sources: &crate::storage::Sources,
        selected: &Selected,
    ) -> Result<Self> {
        let s = sources.get(a)?;
        let d = inspect_on(r, s.clone())?;
        let layout = selected.layout.as_ref().ok_or("missing layout")?;
        let Variable::Dataset { path } = &layout.variable else {
            return fail("missing dataset variable");
        };
        let path = name(path)?;
        let timed = Arc::new(crate::zarr_io::Timed::new(s));
        let trace = Arc::new(std::sync::Mutex::new(ReadTrace::default()));
        let logged = Arc::new(
            zarrs::storage::storage_adapter::usage_log::UsageLogStorageAdapter::new(
                timed.clone(),
                trace.clone(),
                String::new,
            ),
        );
        let array = crate::zarr_decode::array(
            logged,
            path,
            json(sources.get(a)?.as_ref(), &format!("{path}/.zarray"))?,
            json(sources.get(a)?.as_ref(), &format!("{path}/.zattrs"))?,
            &r.budget,
        )?;
        *trace.lock().map_err(|_| "trace lock poisoned")? = ReadTrace::default();
        timed.reset();
        let dims = selected
            .dimensions
            .as_ref()
            .ok_or("missing native dimensions")?
            .clone();
        let yi = dims
            .names
            .iter()
            .position(|v| v == &d.axes[1])
            .ok_or("missing latitude/y dimension")?;
        let xi = 3 - yi;
        Ok(Self {
            asset_id: a.id.clone(),
            key_prefix: format!("{path}/"),
            array,
            timed,
            trace,
            dataset: d,
            layout: layout.clone(),
            window: selected.window.clone(),
            dimensions: dims,
            y_index: yi,
            x_index: xi,
        })
    }

    pub(crate) fn execute_node(
        &mut self,
        r: &Request,
        node: &Node,
        report: &mut crate::ExecutionReport,
        sink: &mut impl FnMut(crate::Chunk) -> Result<()>,
    ) -> Result<()> {
        let Operation::Read {
            read: Read::ChunkObject { key, indices },
        } = &node.operation
        else {
            return Ok(());
        };
        if node.asset_id != self.asset_id || !key.starts_with(&self.key_prefix) {
            return Ok(());
        }
        let d = &self.dataset;
        let dims = &self.dimensions;
        let layout = &self.layout;
        let cs = &dims.chunk_shape;
        let yi = self.y_index;
        let xi = self.x_index;
        let w = &self.window;
        let t = Instant::now();
        let idx = indices.iter().map(|v| *v as u64).collect::<Vec<_>>();
        let native = if d.conventional {
            crate::Pixels::Float32(
                self.array
                    .retrieve_chunk_elements_opt::<f32>(&idx, &serial())?,
            )
        } else {
            crate::Pixels::UInt16(
                self.array
                    .retrieve_chunk_elements_opt::<u16>(&idx, &serial())?,
            )
        };
        report.read_decode_seconds += t.elapsed().as_secs_f64();
        if self
            .trace
            .lock()
            .map_err(|_| "trace lock poisoned")?
            .keys
            .get(key)
            .is_some_and(|(_, bytes)| *bytes > 0)
        {
            report.decoded_source_chunks += 1;
        } else {
            report.filled_source_chunks += 1;
        }
        report.native_materialized_bytes =
            Some(report.native_materialized_bytes.unwrap_or(0) + native.byte_len());
        let origin: Vec<_> = indices.iter().zip(cs).map(|(i, c)| i * c).collect();
        let x0 = w.x.max(origin[xi]);
        let x1 = (w.x + w.width).min(origin[xi] + cs[xi]);
        let y0 = w.y.max(origin[yi]);
        let y1 = (w.y + w.height).min(origin[yi] + cs[yi]);
        for ti in dims.indices[0]
            .iter()
            .copied()
            .filter(|t| *t >= origin[0] && *t < origin[0] + cs[0])
        {
            for y in (y0..y1).step_by(r.budget.chunk_edge) {
                for x in (x0..x1).step_by(r.budget.chunk_edge) {
                    let width = (x1 - x).min(r.budget.chunk_edge);
                    let height = (y1 - y).min(r.budget.chunk_edge);
                    let assembly = Instant::now();
                    let alloc = Instant::now();
                    let mut pixels = if d.conventional {
                        crate::Pixels::Float32(vec![f32::NAN; width * height])
                    } else {
                        crate::Pixels::UInt16(vec![0; width * height])
                    };
                    report.allocation_seconds = Some(
                        report.allocation_seconds.unwrap_or(0.) + alloc.elapsed().as_secs_f64(),
                    );
                    let copy = Instant::now();
                    for sy in y..y + height {
                        for sx in x..x + width {
                            let coord = if yi == 1 {
                                [ti - origin[0], sy - origin[yi], sx - origin[xi]]
                            } else {
                                [ti - origin[0], sx - origin[xi], sy - origin[yi]]
                            };
                            let offset = (coord[0] * cs[1] + coord[1]) * cs[2] + coord[2];
                            pixels.copy((sy - y) * width + sx - x, &native, offset, 1)?;
                        }
                    }
                    report.copying_seconds =
                        Some(report.copying_seconds.unwrap_or(0.) + copy.elapsed().as_secs_f64());
                    report.decoded_bytes += pixels.byte_len();
                    report.non_nodata_pixels += (0..pixels.len())
                        .filter(|i| pixels.valid(*i, layout.nodata))
                        .count();
                    let window = Window {
                        x,
                        y,
                        width,
                        height,
                    };
                    let chunk = crate::Chunk {
                        node_id: node.id,
                        asset_id: self.asset_id.clone(),
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
                        coordinates: Some((
                            d.x[x..x + width].to_vec(),
                            d.y[y..y + height].to_vec(),
                        )),
                    };
                    report.assembly_seconds += assembly.elapsed().as_secs_f64();
                    let t = Instant::now();
                    sink(chunk)?;
                    report.sink_seconds += t.elapsed().as_secs_f64();
                    report.chunks += 1;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&self, report: &mut crate::ExecutionReport) -> Result<()> {
        report.source_retrieval_seconds =
            Some(report.source_retrieval_seconds.unwrap_or(0.) + self.timed.read_seconds());
        let trace = self.trace.lock().map_err(|_| "trace lock poisoned")?;
        report.source_data_requests =
            Some(report.source_data_requests.unwrap_or(0) + trace.requests);
        report.source_data_bytes = Some(report.source_data_bytes.unwrap_or(0) + trace.bytes);
        for (key, (requests, bytes)) in &trace.keys {
            report.source_reads.push(crate::execute::SourceRead {
                asset_id: self.asset_id.clone(),
                key: key.clone(),
                requests: *requests,
                bytes: *bytes,
            });
        }
        Ok(())
    }
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
    source: &crate::storage::Objects,
    target: Arc<OutputStore>,
    path: &str,
    shape: Vec<usize>,
    chunks: Vec<usize>,
) -> Result<Array<OutputStore>> {
    let mut m = json(source, &format!("{path}/.zarray"))?;
    // Source storage filters have already been decoded. Do not quantize twice.
    m["filters"] = serde_json::Value::Null;
    m["shape"] = serde_json::json!(shape);
    m["chunks"] = serde_json::json!(chunks);
    // zarrs owns metadata validation, chunk encoding and writing.
    let mut array =
        Array::new_with_metadata(target, &format!("/{path}"), serde_json::from_value(m)?)?;
    let mut attrs = json(source, &format!("{path}/.zattrs"))?;
    if attrs["_ARRAY_DIMENSIONS"]
        .as_array()
        .is_some_and(|v| v.len() == 3)
        && matches!(
            attrs["_ARRAY_DIMENSIONS"][1].as_str(),
            Some("longitude" | "lon")
        )
    {
        attrs["_ARRAY_DIMENSIONS"] = {
            let dims = attrs["_ARRAY_DIMENSIONS"].as_array().unwrap();
            serde_json::json!([dims[0], dims[2], dims[1]])
        };
    }
    *array.attributes_mut() = serde_json::from_value(attrs)?;
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
    fetch_on(r, p, planning, &crate::storage::Sources::default())
}
pub(crate) fn fetch_on(
    r: &Request,
    p: &Plan,
    planning: f64,
    sources: &crate::storage::Sources,
) -> Result<crate::ExecutionReport> {
    let fetch_started = Instant::now();
    let Sink::ZarrDirectory { path } = &r.sink else {
        return fail("Zarr requires a native Zarr directory sink");
    };
    let root = Path::new(path);
    if p.selected.iter().any(|s| s.dimensions.is_none()) {
        return fail("Zarr directory sink requires supported Zarr group assets");
    }
    // FilesystemStore retains one lock entry per visited object. Bound that
    // bookkeeping as well as decoded payloads, before creating any output.
    let mut objects = 1usize; // completion manifest
    let mut groups = std::collections::HashSet::new();
    for selected in &p.selected {
        if groups.insert(&selected.asset_id) {
            objects = objects
                .checked_add(11)
                .ok_or("output object count overflow")?;
        }
        let w = &selected.window;
        let times = selected
            .dimensions
            .as_ref()
            .ok_or("missing dimensions")?
            .indices[0]
            .len();
        let chunks = times
            .checked_mul(w.width.div_ceil(r.budget.chunk_edge))
            .and_then(|n| n.checked_mul(w.height.div_ceil(r.budget.chunk_edge)))
            .and_then(|n| n.checked_add(2))
            .ok_or("output object count overflow")?;
        objects = objects
            .checked_add(chunks)
            .ok_or("output object count overflow")?;
        if objects > r.budget.max_nodes {
            return fail(
                "output object bookkeeping exceeds max_nodes; increase budget or chunk_edge",
            );
        }
    }
    fs::create_dir(root)?;
    let result = (|| -> Result<crate::ExecutionReport> {
        let Source::Assets { assets } = &r.source else {
            unreachable!()
        };
        let setup = Instant::now();
        let mut stores = Vec::new();
        let mut outputs = std::collections::HashMap::new();
        let mut original_metadata = serde_json::Map::new();
        for a in assets {
            let selected: Vec<_> = p.selected.iter().filter(|s| s.asset_id == a.id).collect();
            if selected.is_empty() {
                continue;
            }
            let source_store = sources.get(a)?;
            let d = inspect_on(r, source_store.clone())?;
            let s = Arc::new(OutputStore::new(Arc::new(Metrics::new(Arc::new(
                Store::new(root.join(format!("{}.zarr", a.id)))?,
            )))));
            stores.push(s.clone());
            let w = &selected[0].window;
            let indices = &selected[0].dimensions.as_ref().unwrap().indices[0];
            let mut group = zarrs::group::Group::new_with_metadata(
                s.clone(),
                "/",
                serde_json::from_value(serde_json::json!({"zarr_format":2}))?,
            )?;
            let mut source_arrays = serde_json::Map::new();
            for sel in &selected {
                if let Variable::Dataset { path } = &sel.layout.as_ref().unwrap().variable {
                    let n = name(path)?;
                    source_arrays.insert(n.into(),serde_json::json!({"metadata":json(&source_store,&format!("{n}/.zarray"))?,"attributes":json(&source_store,&format!("{n}/.zattrs"))?}));
                }
            }
            original_metadata.insert(
                a.id.clone(),
                serde_json::json!({"group_attributes":d.root_attrs,"arrays":source_arrays}),
            );
            let mut attrs = d.root_attrs;
            if d.conventional {
                for (axis, values, start, count) in
                    [("lon", &d.x, w.x, w.width), ("lat", &d.y, w.y, w.height)]
                {
                    let subset = &values[start..start + count];
                    for (suffix, value) in [
                        ("min", subset.iter().copied().fold(f64::INFINITY, f64::min)),
                        (
                            "max",
                            subset.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                        ),
                    ] {
                        let key = format!("geospatial_{axis}_{suffix}");
                        if attrs.get(&key).is_some() {
                            attrs[&key] = serde_json::json!(value);
                        }
                    }
                }
                if attrs.get("geospatial_bounds").is_some() {
                    let edge = |v: &[f64], i: usize| {
                        if i == 0 {
                            v[0] - (v[1] - v[0]) / 2.
                        } else if i == v.len() {
                            v[i - 1] + (v[i - 1] - v[i - 2]) / 2.
                        } else {
                            v[i - 1] / 2. + v[i] / 2.
                        }
                    };
                    let (xa, xb, ya, yb) = (
                        edge(&d.x, w.x),
                        edge(&d.x, w.x + w.width),
                        edge(&d.y, w.y),
                        edge(&d.y, w.y + w.height),
                    );
                    let (lo, hi, la, ha) = (xa.min(xb), xa.max(xb), ya.min(yb), ya.max(yb));
                    attrs["geospatial_bounds"]=serde_json::json!(format!("POLYGON(({lo:.10} {ha:.10}, {lo:.10} {la:.10}, {hi:.10} {la:.10}, {hi:.10} {ha:.10}, {lo:.10} {ha:.10}))"));
                }
                for (key, ti) in [
                    ("time_coverage_start", indices[0]),
                    ("time_coverage_end", *indices.last().unwrap()),
                ] {
                    if attrs.get(key).is_some() {
                        attrs[key] =
                            serde_json::json!(chrono::DateTime::from_timestamp(d.times[ti], 0)
                                .unwrap()
                                .to_rfc3339());
                    }
                }
                attrs
                    .as_object_mut()
                    .unwrap()
                    .remove("time_coverage_duration");
            }
            if !d.conventional {
                attrs["tile_ripper_grid"]["transform"] =
                    serde_json::json!(crate::spatial::window_transform(&d.layout, w));
            }
            *group.attributes_mut() = serde_json::from_value(attrs)?;
            group.store_metadata()?;
            let source = source_store.as_ref();
            for (axis, values) in [
                (d.axes[0].as_str(), &d.x[w.x..w.x + w.width]),
                (d.axes[1].as_str(), &d.y[w.y..w.y + w.height]),
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
            let times: Vec<_> = indices.iter().map(|i| d.raw_times[*i]).collect();
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
                    (Arc::new(ar), w.clone(), indices.clone()),
                );
            }
        }
        let output_setup = setup.elapsed().as_secs_f64();
        let mut buffer = OutputBuffer::new(r.budget.buffer_bytes / 4);
        buffer.started = fetch_started;
        let mut report = crate::execute::execute_on(r, p, planning, sources, |c| {
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
            buffer.write(
                ar,
                &c.asset_id,
                path,
                &ArraySubset::new_with_start_shape(
                    vec![
                        ti as u64,
                        (c.window.y - w.y) as u64,
                        (c.window.x - w.x) as u64,
                    ],
                    vec![1, c.window.height as u64, c.window.width as u64],
                )?,
                &c.pixels,
            )?;
            Ok(())
        })?;
        sources.phase("output_finalization");
        let finalize = Instant::now();
        buffer.flush()?;
        report.output_finalize_seconds = finalize.elapsed().as_secs_f64();
        report.first_output_seconds = buffer.first_output;
        report.output_setup_seconds = output_setup;
        report.output_io = Some(crate::execute::OutputIo {
            reads: stores.iter().map(|s| s.inner.reads()).sum(),
            writes: stores.iter().map(|s| s.inner.writes()).sum(),
            bytes_read: stores.iter().map(|s| s.inner.bytes_read()).sum(),
            bytes_written: stores.iter().map(|s| s.inner.bytes_written()).sum(),
            buffered_bytes_peak: buffer.peak,
            storage_read_seconds: stores.iter().map(|s| s.read_seconds()).sum(),
            storage_write_seconds: stores.iter().map(|s| s.write_seconds()).sum(),
            encode_seconds: buffer.encode_seconds,
            allocation_seconds: buffer.allocation_seconds,
            copying_seconds: buffer.copying_seconds,
        });
        let manifest = Instant::now();
        let bytes = serde_json::to_vec_pretty(
            &serde_json::json!({"plan":p,"observed":report,"sources":assets,"source_metadata":original_metadata,"decisions":"source storage filters decoded once; output has no repeat quantization; subset geospatial/time extent fields recalculated; original metadata retained; native logical values; no calibration; missing chunks fill; coordinates subset in source order; local sources must remain immutable during operation"}),
        )?;
        crate::diagnostics::profile_stage(
            "native_manifest_serialization",
            manifest.elapsed().as_secs_f64(),
        );
        let manifest = Instant::now();
        fs::write(root.join("manifest.json"), bytes)?;
        crate::diagnostics::profile_stage(
            "native_manifest_write_close",
            manifest.elapsed().as_secs_f64(),
        );
        Ok(report)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(root);
    }
    result
}

/// Adapter-local, byte-bounded LRU of decoded destination chunks. Evictions are
/// encoded/synchronised by zarrs; a later revisit reads that destination chunk.
/// Source traversal, selection and the shared executor remain unchanged.
struct OutputBuffer {
    limit: usize,
    peak: usize,
    clock: u64,
    started: Instant,
    first_output: Option<f64>,
    encode_seconds: f64,
    allocation_seconds: f64,
    copying_seconds: f64,
    active: Option<(String, String, Arc<Array<OutputStore>>)>,
    ages: std::collections::HashMap<[u64; 3], u64>,
    entries: std::collections::BTreeMap<u64, ([u64; 3], crate::Pixels)>,
}
impl OutputBuffer {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            peak: 0,
            clock: 0,
            started: Instant::now(),
            first_output: None,
            encode_seconds: 0.,
            allocation_seconds: 0.,
            copying_seconds: 0.,
            active: None,
            ages: Default::default(),
            entries: Default::default(),
        }
    }
    fn evict(&mut self) -> Result<()> {
        if let Some((_, (key, values))) = self.entries.pop_first() {
            self.ages.remove(&key);
            let ar = &self.active.as_ref().ok_or("output buffer missing array")?.2;
            let before = ar.storage().write_seconds();
            let t = Instant::now();
            match &values {
                crate::Pixels::UInt16(v) => ar.store_chunk_elements_opt(&key, v, &serial())?,
                crate::Pixels::Float32(v) => ar.store_chunk_elements_opt(&key, v, &serial())?,
            };
            if self.first_output.is_none() {
                self.first_output = Some(self.started.elapsed().as_secs_f64());
            }
            self.encode_seconds +=
                (t.elapsed().as_secs_f64() - (ar.storage().write_seconds() - before)).max(0.);
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        while !self.entries.is_empty() {
            self.evict()?;
        }
        Ok(())
    }
    fn write(
        &mut self,
        ar: &Arc<Array<OutputStore>>,
        asset: &str,
        variable: &str,
        subset: &ArraySubset,
        pixels: &crate::Pixels,
    ) -> Result<()> {
        if self
            .active
            .as_ref()
            .is_none_or(|(a, v, _)| a != asset || v != variable)
        {
            self.flush()?;
            self.active = Some((asset.into(), variable.into(), ar.clone()));
        }
        let cs = ar.chunk_shape_usize(&[0, 0, 0])?;
        let float = ar.data_type() == &zarrs::array::DataType::Float32;
        let bytes = cs.iter().product::<usize>() * if float { 4 } else { 2 };
        if bytes > self.limit {
            return fail("destination chunk exceeds output buffer budget");
        }
        let capacity = self.limit / bytes;
        let start = subset.start();
        let shape = subset.shape();
        let (y0, x0) = (start[1] as usize, start[2] as usize);
        let (h, w) = (shape[1] as usize, shape[2] as usize);
        for cy in y0 / cs[1]..=(y0 + h - 1) / cs[1] {
            for cx in x0 / cs[2]..=(x0 + w - 1) / cs[2] {
                let key = [start[0], cy as u64, cx as u64];
                let mut values = if let Some(age) = self.ages.remove(&key) {
                    self.entries
                        .remove(&age)
                        .ok_or("output buffer index missing")?
                        .1
                } else {
                    if self.entries.len() == capacity {
                        self.evict()?;
                    }
                    let t = Instant::now();
                    let values = if float {
                        crate::Pixels::Float32(
                            ar.retrieve_chunk_elements_opt::<f32>(&key, &serial())?,
                        )
                    } else {
                        crate::Pixels::UInt16(
                            ar.retrieve_chunk_elements_opt::<u16>(&key, &serial())?,
                        )
                    };
                    self.allocation_seconds += t.elapsed().as_secs_f64();
                    values
                };
                let sy = y0.max(cy * cs[1]);
                let ey = (y0 + h).min((cy + 1) * cs[1]);
                let sx = x0.max(cx * cs[2]);
                let ex = (x0 + w).min((cx + 1) * cs[2]);
                let copying = Instant::now();
                for y in sy..ey {
                    let src = (y - y0) * w + sx - x0;
                    let dst = (y - cy * cs[1]) * cs[2] + sx - cx * cs[2];
                    values.copy(dst, pixels, src, ex - sx)?;
                }
                self.copying_seconds += copying.elapsed().as_secs_f64();
                self.clock += 1;
                self.ages.insert(key, self.clock);
                self.entries.insert(self.clock, (key, values));
                self.peak = self.peak.max(self.entries.len() * bytes);
            }
        }
        Ok(())
    }
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
