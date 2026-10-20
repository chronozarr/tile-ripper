//! COG chunks -> ordinary per-observation Zarr v2 groups. zarrs owns encoding.
//! The destination name is published only after successful stream completion.
use crate::{acquisition::Prepared, execute::OutputIo, model::*, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
use zarrs::{
    array::{Array, ArrayMetadataOptions},
    array_subset::ArraySubset,
    filesystem::FilesystemStore,
    group::Group,
    storage::{
        storage_adapter::performance_metrics::PerformanceMetricsStorageAdapter, StoreKey,
        WritableStorageTraits,
    },
};
type Metrics = PerformanceMetricsStorageAdapter<FilesystemStore>;
type Store = crate::zarr_io::Timed<Metrics>;

fn selections(plan: &Plan) -> Vec<&Selected> {
    let mut selected: Vec<_> = plan.selected.iter().collect();
    selected.sort_by_key(|s| {
        (
            s.datetime
                .as_ref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()),
            s.asset_id.clone(),
            match s.layout.as_ref().map(|l| &l.variable) {
                Some(Variable::Band { index }) => *index,
                _ => 0,
            },
        )
    });
    selected
}
pub(crate) fn outputs(plan: &Plan) -> Value {
    json!(selections(plan).iter().enumerate().map(|(i, s)| {
        let l = s.layout.as_ref();
        json!({"file":".","group":format!("observations/{i:06}"),"dataset":"values",
            "asset_id":s.asset_id,"variable":l.map(|l|&l.variable),"datetime":s.datetime,
            "timestamps":s.timestamps,"window":s.window,"dtype":"uint16","native_dtype":"<u2",
            "nodata":l.and_then(|l|l.nodata),"scale":l.and_then(|l|l.scale),"offset":l.and_then(|l|l.offset),
            "native_grid":l.map(|l|json!({"crs":l.crs,"transform":crate::spatial::window_transform(l,&s.window)})),
            "dimensions":{"names":["time","y","x"],"shape":[1,s.window.height,s.window.width]},
            "aoi_mask_applied":s.mask_geometry.is_some(),"validity":"valid = aoi_mask AND values != source nodata; 0 invalid, 1 valid"})
    }).collect::<Vec<_>>())
}
fn array(
    store: Arc<Store>,
    path: &str,
    shape: &[usize],
    chunks: &[usize],
    dtype: &str,
    fill: Value,
    attrs: Value,
) -> Result<Array<Store>> {
    let metadata = json!({"zarr_format":2,"shape":shape,"chunks":chunks,"dtype":dtype,
        "order":"C","filters":null,"compressor":null,"fill_value":fill});
    let mut a = Array::new_with_metadata(store, path, serde_json::from_value(metadata)?)?;
    *a.attributes_mut() = serde_json::from_value(attrs)?;
    a.store_metadata_opt(&ArrayMetadataOptions::default().with_include_zarrs_metadata(false))?;
    Ok(a)
}
fn group(store: Arc<Store>, path: &str, attrs: Value) -> Result<()> {
    let mut g = Group::new_with_metadata(
        store,
        path,
        serde_json::from_value(json!({"zarr_format":2}))?,
    )?;
    *g.attributes_mut() = serde_json::from_value(attrs)?;
    g.store_metadata()?;
    Ok(())
}
fn serial() -> zarrs::array::codec::CodecOptions {
    let mut options = zarrs::array::codec::CodecOptions::default();
    options.set_concurrent_target(1);
    options
}
fn check_cancelled(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::Acquire) {
        return Err("acquisition cancelled; output was not published".into());
    }
    Ok(())
}
fn crs_attributes(crs: &str, transform: [f64; 6]) -> Value {
    let mut attrs = json!({"_ARRAY_DIMENSIONS":[],"crs":crs,"GeoTransform":transform.iter().map(f64::to_string).collect::<Vec<_>>().join(" ")});
    // CF parameters encode the supported WGS84 UTM CRS without a PROJ database.
    if let Some(code) = crs
        .strip_prefix("EPSG:")
        .and_then(|n| n.parse::<u32>().ok())
    {
        attrs["epsg_code"] = json!(crs);
        if (32601..=32660).contains(&code) || (32701..=32760).contains(&code) {
            attrs["grid_mapping_name"] = json!("transverse_mercator");
            attrs["longitude_of_central_meridian"] = json!((code % 100) as i32 * 6 - 183);
            attrs["latitude_of_projection_origin"] = json!(0.);
            attrs["scale_factor_at_central_meridian"] = json!(0.9996);
            attrs["false_easting"] = json!(500000.);
            attrs["false_northing"] = json!(if code >= 32701 { 10000000. } else { 0. });
            attrs["projected_crs_name"] = json!(format!(
                "WGS 84 / UTM zone {}{}",
                code % 100,
                if code >= 32701 { "S" } else { "N" }
            ));
        } else if code == 4326 {
            attrs["grid_mapping_name"] = json!("latitude_longitude");
        }
        if attrs.get("grid_mapping_name").is_some() {
            attrs["semi_major_axis"] = json!(6378137.);
            attrs["inverse_flattening"] = json!(298.257223563);
            attrs["reference_ellipsoid_name"] = json!("WGS 84");
            attrs["prime_meridian_name"] = json!("Greenwich");
            attrs["longitude_of_prime_meridian"] = json!(0.);
            attrs["geographic_crs_name"] = json!("WGS 84");
            attrs["horizontal_datum_name"] = json!("World Geodetic System 1984 ensemble");
        }
    } else if crs.starts_with("PROJCS[")
        || crs.starts_with("GEOGCS[")
        || crs.starts_with("PROJCRS[")
        || crs.starts_with("GEOGCRS[")
    {
        attrs["crs_wkt"] = json!(crs);
    }
    attrs
}
struct Observation {
    selected: Selected,
    values: Array<Store>,
    valid: Array<Store>,
    aoi: Array<Store>,
    geometry: Option<crate::spatial::Geometry>,
    chunk_shape: [usize; 2],
}

