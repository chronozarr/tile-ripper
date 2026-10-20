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
    array::ArraySubset,
    array::{Array, ArrayMetadataOptions},
    group::Group,
    storage::{
        storage_adapter::performance_metrics::PerformanceMetricsStorageAdapter, StoreKey,
        WritableStorageTraits,
    },
};
use zarrs_filesystem::{FilesystemStore, FilesystemStoreOptions};
type Metrics = PerformanceMetricsStorageAdapter<FilesystemStore>;
type Store = crate::zarr_io::Timed<Metrics>;

/// Lossless scientific-output candidates. The ordinary fetch stays Zarr v2,
/// uncompressed. V3 is deliberately gated at build time.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScientificEncoding {
    Uncompressed,
    Gzip,
    BloscZstd,
    #[cfg(feature = "experimental-output")]
    ExperimentalV3ShardedGzip,
}
impl ScientificEncoding {
    fn version(self) -> u8 {
        match self {
            #[cfg(feature = "experimental-output")]
            Self::ExperimentalV3ShardedGzip => 3,
            _ => 2,
        }
    }
    fn compressor(self) -> Value {
        match self {
            Self::Uncompressed => Value::Null,
            Self::Gzip => json!({"id":"gzip","level":1}),
            Self::BloscZstd => {
                json!({"id":"blosc","cname":"zstd","clevel":1,"shuffle":1,"blocksize":0})
            }
            #[cfg(feature = "experimental-output")]
            Self::ExperimentalV3ShardedGzip => Value::Null,
        }
    }
}
/// Output buffers are bounded by this edge, independently of source deliveries.
/// Each encode call releases its scratch before the next; no scene/shard cache
/// is retained across calls or observations. See the output_io reservation.
#[derive(Clone, Copy, Debug)]
pub struct ScientificOutputOptions {
    pub chunk_edge: usize,
    pub encoding: ScientificEncoding,
}

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
#[allow(clippy::too_many_arguments)]
fn array(
    store: Arc<Store>,
    path: &str,
    shape: &[usize],
    chunks: &[usize],
    dtype: &str,
    fill: Value,
    attrs: Value,
    encoding: ScientificEncoding,
) -> Result<Array<Store>> {
    let compressor = if shape.len() == 3 {
        encoding.compressor()
    } else {
        Value::Null
    };
    #[allow(unused_mut)]
    let mut metadata = json!({"zarr_format":2,"shape":shape,"chunks":chunks,"dtype":dtype,
        "order":"C","filters":null,"compressor":compressor,"fill_value":fill});
    #[cfg(feature = "experimental-output")]
    if encoding.version() == 3 {
        let bytes = json!({"name":"bytes","configuration":{"endian":"little"}});
        let mut outer = chunks.to_vec();
        let codecs = if shape.len() == 3 {
            // One 2x2 shard, encoded by zarrs with no retained application cache.
            outer[1] *= 2;
            outer[2] *= 2;
            json!([{"name":"sharding_indexed","configuration":{"chunk_shape":chunks,
                "codecs":[bytes,{"name":"gzip","configuration":{"level":1}}],
                "index_codecs":[{"name":"bytes","configuration":{"endian":"little"}}],
                "index_location":"end"}}])
        } else {
            json!([bytes])
        };
        metadata = json!({"zarr_format":3,"node_type":"array","shape":shape,
            "data_type":match dtype {"<u2"=>"uint16","|u1"=>"uint8","<f8"=>"float64","<i8"=>"int64","<i4"=>"int32",_=>return Err("unsupported output dtype".into())},
            "chunk_grid":{"name":"regular","configuration":{"chunk_shape":outer}},
            "chunk_key_encoding":{"name":"v2","configuration":{"separator":"."}},
            "fill_value":if fill.is_null(){json!(0)}else{fill},"codecs":codecs,
            "dimension_names":attrs["_ARRAY_DIMENSIONS"]});
    }
    let mut a = Array::new_with_metadata(store, path, serde_json::from_value(metadata)?)?;
    *a.attributes_mut() = serde_json::from_value(attrs)?;
    a.store_metadata_opt(&ArrayMetadataOptions::default().with_include_zarrs_metadata(false))?;
    Ok(a)
}
fn group(store: Arc<Store>, path: &str, attrs: Value, encoding: ScientificEncoding) -> Result<()> {
    let mut g = Group::new_with_metadata(
        store,
        path,
        serde_json::from_value(if encoding.version() == 3 {
            json!({"zarr_format":3,"node_type":"group"})
        } else {
            json!({"zarr_format":2})
        })?,
    )?;
    *g.attributes_mut() = serde_json::from_value(attrs)?;
    g.store_metadata()?;
    Ok(())
}
fn serial() -> zarrs::array::CodecOptions {
    let mut options = zarrs::array::CodecOptions::default();
    options.set_concurrent_target(1);
    options
}
fn encode_call<T, E>(
    store: &Store,
    seconds: &mut f64,
    f: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let io = store.read_seconds() + store.write_seconds();
    let t = Instant::now();
    let result = f();
    *seconds +=
        (t.elapsed().as_secs_f64() - (store.read_seconds() + store.write_seconds() - io)).max(0.);
    result
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
}

