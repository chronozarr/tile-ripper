use crate::{
    cog::{self, LayoutCache},
    fail,
    model::*,
    plan, Result,
};
#[cfg(feature = "gdal")]
use gdal::{raster::Buffer, DriverManager};
use serde::{Deserialize, Serialize};
use std::time::Instant;
#[cfg(feature = "gdal")]
use std::{fs, path::Path};

/// One owned native-grid buffer. Sink callbacks run synchronously: retaining chunks
/// is the caller's responsibility and outside the executor's buffer bound.
pub struct Chunk {
    pub node_id: usize,
    pub asset_id: String,
    pub datetime: Option<String>,
    pub variable: Variable,
    /// Source indices of nonspatial axes, in the selected dimension order.
    pub indices: Vec<usize>,
    pub window: Window,
    pub transform: [f64; 6],
    pub crs: String,
    pub nodata: Option<f64>,
    pub scale: Option<f64>,
    pub offset: Option<f64>,
    pub pixels: crate::Pixels,
    /// Actual rectilinear native centers, when an affine is only approximate.
    pub coordinates: Option<(Vec<f64>, Vec<f64>)>,
}
impl Chunk {
    /// Pixel-center coordinates in this chunk's native CRS, without resampling.
    /// `column` and `row` are relative to the delivered buffer, not the source.
    pub fn pixel_center(&self, column: usize, row: usize) -> Option<[f64; 2]> {
        if column >= self.window.width || row >= self.window.height {
            return None;
        }
        if let Some((x, y)) = &self.coordinates {
            return Some([x[column], y[row]]);
        }
        let x = column as f64 + 0.5;
        let y = row as f64 + 0.5;
        let t = self.transform;
        Some([t[0] + x * t[1] + y * t[2], t[3] + x * t[4] + y * t[5]])
    }
    /// Validity of each stored value; COG AOI exclusions already carry nodata.
    pub fn validity(&self) -> impl Iterator<Item = bool> + '_ {
        (0..self.pixels.len()).map(|i| self.pixels.valid(i, self.nodata))
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionReport {
    #[serde(default)]
    pub source_reads: Vec<SourceRead>,
    #[serde(default)]
    pub scheduler: serde_json::Value,
    #[serde(default)]
    pub transport: serde_json::Value,
    #[serde(default)]
    pub decoded_source_chunks: usize,
    #[serde(default)]
    pub filled_source_chunks: usize,
    #[serde(default)]
    pub first_output_seconds: Option<f64>,
    #[serde(default)]
    pub assembly_seconds: f64,
    #[serde(default)]
    pub source_retrieval_seconds: Option<f64>,
    #[serde(default)]
    pub allocation_seconds: Option<f64>,
    #[serde(default)]
    pub copying_seconds: Option<f64>,
    #[serde(default)]
    pub output_setup_seconds: f64,
    #[serde(default)]
    pub output_finalize_seconds: f64,
    #[serde(default)]
    pub output_io: Option<OutputIo>,
    pub native_materialized_bytes: Option<usize>,
    /// Observed data-chunk store calls and returned encoded bytes (metadata excluded).
    pub source_data_requests: Option<u64>,
    pub source_data_bytes: Option<u64>,
    pub chunks: usize,
    pub decoded_bytes: usize,
    pub non_nodata_pixels: usize,
    pub planning_seconds: f64,
    pub source_open_seconds: f64,
    pub read_decode_seconds: f64,
    pub sink_seconds: f64,
    pub masking_seconds: f64,
    pub remote_requests: Option<u64>,
    pub remote_bytes: Option<u64>,
}
/// Native output-store operations, including coordinates and metadata, excluding
/// manifests. Counts library calls/encoded bytes, not physical device traffic.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OutputIo {
    pub reads: usize,
    pub writes: usize,
    pub bytes_read: usize,
    pub bytes_written: usize,
    pub buffered_bytes_peak: usize,
    pub storage_read_seconds: f64,
    pub storage_write_seconds: f64,
    /// Full-chunk encoding/serialization/library overhead, excluding store I/O.
    pub encode_seconds: f64,
    pub allocation_seconds: f64,
    pub copying_seconds: f64,
}
/// Replan from the runtime request. Diagnostic JSON plans are never replayed.
pub fn execute(
    r: &Request,
    cache: &mut LayoutCache,
    sink: impl FnMut(Chunk) -> Result<()>,
) -> Result<ExecutionReport> {
    if !matches!(r.source, Source::Assets { .. })
        || matches!(r.sink, Sink::Directory { .. })
        || crate::storage::has_remote_zarr(r)
        || matches!(&r.source, Source::Assets {assets} if assets.iter().any(|a| a.format == Format::Cog && cog::remote(&a.locator)))
        || (cog::native_selected()
            && matches!(&r.source, Source::Assets {assets} if assets.iter().any(|a| a.format == Format::Cog)))
    {
        return crate::acquisition::prepare(r, cache)?.execute(sink);
    }
    let t = Instant::now();
    let p = plan(r, cache)?;
    execute_plan(r, &p, t.elapsed().as_secs_f64(), sink)
}
pub(crate) fn execute_plan(
    r: &Request,
    p: &Plan,
    planning_seconds: f64,
    sink: impl FnMut(Chunk) -> Result<()>,
) -> Result<ExecutionReport> {
    execute_on(
        r,
        p,
        planning_seconds,
        &crate::storage::Sources::default(),
        sink,
    )
}
pub(crate) fn execute_on(
    r: &Request,
    p: &Plan,
    planning_seconds: f64,
    sources: &crate::storage::Sources,
    sink: impl FnMut(Chunk) -> Result<()>,
) -> Result<ExecutionReport> {
    crate::scheduler::execute(r, p, planning_seconds, sources, &Default::default(), sink)
}
#[cfg(feature = "gdal")]
#[path = "cog_gdal_reader.rs"]
mod gdal_reader;
/// Internal backend boundary. Public chunks and final validation stay shared.
#[allow(dead_code)]
pub(crate) enum SelectedCogReader {
    #[cfg(not(any(feature = "gdal", feature = "native-cog")))]
    Unavailable,
    #[cfg(feature = "gdal")]
    Gdal(gdal_reader::GdalCogReader),
    #[cfg(feature = "native-cog")]
    Native(crate::native_cog::SelectedReader),
}
impl SelectedCogReader {
    #[allow(unused_variables)]
    pub(crate) fn finish(&self, report: &mut ExecutionReport) {
        match self {
            #[cfg(feature = "native-cog")]
            Self::Native(reader) => reader.finish(report),
            #[cfg(feature = "gdal")]
            Self::Gdal(_) => {}
            #[cfg(not(any(feature = "gdal", feature = "native-cog")))]
            Self::Unavailable => {}
        }
    }
    #[allow(unused_variables)]
    pub(crate) fn new(
        asset: &Asset,
        selected: &Selected,
        sources: &crate::storage::Sources,
        #[cfg(feature = "native-cog")] strategy: crate::native_cog::Strategy,
    ) -> Result<Self> {
        match crate::cog::backend()? {
            #[cfg(feature = "gdal")]
            crate::cog::Backend::Gdal => Ok(Self::Gdal(gdal_reader::GdalCogReader::new(
                asset, selected,
            )?)),
            #[cfg(feature = "native-cog")]
            crate::cog::Backend::Native => Ok(Self::Native(
                crate::native_cog::SelectedReader::new(asset, selected, strategy, &sources.1)?,
            )),
        }
    }
    pub(crate) fn execute_node(
        &mut self,
        node: &Node,
        report: &mut ExecutionReport,
        sink: &mut impl FnMut(Chunk) -> Result<()>,
    ) -> Result<()> {
        match self {
            #[cfg(not(any(feature = "gdal", feature = "native-cog")))]
            Self::Unavailable => {
                let _ = (node, report, sink);
                fail("COG backend unavailable")
            }
            #[cfg(feature = "gdal")]
            Self::Gdal(r) => r.execute_node(node, report, sink),
            #[cfg(feature = "native-cog")]
            Self::Native(r) => r.execute_node(node, report, sink),
        }
    }
}
pub fn fetch(r: &Request, cache: &mut LayoutCache) -> Result<ExecutionReport> {
    if !matches!(r.source, Source::Assets { .. })
        || matches!(r.sink, Sink::Directory { .. })
        || crate::storage::has_remote_zarr(r)
        || matches!(&r.source, Source::Assets {assets} if assets.iter().any(|a| a.format == Format::Cog && cog::remote(&a.locator)))
        || (cog::native_selected()
            && matches!(&r.source, Source::Assets {assets} if assets.iter().any(|a| a.format == Format::Cog)))
    {
        return Ok(serde_json::from_value(
            crate::acquisition::fetch(r, cache)?["observed"].clone(),
        )?);
    }
    if matches!(r.sink, Sink::ZarrDirectory { .. }) {
        return crate::zarr_acquire::fetch(r, cache);
    }
    let Sink::GeoTiffDirectory { path } = &r.sink else {
        return fail("fetch requires a GeoTIFF directory sink; library callers may use execute with a callback");
    };
    let _ = path;
    // Refuse overwrites, and remove a newly created directory on failure.
    let start = Instant::now();
    let preview = plan(r, cache)?;
    let planning_seconds = start.elapsed().as_secs_f64();
    fetch_plan(r, &preview, planning_seconds)
}
pub(crate) fn fetch_plan(
    r: &Request,
    preview: &Plan,
    planning_seconds: f64,
) -> Result<ExecutionReport> {
    fetch_on(
        r,
        preview,
        planning_seconds,
        &crate::storage::Sources::default(),
    )
}
pub(crate) fn fetch_on(
    r: &Request,
    preview: &Plan,
    planning_seconds: f64,
    sources: &crate::storage::Sources,
) -> Result<ExecutionReport> {
    if matches!(r.sink, Sink::ZarrDirectory { .. }) {
        return crate::zarr_acquire::fetch_on(r, preview, planning_seconds, sources);
    }
    let Sink::GeoTiffDirectory { path } = &r.sink else {
        return fail("fetch requires a native directory sink");
    };
    #[cfg(not(feature = "gdal"))]
    {
        let _ = (path, r, preview, planning_seconds, sources);
        fail("GeoTIFF output requires the gdal feature; native streaming and Zarr output do not")
    }
    #[cfg(feature = "gdal")]
    {
        let root = Path::new(path);
        if preview
            .selected
            .iter()
            .any(|s| !s.capabilities.materialization)
        {
            return fail("planning-only format cannot be materialized");
        }
        if preview.selected.iter().any(|s| s.dimensions.is_some()) {
            return fail("Zarr extraction requires native Zarr directory output");
        }
        fs::create_dir(root)?;
        let result = execute_plan(r, preview, planning_seconds, |mut chunk| {
            let file = root.join(format!("chunk-{}.tif", chunk.node_id));
            let mut ds = DriverManager::get_driver_by_name("GTiff")?
                .create_with_band_type::<u16, _>(
                    &file,
                    chunk.window.width,
                    chunk.window.height,
                    1,
                )?;
            ds.set_geo_transform(&chunk.transform)?;
            ds.set_spatial_ref(&gdal::spatial_ref::SpatialRef::from_definition(&chunk.crs)?)?;
            let mut b = ds.rasterband(1)?;
            b.set_no_data_value(chunk.nodata)?;
            if let Some(s) = chunk.scale {
                b.set_scale(s)?;
            }
            if let Some(o) = chunk.offset {
                b.set_offset(o)?;
            }
            b.write(
                (0, 0),
                (chunk.window.width, chunk.window.height),
                &mut Buffer::new(
                    (chunk.window.width, chunk.window.height),
                    std::mem::take(&mut chunk.pixels).into_u16()?,
                ),
            )?;
            ds.flush_cache()?;
            Ok(())
        });
        match result {
            Ok(report) => {
                // Per-chunk metadata maps outputs back to observation/band, not an implicit mosaic.
                let manifest = serde_json::json!({"plan":preview,"observed":report});
                if fs::write(
                    root.join("manifest.json"),
                    serde_json::to_vec_pretty(&manifest)?,
                )
                .is_err()
                {
                    let _ = fs::remove_dir_all(root);
                    return fail("could not write output manifest");
                }
                Ok(report)
            }
            Err(e) => {
                let _ = fs::remove_dir_all(root);
                Err(e)
            }
        }
    }
}

/// Observed data-object access; metadata and coordinate reads are excluded.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRead {
    pub asset_id: String,
    pub key: String,
    pub requests: u64,
    pub bytes: u64,
}

pub(crate) fn empty_report(planning_seconds: f64) -> ExecutionReport {
    ExecutionReport {
        source_reads: vec![],
        scheduler: serde_json::Value::Null,
        transport: serde_json::Value::Null,
        decoded_source_chunks: 0,
        filled_source_chunks: 0,
        first_output_seconds: None,
        assembly_seconds: 0.,
        source_retrieval_seconds: None,
        allocation_seconds: None,
        copying_seconds: None,
        output_setup_seconds: 0.,
        output_finalize_seconds: 0.,
        output_io: None,
        native_materialized_bytes: None,
        source_data_requests: None,
        source_data_bytes: None,
        chunks: 0,
        decoded_bytes: 0,
        non_nodata_pixels: 0,
        planning_seconds,
        source_open_seconds: 0.,
        read_decode_seconds: 0.,
        sink_seconds: 0.,
        masking_seconds: 0.,
        remote_requests: None,
        remote_bytes: None,
    }
}