pub(crate) fn fetch(prepared: Prepared, path: &str, cancelled: Arc<AtomicBool>) -> Result<Value> {
    let started = Instant::now();
    let inspection = prepared.explain();
    let budget: Budget = serde_json::from_value(inspection["output_budget"].clone())?;
    if budget.buffer_bytes < 16 {
        return Err("scientific output needs at least 16 buffer_bytes".into());
    }
    let coordinate_edge = budget.chunk_edge.min(budget.buffer_bytes / 16);
    let plan = prepared.plan();
    let selected = selections(plan);
    let mut objects = 5usize; // two groups plus manifest, each group has two objects
    if objects > budget.max_nodes {
        return Err("output object bookkeeping exceeds max_nodes; even empty output needs five metadata objects".into());
    }
    let mut expected = HashMap::new();
    for (i, s) in selected.iter().enumerate() {
        let l = s
            .layout
            .as_ref()
            .ok_or("COG output needs a native layout")?;
        if s.dimensions.is_some()
            || !matches!(l.variable, Variable::Band { .. })
            || !["UInt16", "<u2"].contains(&l.dtype.as_str())
        {
            return Err("scientific COG output currently requires UInt16 native bands".into());
        }
        if l.transform.iter().any(|v| !v.is_finite())
            || l.transform[2] != 0.
            || l.transform[4] != 0.
        {
            return Err("rectilinear Zarr output requires an unrotated native grid; no resampling is performed".into());
        }
        if let Some(n) = l.nodata {
            if !n.is_finite() || n.fract() != 0. || !(0. ..=65535.).contains(&n) {
                return Err("COG nodata is not representable as UInt16".into());
            }
        } else if s.mask_geometry.is_some() {
            return Err("AOI output requires representable source nodata".into());
        }
        let edge_y = budget.chunk_edge.min(s.window.height);
        let edge_x = budget.chunk_edge.min(s.window.width);
        let bytes = edge_y
            .checked_mul(edge_x)
            .and_then(|n| n.checked_mul(2))
            .ok_or("output chunk size overflow")?;
        if bytes > budget.buffer_bytes / 4 {
            return Err("destination chunk exceeds one quarter of buffer_bytes; reduce chunk_edge or raise buffer budget".into());
        }
        for n in &plan.nodes {
            if n.asset_id == s.asset_id {
                if let Operation::Read {
                    read: Read::LibraryWindow { band, window, .. },
                } = &n.operation
                {
                    if l.variable == (Variable::Band { index: *band }) {
                        expected.insert(n.id, (i, window.clone()));
                    }
                }
            }
        }
        // Each observation: group, seven arrays, scalar/time data, coordinates,
        // and three spatial arrays. Include skipped fill chunks conservatively.
        let tiles = s
            .window
            .width
            .div_ceil(edge_x)
            .checked_mul(s.window.height.div_ceil(edge_y))
            .ok_or("output object count overflow")?;
        objects = objects
            .checked_add(
                18 + s.window.width.div_ceil(coordinate_edge)
                    + s.window.height.div_ceil(coordinate_edge),
            )
            .and_then(|n| n.checked_add(tiles.checked_mul(3)?))
            .ok_or("output object count overflow")?;
        if objects > budget.max_nodes {
            return Err(
                "output object bookkeeping exceeds max_nodes; raise the limit or chunk_edge".into(),
            );
        }
    }
    check_cancelled(&cancelled)?;
    let transaction = Transaction::new(Path::new(path))?;
    let metrics = Arc::new(Metrics::new(Arc::new(FilesystemStore::new(
        &transaction.stage,
    )?)));
    let store = Arc::new(Store::new(metrics.clone()));
    let output_records = outputs(plan);
    let root_attrs = json!({"title":"Native source observations","status":"incomplete",
        "layout":"one source asset/band per observation group, chronological order; no aligned cube",
        "observations":output_records,"logical_request":inspection["logical_request"],
        "sources":inspection["sources"],"query":inspection["query"],
        "processing":"native stored values; no calibration, reprojection, resampling or mosaic",
        "software":{"name":"tile-ripper","version":env!("CARGO_PKG_VERSION"),"writer":"zarrs 0.22.10","zarr_format":2}});
    group(store.clone(), "/", root_attrs.clone())?;
    group(store.clone(), "/observations", json!({}))?;
    let mut observations = Vec::new();
    let mut writer_peak = 0;
    for (i, s) in selected.iter().enumerate() {
        check_cancelled(&cancelled)?;
        let l = s.layout.as_ref().unwrap();
        let base = format!("/observations/{i:06}");
        let source = inspection["sources"]
            .as_array()
            .and_then(|a| a.iter().find(|v| v["asset_id"] == s.asset_id))
            .cloned()
            .unwrap_or(Value::Null);
        let t = crate::spatial::window_transform(l, &s.window);
        let timestamp = s
            .datetime
            .as_deref()
            .map(chrono::DateTime::parse_from_rfc3339)
            .transpose()?;
        // Undated direct COGs retain a missing time coordinate, never a fake date.
        let time_value = timestamp
            .map(|t| {
                t.timestamp_nanos_opt()
                    .ok_or("timestamp outside nanosecond time range")
            })
            .transpose()?
            .unwrap_or(i64::MIN);
        group(
            store.clone(),
            &base,
            json!({"asset_id":s.asset_id,"variable":l.variable,"datetime":s.datetime,
            "source":source,"source_window":s.window,"source_grid":l,"transform":t,"crs":l.crs,
            "source_nodata":l.nodata,"source_scale":l.scale,"source_offset":l.offset,
            "aoi_mask_applied":s.mask_geometry.is_some(),"time_missing":s.datetime.is_none()}),
        )?;
        for (axis, len, origin, step) in [
            ("x", s.window.width, t[0], t[1]),
            ("y", s.window.height, t[3], t[5]),
        ] {
            let mut attrs = json!({"_ARRAY_DIMENSIONS":[axis],"axis":axis.to_uppercase(),"long_name":format!("native {axis} pixel center")});
            if l.crs == "EPSG:4326" {
                attrs["standard_name"] = json!(if axis == "x" { "longitude" } else { "latitude" });
                attrs["units"] = json!(if axis == "x" {
                    "degrees_east"
                } else {
                    "degrees_north"
                });
            } else if crs_attributes(&l.crs, t).get("grid_mapping_name").is_some() {
                attrs["standard_name"] = json!(if axis == "x" {
                    "projection_x_coordinate"
                } else {
                    "projection_y_coordinate"
                });
                attrs["units"] = json!("m");
            }
            let edge = len.min(coordinate_edge);
            let a = array(
                store.clone(),
                &format!("{base}/{axis}"),
                &[len],
                &[edge],
                "<f8",
                Value::Null,
                attrs,
            )?;
            for start in (0..len).step_by(edge) {
                check_cancelled(&cancelled)?;
                let values: Vec<_> = (start..(start + edge).min(len))
                    .map(|j| origin + (j as f64 + 0.5) * step)
                    .collect();
                writer_peak = writer_peak.max(edge * 16);
                a.store_array_subset_elements_opt(
                    &ArraySubset::new_with_start_shape(
                        vec![start as u64],
                        vec![values.len() as u64],
                    )?,
                    &values,
                    &serial(),
                )?;
            }
        }
        let time = array(
            store.clone(),
            &format!("{base}/time"),
            &[1],
            &[1],
            "<i8",
            Value::Null,
            json!({"_ARRAY_DIMENSIONS":["time"],"standard_name":"time","units":"nanoseconds since 1970-01-01 00:00:00","calendar":"proleptic_gregorian"}),
        )?;
        time.store_chunk_elements_opt(&[0], &[time_value], &serial())?;
        let crs = array(
            store.clone(),
            &format!("{base}/spatial_ref"),
            &[],
            &[],
            "<i4",
            Value::Null,
            crs_attributes(&l.crs, t),
        )?;
        crs.store_chunk_elements_opt(&[], &[0i32], &serial())?;
        let shape = [1, s.window.height, s.window.width];
        let chunks = [
            1,
            budget.chunk_edge.min(s.window.height),
            budget.chunk_edge.min(s.window.width),
        ];
        let mut attrs = json!({"_ARRAY_DIMENSIONS":["time","y","x"],"grid_mapping":"spatial_ref",
            "ancillary_variables":"valid aoi_mask","source_nodata":l.nodata,
            "long_name":source["requested_band"].as_str().unwrap_or("native stored pixel value")});
        let stac = &source["raster_bands"][0];
        let scale = l.scale.or_else(|| stac["scale"].as_f64());
        let offset = l.offset.or_else(|| stac["offset"].as_f64());
        if let Some(v) = scale {
            attrs["scale_factor"] = json!(v);
        }
        if let Some(v) = offset {
            attrs["add_offset"] = json!(v);
        }
        attrs["calibration_precedence"] = json!("TIFF metadata, then STAC when absent; original fields retained in source provenance; stored values unmodified");
        let values = array(
            store.clone(),
            &format!("{base}/values"),
            &shape,
            &chunks,
            "<u2",
            json!(l.nodata.map(|n| n as u16)),
            attrs,
        )?;
        let mask_attrs = |name: &str| json!({"_ARRAY_DIMENSIONS":["time","y","x"],"grid_mapping":"spatial_ref","long_name":name,"flag_values":[0,1],"flag_meanings":"excluded included"});
        let valid = array(
            store.clone(),
            &format!("{base}/valid"),
            &shape,
            &chunks,
            "|u1",
            json!(0),
            mask_attrs("AOI included and not source nodata"),
        )?;
        let aoi = array(
            store.clone(),
            &format!("{base}/aoi_mask"),
            &shape,
            &chunks,
            "|u1",
            json!(0),
            mask_attrs("AOI pixel center inclusion; source nodata may still be included"),
        )?;
        observations.push(Observation {
            selected: (*s).clone(),
            values,
            valid,
            aoi,
            geometry: s
                .mask_geometry
                .as_ref()
                .map(|g| crate::spatial::from_geojson(g, &l.crs))
                .transpose()?,
            chunk_shape: [chunks[1], chunks[2]],
        });
    }
    let setup_seconds = started.elapsed().as_secs_f64();
    let mut stream = crate::stream::ChunkStream::start_cancellable(
        prepared,
        crate::stream::StreamOptions { capacity: 0 },
        cancelled.clone(),
    )?;
    let mut written = HashSet::new();
    let mut first_output = None;
    let mut sink_seconds = 0.;
    for item in stream.by_ref() {
        check_cancelled(&cancelled)?;
        let c = item?;
        let write_start = Instant::now();
        let (i, window) = expected
            .get(&c.node_id)
            .ok_or("unplanned COG output chunk")?;
        let o = &observations[*i];
        let l = o.selected.layout.as_ref().unwrap();
        if !written.insert(c.node_id)
            || c.window != *window
            || c.asset_id != o.selected.asset_id
            || c.variable != l.variable
            || c.crs != l.crs
            || c.nodata != l.nodata
            || c.scale != l.scale
            || c.offset != l.offset
            || c.datetime != o.selected.datetime
            || c.coordinates.is_some()
            || c.transform != crate::spatial::window_transform(l, window)
        {
            return Err("stream chunk differs from prepared observation metadata".into());
        }
        let values = c.pixels.as_u16().ok_or("COG output expected UInt16")?;
        if values.len() != window.width * window.height {
            return Err("invalid COG output buffer dimensions".into());
        }
        let subset = ArraySubset::new_with_start_shape(
            vec![
                0,
                (window.y - o.selected.window.y) as u64,
                (window.x - o.selected.window.x) as u64,
            ],
            vec![1, window.height as u64, window.width as u64],
        )?;
        o.values
            .store_array_subset_elements_opt(&subset, values, &serial())?;
        let valid: Vec<u8> = c.validity().map(u8::from).collect();
        o.valid
            .store_array_subset_elements_opt(&subset, &valid, &serial())?;
        let mask = o
            .geometry
            .as_ref()
            .map(|g| crate::spatial::mask(g, l, window))
            .transpose()?;
        let aoi = mask
            .as_ref()
            .map(|m| m.data().to_vec())
            .unwrap_or_else(|| vec![1; values.len()]);
        o.aoi
            .store_array_subset_elements_opt(&subset, &aoi, &serial())?;
        // One delivered payload, two byte masks and bounded library encode/pad
        // scratch. This is a conservative explicit-buffer count, not an RSS cap.
        writer_peak = writer_peak.max(o.chunk_shape.iter().product::<usize>() * 8);
        first_output.get_or_insert_with(|| started.elapsed().as_secs_f64());
        sink_seconds += write_start.elapsed().as_secs_f64();
    }
    let mut report = stream.finish()?;
    if written.len() != expected.len() || report.chunks != written.len() {
        return Err("incomplete COG output coverage".into());
    }
    check_cancelled(&cancelled)?;
    let finalize = Instant::now();
    let mut attrs = root_attrs;
    attrs["status"] = json!("complete");
    group(store.clone(), "/", attrs)?;
    report.output_setup_seconds = setup_seconds;
    report.first_output_seconds = first_output;
    report.sink_seconds = sink_seconds;
    report.output_io = Some(OutputIo {
        reads: metrics.reads(),
        writes: metrics.writes(),
        bytes_read: metrics.bytes_read(),
        bytes_written: metrics.bytes_written(),
        buffered_bytes_peak: writer_peak,
        storage_read_seconds: store.read_seconds(),
        storage_write_seconds: store.write_seconds(),
        ..Default::default()
    });
    report.output_finalize_seconds = finalize.elapsed().as_secs_f64();
    let manifest = json!({"schema_version":3,"document_type":"acquisition_result","status":"complete",
        "format":"zarr_v2","writer":"zarrs 0.22.10","sources":inspection["sources"],"outputs":output_records,
        "inspection":inspection,"observed":report,"processing":["native stored UInt16 values; calibration is metadata only","one source asset/band and native grid per observation; chronological order","no reprojection, resampling, mosaicking or implicit alignment","valid and aoi_mask distinguish source nodata from excluded AOI centers"],
        "completion":{"stream_finish":"successful","publication":"atomic no-replace sibling rename","existing_destination":"refused; replacement is never implicit","scope":"validated provider ETags or local size/mtime, not an atomic multi-object snapshot"}});
    // Metrics above exclude this manifest (like the existing output adapter).
    store.set(
        &StoreKey::new("manifest.json")?,
        serde_json::to_vec_pretty(&manifest)?.into(),
    )?;
    drop(observations);
    drop(store);
    drop(metrics);
    transaction.publish(&cancelled)?;
    Ok(manifest)
}