pub(crate) fn fetch(prepared: Prepared, path: &str, cancelled: Arc<AtomicBool>) -> Result<Value> {
    let budget = prepared.output_budget()?;
    fetch_with_options(
        prepared,
        path,
        cancelled,
        ScientificOutputOptions {
            chunk_edge: budget.chunk_edge,
            encoding: ScientificEncoding::Uncompressed,
        },
    )
}

pub(crate) fn fetch_with_options(
    prepared: Prepared,
    path: &str,
    cancelled: Arc<AtomicBool>,
    options: ScientificOutputOptions,
) -> Result<Value> {
    let started = Instant::now();
    let inspection = prepared.explain();
    let budget: Budget = serde_json::from_value(inspection["output_budget"].clone())?;
    if options.chunk_edge == 0 {
        return Err("output chunk_edge must be positive".into());
    }
    let encoding = options.encoding;
    if budget.buffer_bytes < 16 {
        return Err("scientific output needs at least 16 buffer_bytes".into());
    }
    let coordinate_edge = options.chunk_edge.min(budget.buffer_bytes / 16);
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
        let edge_y = options.chunk_edge.min(s.window.height);
        let edge_x = options.chunk_edge.min(s.window.width);
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
    // Explicit per-call payload/mask/encode reservation. A sharded library call
    // may materialize one whole shard; bound that too, never a whole scene.
    let source_area = expected
        .values()
        .map(|(_, w)| w.width * w.height)
        .max()
        .unwrap_or(0);
    let output_area = selected
        .iter()
        .map(|s| options.chunk_edge.min(s.window.width) * options.chunk_edge.min(s.window.height))
        .max()
        .unwrap_or(0);
    let shard_factor = if encoding.version() == 3 { 4 } else { 1 };
    let codec_workspace = if matches!(encoding, ScientificEncoding::Uncompressed) {
        0
    } else {
        1024 * 1024
    };
    let reservation = output_area
        .checked_mul(shard_factor)
        .and_then(|n| n.checked_mul(32))
        .and_then(|n| n.checked_add(source_area.checked_mul(6)?))
        .and_then(|n| n.checked_add(codec_workspace))
        .ok_or("output reservation overflow")?;
    if reservation > budget.buffer_bytes {
        return Err("scientific output encode/shard reservation exceeds buffer_bytes; reduce output chunk_edge or raise buffer budget".into());
    }
    check_cancelled(&cancelled)?;
    let transaction = Transaction::new(Path::new(path))?;
    let mut filesystem_options = FilesystemStoreOptions::default();
    filesystem_options.defer_device_flush(true);
    let metrics = Arc::new(Metrics::new(Arc::new(FilesystemStore::new_with_options(
        &transaction.stage,
        filesystem_options,
    )?)));
    let store = Arc::new(Store::new(metrics.clone()));
    let output_records = outputs(plan);
    let root_attrs = json!({"title":"Native source observations","status":"incomplete",
        "layout":"one source asset/band per observation group, chronological order; no aligned cube",
        "observations":output_records,"logical_request":inspection["logical_request"],
        "sources":inspection["sources"],"query":inspection["query"],
        "processing":"native stored values; no calibration, reprojection, resampling or mosaic",
        "software":{"name":"tile-ripper","version":env!("CARGO_PKG_VERSION"),"writer":"zarrs 0.23.14","zarr_format":encoding.version()},
        "output_encoding":encoding,"output_chunk_edge":options.chunk_edge});
    group(store.clone(), "/", root_attrs.clone(), encoding)?;
    group(store.clone(), "/observations", json!({}), encoding)?;
    let mut observations = Vec::new();
    let mut writer_peak = reservation;
    let mut encode_seconds = 0.;
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
            encoding,
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
                encoding,
            )?;
            for start in (0..len).step_by(edge) {
                check_cancelled(&cancelled)?;
                let values: Vec<_> = (start..(start + edge).min(len))
                    .map(|j| origin + (j as f64 + 0.5) * step)
                    .collect();
                writer_peak = writer_peak.max(edge * 16);
                encode_call(&store, &mut encode_seconds, || {
                    a.store_array_subset_opt(
                        &ArraySubset::new_with_start_shape(
                            vec![start as u64],
                            vec![values.len() as u64],
                        )?,
                        &values,
                        &serial(),
                    )
                })?;
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
            encoding,
        )?;
        encode_call(&store, &mut encode_seconds, || {
            time.store_chunk_opt(&[0], &[time_value], &serial())
        })?;
        let crs = array(
            store.clone(),
            &format!("{base}/spatial_ref"),
            &[],
            &[],
            "<i4",
            Value::Null,
            crs_attributes(&l.crs, t),
            encoding,
        )?;
        encode_call(&store, &mut encode_seconds, || {
            crs.store_chunk_opt(&[], &[0i32], &serial())
        })?;
        let shape = [1, s.window.height, s.window.width];
        let chunks = [
            1,
            options.chunk_edge.min(s.window.height),
            options.chunk_edge.min(s.window.width),
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
            encoding,
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
            encoding,
        )?;
        let aoi = array(
            store.clone(),
            &format!("{base}/aoi_mask"),
            &shape,
            &chunks,
            "|u1",
            json!(0),
            mask_attrs("AOI pixel center inclusion; source nodata may still be included"),
            encoding,
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
        });
    }
    let setup_seconds = started.elapsed().as_secs_f64();
    crate::diagnostics::profile_stage("output_setup", setup_seconds);
    let source_stream = Instant::now();
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
        encode_call(&store, &mut encode_seconds, || {
            o.values.store_array_subset_opt(&subset, values, &serial())
        })?;
        let valid: Vec<u8> = c.validity().map(u8::from).collect();
        encode_call(&store, &mut encode_seconds, || {
            o.valid.store_array_subset_opt(&subset, &valid, &serial())
        })?;
        let mask = o
            .geometry
            .as_ref()
            .map(|g| crate::spatial::mask(g, l, window))
            .transpose()?;
        let aoi = mask
            .as_ref()
            .map(|m| m.data().to_vec())
            .unwrap_or_else(|| vec![1; values.len()]);
        encode_call(&store, &mut encode_seconds, || {
            o.aoi.store_array_subset_opt(&subset, &aoi, &serial())
        })?;
        // Payload and encode scratch were admitted before creating staging;
        // these temporaries are dropped before accepting the next delivery.
        first_output.get_or_insert_with(|| started.elapsed().as_secs_f64());
        sink_seconds += write_start.elapsed().as_secs_f64();
    }
    let mut report = stream.finish()?;
    crate::diagnostics::profile_stage(
        "source_stream_including_backpressure",
        source_stream.elapsed().as_secs_f64(),
    );
    if written.len() != expected.len() || report.chunks != written.len() {
        return Err("incomplete COG output coverage".into());
    }
    check_cancelled(&cancelled)?;
    let finalize = Instant::now();
    let mut attrs = root_attrs;
    attrs["status"] = json!("complete");
    group(store.clone(), "/", attrs, encoding)?;
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
        encode_seconds,
        ..Default::default()
    });
    report.output_finalize_seconds = finalize.elapsed().as_secs_f64();
    let manifest_start = Instant::now();
    let mut manifest = json!({"schema_version":3,"document_type":"acquisition_result","status":"complete",
        "format":format!("zarr_v{}",encoding.version()),"writer":"zarrs 0.23.14","sources":inspection["sources"],"outputs":output_records,
        "output_encoding":encoding,"output_chunk_edge":options.chunk_edge,
        "observed":report,"processing":["native stored UInt16 values; calibration is metadata only","one source asset/band and native grid per observation; chronological order","no reprojection, resampling, mosaicking or implicit alignment","valid and aoi_mask distinguish source nodata from excluded AOI centers"],
        "completion":{"stream_finish":"successful","publication":"atomic no-replace sibling rename","existing_destination":"refused; replacement is never implicit","scope":"validated provider ETags or local size/mtime, not an atomic multi-object snapshot",
            "synchronization":"file contents and metadata before closing every writer handle; bottom-up directories and pre-publication barrier; rename; parent synchronization and post-publication barrier",
            "memory_scope":"output_io.buffered_bytes_peak is a separate bounded per-encode reservation including delivered payload, masks, output scratch and codec allowance; add scheduler reservations; neither is an RSS ceiling"}});
    manifest["inspection"] = inspection;
    crate::diagnostics::profile_stage(
        "manifest_generation",
        manifest_start.elapsed().as_secs_f64(),
    );
    let serialization = Instant::now();
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    crate::diagnostics::profile_stage(
        "manifest_serialization",
        serialization.elapsed().as_secs_f64(),
    );
    // Metrics above exclude this manifest (like the existing output adapter).
    let write = Instant::now();
    store.set(&StoreKey::new("manifest.json")?, bytes.into())?;
    crate::diagnostics::profile_stage("manifest_write", write.elapsed().as_secs_f64());
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
        let publication = Instant::now();
        // Every file has already been fsynced on its original writing handle.
        // Reopening files here instead would risk missing prior write-back errors.
        fn sync_dirs(path: &Path, cancelled: &AtomicBool) -> Result<()> {
            check_cancelled(cancelled)?;
            for e in fs::read_dir(path)? {
                let e = e?;
                if e.file_type()?.is_dir() {
                    sync_dirs(&e.path(), cancelled)?;
                }
            }
            host_sync(&fs::File::open(path)?)?;
            Ok(())
        }
        let t = Instant::now();
        sync_dirs(&self.stage, cancelled)?;
        // Include newly created destination ancestors, not just the immediate
        // parent. Persist the hidden sibling's name before its commit rename.
        for ancestor in self.destination.parent().unwrap().ancestors() {
            check_cancelled(cancelled)?;
            host_sync(&fs::File::open(ancestor)?)?;
        }
        crate::diagnostics::profile_stage("final_directory_sync", t.elapsed().as_secs_f64());
        let barrier = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.stage.join("manifest.json"))?;
        let t = Instant::now();
        device_barrier(&barrier)?;
        crate::diagnostics::profile_stage("prepublish_device_flush", t.elapsed().as_secs_f64());
        check_cancelled(cancelled)?;
        let t = Instant::now();
        rename_new(&self.stage, &self.destination)?;
        crate::diagnostics::profile_stage("atomic_publication", t.elapsed().as_secs_f64());
        // Rename is the commit point. A failure after it must leave the complete
        // destination in place and explicitly report its unconfirmed durability.
        let post_commit = || -> Result<()> {
            let t = Instant::now();
            host_sync(&fs::File::open(self.destination.parent().unwrap())?)?;
            crate::diagnostics::profile_stage("publication_parent_sync", t.elapsed().as_secs_f64());
            let t = Instant::now();
            device_barrier(&barrier)?;
            crate::diagnostics::profile_stage(
                "postpublish_device_flush",
                t.elapsed().as_secs_f64(),
            );
            Ok(())
        };
        post_commit().map_err(|e| format!("complete output was published at {}, but final synchronization failed; durability is unconfirmed: {e}", self.destination.display()))?;
        crate::diagnostics::profile_stage("publication_total", publication.elapsed().as_secs_f64());
        Ok(())
    }
}

