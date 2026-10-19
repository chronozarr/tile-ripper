use crate::{
    cog::{self, LayoutCache},
    fail,
    model::*,
    plan, Result,
};
use gdal::{raster::Buffer, DriverManager};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, time::Instant};

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
    pub pixels: Vec<u16>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionReport {
    #[serde(default)]
    pub source_reads: Vec<SourceRead>,
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
/// Replan from the runtime request. Diagnostic JSON plans are never replayed.
pub fn execute(
    r: &Request,
    cache: &mut LayoutCache,
    sink: impl FnMut(Chunk) -> Result<()>,
) -> Result<ExecutionReport> {
    if !matches!(r.source, Source::Assets { .. }) || matches!(r.sink, Sink::Directory { .. }) {
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
    mut sink: impl FnMut(Chunk) -> Result<()>,
) -> Result<ExecutionReport> {
    let _quiet = crate::diagnostics::QuietNative::enter();
    // Preflight every format before delivering or writing the first chunk.
    if p.selected.iter().any(|s| !s.capabilities.materialization) {
        return fail("plan contains a planning-only format; execution is unsupported");
    }
    let Source::Assets { assets } = &r.source else {
        unreachable!()
    };
    let mut report = ExecutionReport {
        source_reads: vec![],
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
    };
    for selected in &p.selected {
        let asset = assets
            .iter()
            .find(|a| a.id == selected.asset_id)
            .ok_or("asset binding missing")?;
        if asset.format == Format::ZarrV2 {
            crate::zarr_acquire::execute_selected(r, p, asset, selected, &mut report, &mut sink)?;
            continue;
        }
        let layout = selected.layout.as_ref().ok_or("missing layout")?;
        let mask_geometry = selected
            .mask_geometry
            .as_ref()
            .map(|v| gdal::vector::Geometry::from_geojson(&v.to_string()))
            .transpose()?;
        // A different prepared job may have populated GDAL's URL-only cache
        // since planning. Rebind the cached metadata/data to this snapshot.
        if asset.etag.is_some() {
            cog::clear_remote_cache(asset)?;
        }
        let _headers = cog::ConditionalHeaders::for_asset(asset)?;
        let open_started = Instant::now();
        let ds = cog::open(asset)?;
        if ds.raster_size() != (layout.width, layout.height)
            || ds.geo_transform()? != layout.transform
        {
            return fail("asset grid changed since planning");
        }
        let Variable::Band { index } = layout.variable else {
            return fail("COG band missing");
        };
        let band = ds.rasterband(index)?;
        report.source_open_seconds += open_started.elapsed().as_secs_f64();
        for node in &p.nodes {
            if node.asset_id != asset.id {
                continue;
            }
            let Operation::Read {
                read:
                    Read::LibraryWindow {
                        window,
                        band: band_index,
                        ..
                    },
            } = &node.operation
            else {
                continue;
            };
            if *band_index != index {
                continue;
            }
            let t = Instant::now();
            let mut pixels = band
                .read_as::<u16>(
                    (window.x as isize, window.y as isize),
                    (window.width, window.height),
                    (window.width, window.height),
                    None,
                )
                .map_err(|_| "GDAL window read failed (locator omitted)")?;
            report.read_decode_seconds += t.elapsed().as_secs_f64();
            let t = Instant::now();
            if let Some(g) = &mask_geometry {
                let mask = crate::spatial::mask(g, layout, window)?;
                let nodata = layout.nodata.ok_or("AOI mask requires nodata")? as u16;
                for (value, valid) in pixels.data_mut().iter_mut().zip(mask.data()) {
                    if *valid == 0 {
                        *value = nodata;
                    }
                }
            }
            report.masking_seconds += t.elapsed().as_secs_f64();
            report.decoded_bytes += pixels.data().len() * 2;
            report.non_nodata_pixels += pixels
                .data()
                .iter()
                .filter(|v| layout.nodata.is_none_or(|n| f64::from(**v) != n))
                .count();
            let mut transform = layout.transform;
            transform[0] += window.x as f64 * transform[1];
            transform[3] += window.y as f64 * transform[5];
            let chunk = Chunk {
                node_id: node.id,
                asset_id: asset.id.clone(),
                datetime: asset.datetime.clone(),
                variable: layout.variable.clone(),
                indices: vec![],
                window: window.clone(),
                transform,
                crs: layout.crs.clone(),
                nodata: layout.nodata,
                scale: layout.scale,
                offset: layout.offset,
                pixels: pixels.into_shape_and_vec().1,
            };
            let t = Instant::now();
            sink(chunk)?;
            report.sink_seconds += t.elapsed().as_secs_f64();
            report.chunks += 1;
        }
    }
    Ok(report)
}
pub fn fetch(r: &Request, cache: &mut LayoutCache) -> Result<ExecutionReport> {
    if !matches!(r.source, Source::Assets { .. }) || matches!(r.sink, Sink::Directory { .. }) {
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
    if matches!(r.sink, Sink::ZarrDirectory { .. }) {
        return crate::zarr_acquire::fetch_plan(r, preview, planning_seconds);
    }
    let Sink::GeoTiffDirectory { path } = &r.sink else {
        return fail("fetch requires a native directory sink");
    };
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
        let mut ds = DriverManager::get_driver_by_name("GTiff")?.create_with_band_type::<u16, _>(
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
                std::mem::take(&mut chunk.pixels),
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

/// Observed data-object access; metadata and coordinate reads are excluded.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRead {
    pub asset_id: String,
    pub key: String,
    pub requests: u64,
    pub bytes: u64,
}