struct Transaction {
    stage: PathBuf,
    destination: PathBuf,
}
impl Transaction {
    fn new(destination: &Path) -> Result<Self> {
        // Resolve parent aliases once. Refuse files, directories and dangling links.
        if destination.symlink_metadata().is_ok() {
            return Err("output destination already exists; choose a new directory".into());
        }
        let name = destination
            .file_name()
            .ok_or("output needs a new directory name")?;
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let parent = fs::canonicalize(parent)?;
        let destination = parent.join(name);
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let stage = parent.join(format!(
                ".{}.partial-{}-{}",
                name.to_string_lossy(),
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&stage) {
                Ok(()) => return Ok(Self { stage, destination }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err("cannot reserve a unique staging directory".into())
    }
    fn publish(self, cancelled: &AtomicBool) -> Result<()> {
        // zarrs synchronises every object. Persist the directory entries too.
        fn sync_dirs(path: &Path) -> Result<()> {
            for e in fs::read_dir(path)? {
                let p = e?.path();
                if p.is_dir() {
                    sync_dirs(&p)?;
                }
            }
            fs::File::open(path)?.sync_all()?;
            Ok(())
        }
        sync_dirs(&self.stage)?;
        check_cancelled(cancelled)?;
        rename_new(&self.stage, &self.destination)?;
        // Publication is the commit point. A crash before parent fsync can lose
        // the name on some filesystems; it cannot expose a partial acquisition.
        if let Some(parent) = self.destination.parent() {
            let _ = fs::File::open(parent).and_then(|f| f.sync_all());
        }
        Ok(())
    }
}
impl Drop for Transaction {
    fn drop(&mut self) {
        if self.stage.exists() {
            if let Err(e) = fs::remove_dir_all(&self.stage) {
                eprintln!(
                    "tile-ripper: incomplete staging directory retained at {}: {e}",
                    self.stage.display()
                );
            }
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_new(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    // Both system calls atomically fail when *any* destination already exists.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_new(_: &Path, _: &Path) -> Result<()> {
    Err("atomic no-replace scientific output is currently supported on Linux and macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "tile-ripper-output-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        p
    }
    #[test]
    fn destination_race_never_replaces_an_existing_directory() {
        let r = root();
        let dest = r.join("result.zarr");
        let tx = Transaction::new(&dest).unwrap();
        fs::write(tx.stage.join("provisional"), b"data").unwrap();
        fs::create_dir(&dest).unwrap(); // even empty destinations must survive
        assert!(tx.publish(&AtomicBool::new(false)).is_err());
        assert!(dest.is_dir());
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&r).unwrap().count(), 1);
        fs::remove_dir_all(r).unwrap();
    }
    #[test]
    fn failure_and_cancellation_remove_only_owned_staging() {
        let r = root();
        let dest = r.join("result.zarr");
        let abandoned = r.join(".result.zarr.partial-interrupted");
        fs::create_dir(&abandoned).unwrap();
        let tx = Transaction::new(&dest).unwrap();
        fs::write(tx.stage.join("provisional"), b"data").unwrap();
        assert!(tx.publish(&AtomicBool::new(true)).is_err());
        assert!(!dest.exists());
        assert!(abandoned.is_dir());
        let tx = Transaction::new(&dest).unwrap();
        drop(tx);
        assert_eq!(fs::read_dir(&r).unwrap().count(), 1);
        fs::remove_dir_all(r).unwrap();
    }
}