fn host_sync(file: &fs::File) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(file.sync_all()?)
    }
}
fn device_barrier(file: &fs::File) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // Require the explicit stronger macOS request and propagate failures;
        // POSIX fsync alone does not satisfy this transaction barrier.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(file.sync_all()?)
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
        fs::write(tx.stage.join("manifest.json"), b"complete").unwrap();
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
    #[test]
    fn publication_rejects_files_and_dangling_links_created_after_staging() {
        use std::os::unix::fs::symlink;
        for link in [false, true] {
            let r = root();
            let dest = r.join("result.zarr");
            let tx = Transaction::new(&dest).unwrap();
            fs::write(tx.stage.join("manifest.json"), b"complete").unwrap();
            if link {
                symlink("missing", &dest).unwrap();
            } else {
                fs::write(&dest, b"winner").unwrap();
            }
            assert!(tx.publish(&AtomicBool::new(false)).is_err());
            if link {
                assert_eq!(fs::read_link(&dest).unwrap(), Path::new("missing"));
            } else {
                assert_eq!(fs::read(&dest).unwrap(), b"winner");
            }
            assert_eq!(fs::read_dir(&r).unwrap().count(), 1);
            fs::remove_dir_all(r).unwrap();
        }
    }
    #[test]
    fn successful_nested_publication_has_all_data_and_no_staging_name() {
        let r = root();
        let dest = r.join("new/ancestors/result.zarr");
        let tx = Transaction::new(&dest).unwrap();
        // Exercise the actual store path, including file synchronisation.
        let mut options = FilesystemStoreOptions::default();
        options.defer_device_flush(true);
        let store = FilesystemStore::new_with_options(&tx.stage, options).unwrap();
        store
            .set(
                &StoreKey::new("manifest.json").unwrap(),
                b"complete".to_vec().into(),
            )
            .unwrap();
        store
            .set(
                &StoreKey::new("group/chunk").unwrap(),
                b"pixels".to_vec().into(),
            )
            .unwrap();
        tx.publish(&AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(dest.join("group/chunk")).unwrap(), b"pixels");
        assert_eq!(fs::read_dir(dest.parent().unwrap()).unwrap().count(), 1);
        fs::remove_dir_all(r).unwrap();
    }
}
