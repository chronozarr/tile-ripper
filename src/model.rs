use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub source: Source,
    pub spatial: Spatial,
    pub time: Option<Time>,
    pub variables: Vec<Variable>,
    #[serde(default)]
    pub quality: Vec<QualityFilter>,
    pub grid: Grid,
    #[serde(default)]
    pub operations: Vec<Transform>,
    pub sink: Sink,
    pub budget: Budget,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    Assets {
        assets: Vec<Asset>,
    },
    Stac {
        endpoint: String,
        collection: String,
        #[serde(default)]
        limits: CatalogLimits,
    },
    Catalog {
        endpoint: String,
        collection: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    pub id: String,
    pub locator: String,
    pub format: Format,
    pub datetime: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Cog,
    ZarrV2,
    ZarrV3,
    Hdf5,
}
impl Format {
    pub(crate) fn is_zarr(self) -> bool {
        matches!(self, Self::ZarrV2 | Self::ZarrV3)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Spatial {
    /// Rank-two stored array selection in dimension order [row, column].
    /// Axis paths are explicit; no affine geolocation or CF inference is implied.
    ArraySlice {
        start: [usize; 2],
        count: [usize; 2],
        step: [usize; 2],
        axes: [String; 2],
    },
    Bounds {
        crs: String,
        bbox: [f64; 4],
    },
    Polygon {
        crs: String,
        coordinates: serde_json::Value,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Time {
    Interval { start: String, end: String },
    Instants { values: Vec<String> },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Variable {
    Named { name: String },
    Band { index: usize },
    Dataset { path: String },
    Polarization { name: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Grid {
    Native,
    Target {
        crs: String,
        resolution: [f64; 2],
        resampling: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Sink {
    /// Preserve each source format and native grid.
    Directory {
        path: String,
    },
    Callback,
    ZarrDirectory {
        path: String,
    },
    #[serde(rename = "geotiff_directory")]
    GeoTiffDirectory {
        path: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub buffer_bytes: usize,
    pub max_nodes: usize,
    pub chunk_edge: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capabilities {
    pub selection: Vec<String>,
    pub reads: Vec<String>,
    pub decoding: String,
    pub compression: String,
    pub materialization: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tile {
    pub x: usize,
    pub y: usize,
    pub offset: Option<u64>,
    pub bytes: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub transform: [f64; 6],
    pub crs: String,
    pub variable: Variable,
    pub block: [usize; 2],
    pub dtype: String,
    pub nodata: Option<f64>,
    pub scale: Option<f64>,
    pub offset: Option<f64>,
    pub tiles: Vec<Tile>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Read {
    LibraryWindow {
        window: Window,
        band: usize,
        tiles: Vec<Tile>,
    },
    ChunkObject {
        key: String,
        indices: Vec<usize>,
    },
    ByteRange {
        object_id: String,
        start: u64,
        length: u64,
    },
    WholeObject,
    DatasetSlice {
        path: String,
        start: Vec<usize>,
        count: Vec<usize>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    Inspect { cached: bool },
    Read { read: Read },
    Decode { reader: String },
    Write { output: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: usize,
    pub dependencies: Vec<usize>,
    pub asset_id: String,
    pub operation: Operation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Selected {
    #[serde(default)]
    pub timestamps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<DimensionSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_geometry: Option<serde_json::Value>,
    pub asset_id: String,
    pub datetime: Option<String>,
    pub reason: String,
    pub capabilities: Capabilities,
    pub layout: Option<Layout>,
    pub window: Window,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Estimates {
    pub selected_compressed_payload_bytes: Option<u64>,
    pub remote_requests: Option<u64>,
    pub note: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub schema_version: u32,
    pub selected: Vec<Selected>,
    pub nodes: Vec<Node>,
    pub estimates: Estimates,
    pub limitations: Vec<String>,
}

/// Declared semantics for future adapters. The initial executor rejects these.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QualityFilter {
    Metadata {
        field: String,
        comparison: Comparison,
        value: f64,
    },
    PixelMask {
        variable: Variable,
        accepted_values: Vec<u64>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    LessThan,
    LessOrEqual,
    Equal,
    GreaterOrEqual,
    GreaterThan,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Transform {
    Mosaic { method: MosaicMethod },
    Composite { period: String, reducer: Reducer },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MosaicMethod {
    FirstValid,
    LastValid,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reducer {
    Mean,
    Median,
    Min,
    Max,
    Count,
}

/// Native multidimensional selection; axis names and indices have no format semantics.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DimensionSelection {
    pub shape: Vec<usize>,
    pub chunk_shape: Vec<usize>,
    pub names: Vec<String>,
    pub indices: Vec<Vec<usize>>,
}

/// Catalog/provider limits. Pixel buffers and DAG limits live only in Budget.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogLimits {
    pub max_items: usize,
    pub page_size: usize,
    pub max_assets: usize,
    pub max_output_pixels: u64,
    pub gdal_cache_mb: usize,
    pub max_retries: u32,
    pub timeout_seconds: u64,
}
impl Default for CatalogLimits {
    fn default() -> Self {
        Self {
            max_items: 32,
            page_size: 8,
            max_assets: 64,
            max_output_pixels: 16_000_000,
            gdal_cache_mb: 32,
            max_retries: 2,
            timeout_seconds: 45,
        }
    }
}
