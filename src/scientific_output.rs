//! COG chunks -> ordinary per-observation Zarr v2 groups. zarrs owns encoding.
//! The destination name is published only after successful stream completion.
use crate::{
    acquisition::Prepared, execute::OutputIo, model::*, output_transaction::Transaction, Result,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
#[cfg(test)]
use std::{fs, path::PathBuf, sync::atomic::AtomicU64};
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

#[cfg(feature = "native-hdf5")]
pub(crate) mod arrays;
#[cfg(feature = "native-hdf5")]
pub(crate) mod swot;

/// Lossless scientific-output candidates. The ordinary fetch stays Zarr v2,
/// uncompressed. V3 is deliberately gated at build time.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScientificEncoding {
    Uncompressed,
    Gzip,
    BloscZstd,
    #[cfg(feature = "experimental-output")]
    ExperimentalV3Uncompressed,
    #[cfg(feature = "experimental-output")]
    ExperimentalV3ShardedUncompressed,
    #[cfg(feature = "experimental-output")]
    ExperimentalV3ShardedGzip,
    #[cfg(feature = "experimental-output")]
    ExperimentalV3ShardedBloscZstd,
    #[cfg(feature = "experimental-output")]
    ExperimentalV3ShardedZstd,
}
impl ScientificEncoding {
    fn version(self) -> u8 {
        match self {
            #[cfg(feature = "experimental-output")]
            Self::ExperimentalV3Uncompressed
            | Self::ExperimentalV3ShardedUncompressed
            | Self::ExperimentalV3ShardedGzip
            | Self::ExperimentalV3ShardedBloscZstd
            | Self::ExperimentalV3ShardedZstd => 3,
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
            Self::ExperimentalV3Uncompressed
            | Self::ExperimentalV3ShardedUncompressed
            | Self::ExperimentalV3ShardedGzip
            | Self::ExperimentalV3ShardedBloscZstd
            | Self::ExperimentalV3ShardedZstd => Value::Null,
        }
    }
    fn shard_factor(self) -> usize {
        match self {
            #[cfg(feature = "experimental-output")]
            Self::ExperimentalV3ShardedUncompressed
            | Self::ExperimentalV3ShardedGzip
            | Self::ExperimentalV3ShardedBloscZstd
            | Self::ExperimentalV3ShardedZstd => 4,
            _ => 1,
        }
    }
    fn codec_workspace(self) -> usize {
        match self {
            Self::Uncompressed => 0,
            #[cfg(feature = "experimental-output")]
            Self::ExperimentalV3Uncompressed | Self::ExperimentalV3ShardedUncompressed => 0,
            _ => 1024 * 1024,
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

#[derive(Clone, Copy)]
enum StorageType {
    UInt8,
    UInt16,
    Int16,
    Float32,
}
impl StorageType {
    fn from_layout(dtype: &str) -> Option<Self> {
        match dtype {
            "Byte" | "|u1" => Some(Self::UInt8),
            "UInt16" | "<u2" => Some(Self::UInt16),
            "Int16" | "<i2" => Some(Self::Int16),
            "Float32" | "<f4" => Some(Self::Float32),
            _ => None,
        }
    }
    fn zarr(self) -> &'static str {
        match self {
            Self::UInt8 => "|u1",
            Self::UInt16 => "<u2",
            Self::Int16 => "<i2",
            Self::Float32 => "<f4",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::UInt8 => "uint8",
            Self::UInt16 => "uint16",
            Self::Int16 => "int16",
            Self::Float32 => "float32",
        }
    }
    fn bytes(self) -> usize {
        match self {
            Self::UInt8 => 1,
            Self::UInt16 | Self::Int16 => 2,
            Self::Float32 => 4,
        }
    }
    fn nodata(self, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }
        match self {
            Self::UInt8 => value.fract() == 0. && (0. ..=u8::MAX as f64).contains(&value),
            Self::UInt16 => value.fract() == 0. && (0. ..=u16::MAX as f64).contains(&value),
            Self::Int16 => {
                value.fract() == 0. && (i16::MIN as f64..=i16::MAX as f64).contains(&value)
            }
            Self::Float32 => (value as f32) as f64 == value,
        }
    }
    fn fill(self, nodata: Option<f64>) -> Value {
        match (self, nodata) {
            // Zarr v2 serializes its non-finite Float32 fill using the string
            // spelling. This keeps unwritten Float32 cells invalid even when
            // the source has no finite nodata sentinel (including GDAL NaN).
            (Self::Float32, None) => json!("NaN"),
            (_, None) => Value::Null,
            (Self::UInt8 | Self::UInt16, Some(value)) => json!(value as u64),
            (Self::Int16, Some(value)) => json!(value as i64),
            (Self::Float32, Some(value)) => json!(value as f32),
        }
    }
}

fn quality_policies(
    inspection: &Value,
    asset_id: &str,
) -> Result<Vec<crate::products::profile::QualityPolicy>> {
    let Some(source) = inspection["sources"]
        .as_array()
        .and_then(|sources| sources.iter().find(|source| source["asset_id"] == asset_id))
    else {
        return Ok(Vec::new());
    };
    let Some(values) = source.get("quality_policies") else {
        return Ok(Vec::new());
    };
    let values = values
        .as_array()
        .ok_or("source quality_policies must be an array")?;
    let band = source["product_band"]["name"].as_str();
    let mut policies = Vec::with_capacity(values.len());
    for value in values {
        let name = value
            .as_str()
            .ok_or("source quality policy name must be a string")?;
        let policy: crate::products::profile::QualityPolicy = name.parse()?;
        if band.is_none_or(|band| !policy.required_bands().contains(&band)) {
            return Err("quality policy is not bound to this raw product band".into());
        }
        if policies.contains(&policy) {
            return Err("duplicate source quality policy".into());
        }
        policies.push(policy);
    }
    Ok(policies)
}

#[derive(Clone)]
struct ProductValidity {
    profile: crate::products::profile::ProductProfile,
    band: crate::products::profile::BandProfile,
}

fn product_validity(source: &Value) -> Result<Option<ProductValidity>> {
    let Some(product) = source.get("product").and_then(Value::as_str) else {
        return Ok(None);
    };
    let band = source["product_band"]["name"]
        .as_str()
        .ok_or("product source is missing its canonical band name")?;
    let profile = crate::products::profile::profile(product)?;
    let band = profile.band(band)?.clone();
    Ok(Some(ProductValidity { profile, band }))
}

fn product_missingness(source: &Value, layout: &Layout, product: &ProductValidity) -> Value {
    let fill_rule = if product.profile.family == crate::products::profile::ProductFamily::Landsat
        && matches!(product.band.name.as_str(), "QA_PIXEL" | "SR_QA_AEROSOL")
    {
        "stored bit 0 set marks product fill; profile nodata is also applied"
    } else {
        "the product profile nodata value is applied when present"
    };
    json!({
        "validity":"AOI inclusion AND physical source validity AND product semantic validity",
        "physical_source":{
            "layout_nodata":layout.nodata,
            "gdal_nodata_text":source["object_identities"]["native_cog"]["gdal_nodata_text"],
            "provenance":"sources[].object_identities.native_cog"
        },
        "product":{
            "profile":product.profile.id,
            "band":product.band.name,
            "nodata":product.band.nodata,
            "fill_rule":fill_rule
        },
        "raw_values":"preserved; product missingness does not create a physical source nodata value"
    })
}
/// Validate annual interval metadata before output staging or time encoding.
fn annual_bounds(source: &Value) -> Result<Option<[i64; 2]>> {
    let Some(extent) = source.get("temporal_extent") else {
        return Ok(None);
    };
    let nanos = |key: &str| -> Result<i64> {
        let date = chrono::DateTime::parse_from_rfc3339(
            extent[key]
                .as_str()
                .ok_or("annual coverage endpoint missing")?,
        )?;
        Ok(date
            .timestamp_nanos_opt()
            .ok_or("annual coverage outside nanosecond range")?)
    };
    let bounds = [nanos("coverage_start")?, nanos("coverage_end")?];
    if bounds[0] > bounds[1] {
        return Err("annual coverage is reversed".into());
    }
    Ok(Some(bounds))
}
fn quality_mask(
    policy: crate::products::profile::QualityPolicy,
    pixels: &crate::Pixels,
) -> Result<Vec<u8>> {
    use crate::products::profile::QualitySample;
    let mut mask = Vec::with_capacity(pixels.len());
    match pixels {
        crate::Pixels::UInt8(values) => {
            for value in values {
                mask.push(u8::from(policy.accepts(QualitySample::UInt8(*value))?));
            }
        }
        crate::Pixels::UInt16(values) => {
            for value in values {
                mask.push(u8::from(policy.accepts(QualitySample::UInt16(*value))?));
            }
        }
        crate::Pixels::Float32(values) => {
            for value in values {
                mask.push(u8::from(policy.accepts(QualitySample::Float32(*value))?));
            }
        }
        crate::Pixels::Int8(_) | crate::Pixels::Int16(_) | crate::Pixels::UInt32(_) => {
            return Err("quality policy received an unsupported raw storage type".into());
        }
    }
    Ok(mask)
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
        let storage = l.and_then(|l| StorageType::from_layout(&l.dtype));
        json!({"file":".","group":format!("observations/{i:06}"),"dataset":"values",
            "asset_id":s.asset_id,"variable":l.map(|l|&l.variable),"datetime":s.datetime,
            "timestamps":s.timestamps,"window":s.window,"dtype":storage.map(StorageType::name),"native_dtype":storage.map(StorageType::zarr),
            "nodata":l.and_then(|l|l.nodata),"scale":l.and_then(|l|l.scale),"offset":l.and_then(|l|l.offset),
            "native_grid":l.map(|l|json!({"crs":l.crs,"transform":crate::spatial::window_transform(l,&s.window)})),
            "dimensions":{"names":["time","y","x"],"shape":[1,s.window.height,s.window.width]},
            "aoi_mask_applied":s.mask_geometry.is_some(),
            "validity":"valid = AOI inclusion AND physical source validity AND product semantic validity when a product profile is attached; 0 invalid, 1 valid"})
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
        let mut inner = vec![bytes.clone()];
        if shape.len() == 3 {
            match encoding {
                ScientificEncoding::ExperimentalV3ShardedGzip =>
                    inner.push(json!({"name":"gzip","configuration":{"level":1}})),
                ScientificEncoding::ExperimentalV3ShardedBloscZstd =>
                    inner.push(json!({"name":"blosc","configuration":{"cname":"zstd","clevel":1,"shuffle":"shuffle","typesize":match dtype {"|u1"=>1,"<u2"|"<i2"=>2,"<f4"=>4,_=>1},"blocksize":0}})),
                ScientificEncoding::ExperimentalV3ShardedZstd =>
                    inner.push(json!({"name":"zstd","configuration":{"level":1,"checksum":true}})),
                _ => {}
            }
        }
        let codecs = if shape.len() == 3 && encoding.shard_factor() == 4 {
            // One 2x2 shard, encoded by zarrs with no retained application cache.
            outer[1] *= 2;
            outer[2] *= 2;
            json!([{"name":"sharding_indexed","configuration":{"chunk_shape":chunks,
                "codecs":inner,
                "index_codecs":[bytes,{"name":"crc32c"}],
                "index_location":"end"}}])
        } else {
            json!(inner)
        };
        metadata = json!({"zarr_format":3,"node_type":"array","shape":shape,
            "data_type":match dtype {"<u2"=>"uint16","<i2"=>"int16","|u1"=>"uint8","<f4"=>"float32","<f8"=>"float64","<i8"=>"int64","<i4"=>"int32",_=>return Err("unsupported output dtype".into())},
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
fn serial_required() -> zarrs::array::CodecOptions {
    let mut options = serial();
    // Zarr v2 `fill_value: null` means that absent chunks are uninitialized.
    // Persist legitimate all-zero source/coordinate/scalar payloads rather
    // than allowing zarrs' type-default empty-chunk elision to lose them.
    options.set_store_empty_chunks(true);
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
    quality: Vec<(crate::products::profile::QualityPolicy, Array<Store>)>,
    product_validity: Option<ProductValidity>,
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
    let mut product_validities = HashMap::new();
    for selected in &selected {
        let source = inspection["sources"]
            .as_array()
            .and_then(|sources| {
                sources
                    .iter()
                    .find(|source| source["asset_id"] == selected.asset_id)
            })
            .unwrap_or(&Value::Null);
        if let Some(validity) = product_validity(source)? {
            product_validities.insert(selected.asset_id.clone(), validity);
        }
    }
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
        let storage = StorageType::from_layout(&l.dtype)
            .ok_or("scientific COG output requires Byte, UInt16, Int16, or Float32 native bands")?;
        if s.dimensions.is_some() || !matches!(l.variable, Variable::Band { .. }) {
            return Err("scientific COG output requires a supported native raster band".into());
        }
        if l.transform.iter().any(|v| !v.is_finite())
            || l.transform[2] != 0.
            || l.transform[4] != 0.
        {
            return Err("rectilinear Zarr output requires an unrotated native grid; no resampling is performed".into());
        }
        if let Some(n) = l.nodata {
            if !storage.nodata(n) {
                return Err("COG nodata is not exactly representable in its storage type".into());
            }
        }
        let edge_y = options.chunk_edge.min(s.window.height);
        let edge_x = options.chunk_edge.min(s.window.width);
        let bytes = edge_y
            .checked_mul(edge_x)
            .and_then(|n| n.checked_mul(storage.bytes()))
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
        let quality_count = quality_policies(&inspection, &s.asset_id)?.len();
        let source = inspection["sources"]
            .as_array()
            .and_then(|sources| {
                sources
                    .iter()
                    .find(|source| source["asset_id"] == s.asset_id)
            })
            .unwrap_or(&Value::Null);
        let bounds_objects = if annual_bounds(source)?.is_some() {
            3
        } else {
            0
        };
        objects = objects
            .checked_add(
                18 + bounds_objects
                    + s.window.width.div_ceil(coordinate_edge)
                    + s.window.height.div_ceil(coordinate_edge),
            )
            .and_then(|n| n.checked_add(tiles.checked_mul(3)?))
            .and_then(|n| n.checked_add(quality_count.checked_mul(2 + tiles)?))
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
    let source_element_bytes = selected
        .iter()
        .filter_map(|s| {
            s.layout
                .as_ref()
                .and_then(|l| StorageType::from_layout(&l.dtype))
                .map(StorageType::bytes)
        })
        .max()
        .unwrap_or(1);
    let output_area = selected
        .iter()
        .map(|s| options.chunk_edge.min(s.window.width) * options.chunk_edge.min(s.window.height))
        .max()
        .unwrap_or(0);
    let shard_factor = encoding.shard_factor();
    let codec_workspace = encoding.codec_workspace();
    let has_quality = selected
        .iter()
        .try_fold(false, |found, selected| -> Result<_> {
            Ok(found || !quality_policies(&inspection, &selected.asset_id)?.is_empty())
        })?;
    let reservation = output_area
        .checked_mul(shard_factor)
        .and_then(|n| n.checked_mul(32))
        .and_then(|n| {
            n.checked_add(
                source_area.checked_mul(source_element_bytes + 4 + usize::from(has_quality))?,
            )
        })
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
        transaction.stage(),
        filesystem_options,
    )?)));
    let store = Arc::new(Store::new(metrics.clone()));
    let mut output_records = outputs(plan);
    if let Some(records) = output_records.as_array_mut() {
        for (record, selected) in records.iter_mut().zip(&selected) {
            let policies = quality_policies(&inspection, &selected.asset_id)?;
            if let Some(source) = inspection["sources"].as_array().and_then(|sources| {
                sources
                    .iter()
                    .find(|source| source["asset_id"] == selected.asset_id)
            }) {
                for key in [
                    "temporal_extent",
                    "time_coordinate_role",
                    "prototype",
                    "qualification_scope",
                ] {
                    if let Some(value) = source.get(key) {
                        record[key] = value.clone();
                    }
                }
            }
            if !policies.is_empty() {
                record["derived_masks"] = json!(policies
                    .iter()
                    .map(|policy| json!({
                        "dataset":format!("quality_{}", policy.name()),
                        "policy":policy.name(),
                        "description":policy.description(),
                        "scope":"this raw observation intersected with physical source validity, product semantic validity, and AOI inclusion"
                    }))
                    .collect::<Vec<_>>());
            }
        }
    }
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
        let storage = StorageType::from_layout(&l.dtype).unwrap();
        let base = format!("/observations/{i:06}");
        let source = inspection["sources"]
            .as_array()
            .and_then(|a| a.iter().find(|v| v["asset_id"] == s.asset_id))
            .cloned()
            .unwrap_or(Value::Null);
        let policies = quality_policies(&inspection, &s.asset_id)?;
        let product_validity = product_validities.get(&s.asset_id).cloned();
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
        let mut observation_attrs = json!({"asset_id":s.asset_id,"variable":l.variable,"datetime":s.datetime,
            "source":source,"source_window":s.window,"source_grid":l,"transform":t,"crs":l.crs,
            "source_nodata":l.nodata,"source_scale":l.scale,"source_offset":l.offset,
            "aoi_mask_applied":s.mask_geometry.is_some(),"time_missing":s.datetime.is_none()});
        for key in [
            "temporal_extent",
            "time_coordinate_role",
            "prototype",
            "qualification_scope",
        ] {
            if let Some(value) = source.get(key) {
                observation_attrs[key] = value.clone();
            }
        }
        group(store.clone(), &base, observation_attrs, encoding)?;
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
                        &serial_required(),
                    )
                })?;
            }
        }
        let bounds = annual_bounds(&source)?;
        if bounds.is_some_and(|bounds| bounds[0] != time_value) {
            return Err("annual time coordinate differs from source coverage_start".into());
        }
        let mut time_attrs = json!({"_ARRAY_DIMENSIONS":["time"],"standard_name":"time","units":"nanoseconds since 1970-01-01 00:00:00","calendar":"proleptic_gregorian"});
        if bounds.is_some() {
            time_attrs["bounds"] = json!("time_bounds");
            time_attrs["long_name"] = json!("actual source coverage start for an annual summary");
            time_attrs["time_coordinate_role"] = json!("source_coverage_start");
        }
        let time = array(
            store.clone(),
            &format!("{base}/time"),
            &[1],
            &[1],
            "<i8",
            Value::Null,
            time_attrs,
            encoding,
        )?;
        encode_call(&store, &mut encode_seconds, || {
            time.store_chunk_opt(&[0], &[time_value], &serial_required())
        })?;
        if let Some(bounds) = bounds {
            let output = array(
                store.clone(),
                &format!("{base}/time_bounds"),
                &[1, 2],
                &[1, 2],
                "<i8",
                Value::Null,
                json!({"_ARRAY_DIMENSIONS":["time","bounds"],"units":"nanoseconds since 1970-01-01 00:00:00","calendar":"proleptic_gregorian","endpoint_semantics":"actual provider coverage endpoints, unchanged"}),
                encoding,
            )?;
            encode_call(&store, &mut encode_seconds, || {
                output.store_chunk_opt(&[0, 0], &bounds, &serial_required())
            })?;
        }
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
            crs.store_chunk_opt(&[], &[0i32], &serial_required())
        })?;
        let shape = [1, s.window.height, s.window.width];
        let chunks = [
            1,
            options.chunk_edge.min(s.window.height),
            options.chunk_edge.min(s.window.width),
        ];
        let quality_names = policies
            .iter()
            .map(|policy| format!("quality_{}", policy.name()))
            .collect::<Vec<_>>();
        let ancillary = std::iter::once("valid".to_owned())
            .chain(std::iter::once("aoi_mask".to_owned()))
            .chain(quality_names.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        let mut attrs = json!({"_ARRAY_DIMENSIONS":["time","y","x"],"grid_mapping":"spatial_ref",
            "ancillary_variables":ancillary,"source_nodata":l.nodata,
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
        if let Some(unit) = source["product_band"]["unit"]
            .as_str()
            .or_else(|| stac["unit"].as_str())
        {
            attrs["units"] = json!(unit);
        }
        attrs["calibration_precedence"] = json!("TIFF metadata, then STAC when absent; original fields retained in source provenance; stored values unmodified");
        for key in ["product_band", "quality_policies"] {
            if source.get(key).is_some_and(|value| !value.is_null()) {
                attrs[key] = source[key].clone();
            }
        }
        if let Some(product) = &product_validity {
            attrs["missingness"] = product_missingness(&source, l, product);
        }
        let values = array(
            store.clone(),
            &format!("{base}/values"),
            &shape,
            &chunks,
            storage.zarr(),
            storage.fill(l.nodata),
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
            Value::Null,
            mask_attrs(
                "AOI included and valid under physical source and product semantic missingness",
            ),
            encoding,
        )?;
        let aoi = array(
            store.clone(),
            &format!("{base}/aoi_mask"),
            &shape,
            &chunks,
            "|u1",
            Value::Null,
            mask_attrs("AOI pixel center inclusion; source nodata may still be included"),
            encoding,
        )?;
        let mut quality = Vec::with_capacity(policies.len());
        for (policy, name) in policies.into_iter().zip(quality_names) {
            let mask = array(
                store.clone(),
                &format!("{base}/{name}"),
                &shape,
                &chunks,
                "|u1",
                Value::Null,
                json!({"_ARRAY_DIMENSIONS":["time","y","x"],"grid_mapping":"spatial_ref",
                    "long_name":policy.description(),"flag_values":[0,1],
                    "flag_meanings":"rejected accepted","quality_policy":policy.name(),
                    "derivation":"policy evaluated from this observation's raw stored samples, then intersected with physical source validity, product semantic validity, and AOI inclusion; no calibration or cross-band alignment"}),
                encoding,
            )?;
            quality.push((policy, mask));
        }
        observations.push(Observation {
            selected: (*s).clone(),
            values,
            valid,
            aoi,
            quality,
            product_validity,
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
        if c.pixels.len() != window.width * window.height {
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
        macro_rules! write_values {
            ($values:expr) => {
                encode_call(&store, &mut encode_seconds, || {
                    o.values
                        .store_array_subset_opt(&subset, $values, &serial_required())
                })?
            };
        }
        match &c.pixels {
            crate::Pixels::UInt8(values) => write_values!(values),
            crate::Pixels::UInt16(values) => write_values!(values),
            crate::Pixels::Int16(values) => write_values!(values),
            crate::Pixels::Float32(values) => write_values!(values),
            crate::Pixels::Int8(_) | crate::Pixels::UInt32(_) => {
                return Err("unsupported COG output sample type".into())
            }
        }
        let mask = o
            .geometry
            .as_ref()
            .map(|g| crate::spatial::mask(g, l, window))
            .transpose()?;
        let aoi = mask
            .as_ref()
            .map(|m| m.data().to_vec())
            .unwrap_or_else(|| vec![1; c.pixels.len()]);
        let mut valid = Vec::with_capacity(c.pixels.len());
        if let Some(product) = &o.product_validity {
            valid.extend(
                c.validity()
                    .zip(&aoi)
                    .zip(crate::products::source_validity(
                        &product.profile,
                        &product.band,
                        &c.pixels,
                    ))
                    .map(|((physical, aoi), semantic)| u8::from(physical && *aoi != 0 && semantic)),
            );
        } else {
            valid.extend(
                c.validity()
                    .zip(&aoi)
                    .map(|(physical, aoi)| u8::from(physical && *aoi != 0)),
            );
        }
        encode_call(&store, &mut encode_seconds, || {
            o.valid
                .store_array_subset_opt(&subset, &valid, &serial_required())
        })?;
        encode_call(&store, &mut encode_seconds, || {
            o.aoi
                .store_array_subset_opt(&subset, &aoi, &serial_required())
        })?;
        for (policy, output) in &o.quality {
            let mut derived = quality_mask(*policy, &c.pixels)?;
            for (accepted, included) in derived.iter_mut().zip(&valid) {
                *accepted &= *included;
            }
            encode_call(&store, &mut encode_seconds, || {
                output.store_array_subset_opt(&subset, &derived, &serial_required())
            })?;
        }
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
    // Chunks wholly outside a polygon AOI are never read. Masks declare no
    // fill value (xarray would decode it as missing), so store their zeros.
    let mask_fill = Instant::now();
    let mut touched = vec![HashSet::new(); observations.len()];
    for (i, window) in expected.values() {
        let o = &observations[*i];
        let subset = ArraySubset::new_with_start_shape(
            vec![
                0,
                (window.y - o.selected.window.y) as u64,
                (window.x - o.selected.window.x) as u64,
            ],
            vec![1, window.height as u64, window.width as u64],
        )?;
        if let Some(chunks) = o.valid.chunks_in_array_subset(&subset)? {
            touched[*i].extend(&chunks.indices());
        }
    }
    for (o, touched) in observations.iter().zip(&touched) {
        let grid = ArraySubset::new_with_shape(o.valid.chunk_grid_shape().to_vec());
        for chunk in &grid.indices() {
            if touched.contains(&chunk) {
                continue;
            }
            check_cancelled(&cancelled)?;
            let subset = o.valid.chunk_subset_bounded(&chunk)?;
            let zeros = vec![0u8; subset.num_elements_usize()];
            for mask in [&o.valid, &o.aoi]
                .into_iter()
                .chain(o.quality.iter().map(|(_, mask)| mask))
            {
                encode_call(&store, &mut encode_seconds, || {
                    mask.store_array_subset_opt(&subset, &zeros, &serial_required())
                })?;
            }
        }
    }
    sink_seconds += mask_fill.elapsed().as_secs_f64();
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
        "observed":report,"processing":["native stored typed values; calibration is metadata only","one source asset/band and native grid per observation; chronological order","no reprojection, resampling, mosaicking or implicit alignment","valid and aoi_mask distinguish source nodata from excluded AOI centers"],
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
    fn quality_masks_are_derived_without_mutating_raw_samples() {
        use crate::products::profile::QualityPolicy;
        let raw = crate::Pixels::UInt8(vec![0, 2, 255]);
        assert_eq!(
            quality_mask(QualityPolicy::HlsFmaskClear, &raw).unwrap(),
            [1, 0, 0]
        );
        assert_eq!(raw.as_u8().unwrap(), [0, 2, 255]);

        let nan = f32::from_bits(0x7fc01234);
        let radar = crate::Pixels::Float32(vec![0.25, nan]);
        assert_eq!(
            quality_mask(QualityPolicy::OperaRtcFinite, &radar).unwrap(),
            [1, 0]
        );
        assert_eq!(radar.as_f32().unwrap()[1].to_bits(), 0x7fc01234);

        let reserved = crate::Pixels::UInt8(vec![1]);
        assert!(quality_mask(QualityPolicy::HlsFmaskClear, &reserved).is_err());
    }
    #[cfg(all(feature = "native-cog", not(feature = "gdal")))]
    fn typed_product_request(source: &str, output: &Path) -> Request {
        Request {
            source: Source::Assets {
                assets: vec![Asset {
                    etag: None,
                    id: "typed-product".into(),
                    locator: Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/data/native_cog")
                        .join(source)
                        .to_string_lossy()
                        .into(),
                    format: Format::Cog,
                    datetime: None,
                }],
            },
            spatial: Spatial::Bounds {
                crs: "EPSG:32617".into(),
                bbox: [500000., 3999040., 500960., 4000000.],
            },
            time: None,
            variables: vec![Variable::Band { index: 1 }],
            quality: vec![],
            grid: Grid::Native,
            operations: vec![],
            sink: Sink::ZarrDirectory {
                path: output.to_string_lossy().into(),
            },
            budget: Budget {
                buffer_bytes: 16 * 1024 * 1024,
                max_nodes: 4096,
                chunk_edge: 64,
            },
        }
    }
    #[cfg(all(feature = "native-cog", not(feature = "gdal")))]
    #[test]
    fn typed_product_metadata_and_quality_mask_are_written_independently() {
        use zarrs::filesystem::FilesystemStore;
        let r = root();
        let dest = r.join("radar.zarr");
        let prepared = crate::acquisition::prepare(
            &typed_product_request("radar_float32.tif", &dest),
            &mut crate::cog::LayoutCache::default(),
        )
        .unwrap()
        .with_product_metadata(
            vec![json!({
                "asset_id":"typed-product",
                "product_band":{"name":"VV","unit":"1","scale":1.0,"offset":0.0},
                "raster_bands":[{"scale":1.0,"offset":0.0,"unit":"1"}],
                "quality_policies":["opera_rtc_finite"]
            })],
            json!({"profile":"test"}),
        );
        prepared.fetch().unwrap();
        let attrs: Value = serde_json::from_slice(
            &fs::read(dest.join("observations/000000/values/.zattrs")).unwrap(),
        )
        .unwrap();
        assert_eq!(attrs["units"], "1");
        assert_eq!(attrs["scale_factor"], 1.0);
        assert_eq!(attrs["add_offset"], 0.0);
        assert_eq!(attrs["product_band"]["name"], "VV");
        assert_eq!(attrs["quality_policies"][0], "opera_rtc_finite");

        let store = Arc::new(FilesystemStore::new(&dest).unwrap());
        let mask = Array::open(store, "/observations/000000/quality_opera_rtc_finite").unwrap();
        let mask = mask
            .retrieve_array_subset::<Vec<u8>>(&mask.subset_all())
            .unwrap();
        assert_eq!(&mask[..4], &[0, 0, 0, 1]);
        fs::remove_dir_all(r).unwrap();
    }
    #[cfg(all(feature = "native-cog", not(feature = "gdal")))]
    #[test]
    fn annual_output_preserves_actual_subyear_coverage_and_fractional_seconds() {
        use zarrs::filesystem::FilesystemStore;
        let root = root();
        let dest = root.join("annual.zarr");
        let start = "2025-04-20T17:49:36.820Z";
        let end = "2025-08-23T17:49:16.708Z";
        let mut request = typed_product_request("radar_float32.tif", &dest);
        let Source::Assets { assets } = &mut request.source else {
            unreachable!()
        };
        assets[0].datetime = Some(start.into());
        let extent = json!({"coverage_start":start,"coverage_end":end,"summary_year":2025});
        let prepared = crate::acquisition::prepare(&request, &mut crate::cog::LayoutCache::default()).unwrap()
            .with_product_metadata(vec![json!({"asset_id":"typed-product","temporal_extent":extent,"time_coordinate_role":"source_coverage_start"})], json!({"profile":"test"}));
        let result = prepared.fetch().unwrap();
        assert_eq!(result["outputs"][0]["temporal_extent"], extent);
        let store = Arc::new(FilesystemStore::new(&dest).unwrap());
        let time = Array::open(store.clone(), "/observations/000000/time").unwrap();
        let time_values = time
            .retrieve_array_subset::<Vec<i64>>(&time.subset_all())
            .unwrap();
        let bounds = Array::open(store, "/observations/000000/time_bounds").unwrap();
        let bounds_values = bounds
            .retrieve_array_subset::<Vec<i64>>(&bounds.subset_all())
            .unwrap();
        let expected: Vec<i64> = [start, end]
            .iter()
            .map(|value| {
                chrono::DateTime::parse_from_rfc3339(value)
                    .unwrap()
                    .timestamp_nanos_opt()
                    .unwrap()
            })
            .collect();
        assert_eq!(bounds_values, expected);
        assert_eq!(time_values, [expected[0]]);
        let attrs: Value = serde_json::from_slice(
            &fs::read(dest.join("observations/000000/time/.zattrs")).unwrap(),
        )
        .unwrap();
        assert_eq!(attrs["bounds"], "time_bounds");
        assert_eq!(attrs["time_coordinate_role"], "source_coverage_start");
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(all(feature = "native-cog", not(feature = "gdal")))]
    #[test]
    fn product_semantic_fill_is_independent_from_physical_nodata_and_raw_values() {
        use zarrs::filesystem::FilesystemStore;
        let root = root();
        let profile = crate::products::profile::profile("landsat-c2-l2-sr").unwrap();
        for (fixture, band_name) in [
            ("qa_uint16_no_nodata.tif", "QA_PIXEL"),
            ("qa_uint16_allzero_no_nodata.tif", "QA_RADSAT"),
        ] {
            let destination = root.join(format!("{band_name}.zarr"));
            let band = profile.band(band_name).unwrap();
            let prepared = crate::acquisition::prepare(
                &typed_product_request(fixture, &destination),
                &mut crate::cog::LayoutCache::default(),
            )
            .unwrap();
            assert_eq!(
                prepared.plan().selected[0].layout.as_ref().unwrap().nodata,
                None
            );
            let prepared = prepared.with_product_metadata(
                vec![json!({
                    "asset_id":"typed-product",
                    "product":profile.id,
                    "product_band":band,
                    "raster_bands":[{"scale":band.scale,"offset":band.offset,"unit":band.unit}],
                    "quality_policies":[]
                })],
                json!({"profile":profile.id}),
            );
            prepared.fetch().unwrap();

            let store = Arc::new(FilesystemStore::new(&destination).unwrap());
            let values = Array::open(store.clone(), "/observations/000000/values").unwrap();
            let values = values
                .retrieve_array_subset::<Vec<u16>>(&values.subset_all())
                .unwrap();
            let valid = Array::open(store, "/observations/000000/valid").unwrap();
            let valid = valid
                .retrieve_array_subset::<Vec<u8>>(&valid.subset_all())
                .unwrap();
            let attrs: Value = serde_json::from_slice(
                &fs::read(destination.join("observations/000000/values/.zattrs")).unwrap(),
            )
            .unwrap();
            assert_eq!(attrs["source_nodata"], Value::Null);
            assert_eq!(
                attrs["missingness"]["physical_source"]["layout_nodata"],
                Value::Null
            );
            assert_eq!(attrs["missingness"]["product"]["band"], band_name);
            if band_name == "QA_PIXEL" {
                assert_eq!(&values[..4], &[1000, 1001, 1002, 1003]);
                assert_eq!(&valid[..4], &[1, 0, 1, 0]);
                assert_eq!(
                    attrs["missingness"]["product"]["nodata"],
                    json!({"type":"uint16","value":1})
                );
                assert!(attrs["missingness"]["product"]["fill_rule"]
                    .as_str()
                    .unwrap()
                    .contains("bit 0"));
            } else {
                assert!(values.iter().all(|value| *value == 0));
                assert!(valid.iter().all(|value| *value == 1));
                assert_eq!(attrs["missingness"]["product"]["nodata"], Value::Null);
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(all(feature = "native-cog", not(feature = "gdal")))]
    #[test]
    fn undefined_typed_quality_code_aborts_without_publication() {
        let r = root();
        let dest = r.join("invalid.zarr");
        let prepared = crate::acquisition::prepare(
            &typed_product_request("classification_uint8.tif", &dest),
            &mut crate::cog::LayoutCache::default(),
        )
        .unwrap()
        .with_product_metadata(
            vec![json!({
                "asset_id":"typed-product",
                "product_band":{"name":"Fmask","unit":"1"},
                "quality_policies":["hls_fmask_clear"]
            })],
            json!({"profile":"test"}),
        );
        assert!(prepared.fetch().is_err());
        assert!(!dest.exists());
        assert_eq!(fs::read_dir(&r).unwrap().count(), 0);
        fs::remove_dir_all(r).unwrap();
    }
    #[test]
    fn destination_race_never_replaces_an_existing_directory() {
        let r = root();
        let dest = r.join("result.zarr");
        let tx = Transaction::new(&dest).unwrap();
        fs::write(tx.stage().join("manifest.json"), b"complete").unwrap();
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
        fs::write(tx.stage().join("provisional"), b"data").unwrap();
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
            fs::write(tx.stage().join("manifest.json"), b"complete").unwrap();
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
        let store = FilesystemStore::new_with_options(tx.stage(), options).unwrap();
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
