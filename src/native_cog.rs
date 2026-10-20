//! Narrow storage-domain UInt16 COG adapter. TIFF parsing/codecs are upstream.
use crate::{
    fail, Asset, Chunk, ExecutionReport, Layout, Node, Operation, Read, Result, Selected, Tile,
    Variable,
};
use geotiff_core::{CrsInfo, GeoKeyDirectory, GeoTransform, RasterType};
use std::{sync::Arc, time::Instant};
use tiff_reader::{Ifd, OpenOptions, ParseBudgets, TagValue, TiffFile};
#[path = "cog_range.rs"]
mod range;
use range::RangeSource;
const MAX_CACHE: usize = 8 * 1024 * 1024;
const MAX_TILE: usize = 2 * 1024 * 1024;

fn cache_bytes(l: &Layout, w: &crate::Window) -> Result<usize> {
    let tiles = crate::cog::tiles(l, w).len();
    l.block[0]
        .checked_mul(l.block[1])
        .and_then(|n| n.checked_mul(2))
        .and_then(|n| n.checked_mul(tiles))
        .map(|n| n.min(MAX_CACHE))
        .ok_or_else(|| "native COG cache reservation overflow".into())
}
pub(crate) fn reservation(l: &Layout, w: &crate::Window) -> Result<(usize, usize)> {
    let tile = l.block[0]
        .checked_mul(l.block[1])
        .and_then(|n| n.checked_mul(2))
        .filter(|n| *n <= MAX_TILE)
        .ok_or("native COG tile exceeds 2 MiB decode limit")?;
    let encoded = l
        .tiles
        .iter()
        .map(|t| t.bytes.unwrap_or(0))
        .max()
        .unwrap_or(0) as usize;
    // Upstream keeps a cached decoded block, a decode buffer, a typed window,
    // and encoded input. Metadata/runtime allocations are reported separately.
    Ok((
        cache_bytes(l, w)?,
        tile.checked_mul(2)
            .and_then(|n| n.checked_add(encoded))
            .ok_or("native COG working set overflow")?,
    ))
}
fn open(a: &Asset, cache: usize) -> Result<(TiffFile, Arc<RangeSource>)> {
    let source = RangeSource::open(a)?;
    let tiff = TiffFile::from_source_with_options(
        source.clone(),
        OpenOptions {
            block_cache_bytes: cache,
            block_cache_slots: 64,
            parse_budgets: ParseBudgets {
                max_ifds: 16,
                max_ifd_entries: 256,
                max_tag_value_bytes: 1024 * 1024,
                max_metadata_value_bytes: 2 * 1024 * 1024,
            },
            decode_output_bytes: MAX_TILE,
        },
    )?;
    Ok((tiff, source))
}
fn doubles(ifd: &Ifd, tag: u16) -> Result<Vec<f64>> {
    match ifd.tag(tag).map(|t| &t.value) {
        Some(TagValue::Double(v)) => Ok(v.clone()),
        _ => fail("native COG missing/invalid DOUBLE spatial tag"),
    }
}
fn layout(tiff: &TiffFile, band: usize) -> Result<Layout> {
    if band != 1 {
        return fail("native COG supports one UInt16 band (index 1)");
    }
    let i = tiff.ifd(0)?;
    if !i.is_tiled()
        || i.width() == 0
        || i.height() == 0
        || i.samples_per_pixel() != 1
        || i.bits_per_sample()? != [16]
        || i.sample_format()? != [1]
        || i.planar_configuration() != 1
        || i.photometric_interpretation() != Some(1)
        || !i.extra_samples()?.is_empty()
        || i.tag(274).is_some_and(|t| t.value.as_u16() != Some(1))
    {
        return fail("native COG requires tiled single-band MinIsBlack UInt16, top-left orientation, no alpha");
    }
    if !matches!(i.compression(), 1 | 5 | 8 | 32946) || !matches!(i.predictor(), 1 | 2) {
        return fail(
            "native COG supports lossless uncompressed/LZW/Deflate with predictor 1/2 only",
        );
    }
    // Never silently lose internal masks or follow unbounded auxiliary trees.
    if tiff.ifds().iter().any(|d| {
        d.tag(330).is_some()
            || d.tag(254)
                .and_then(|t| t.value.as_u32())
                .is_some_and(|v| v & 4 != 0)
    }) {
        return fail("native COG explicit masks and SubIFDs are unsupported");
    }
    if i.tag(254).and_then(|t| t.value.as_u32()).unwrap_or(0) != 0 {
        return fail("native COG first IFD must be full resolution");
    }
    let (scale_value, offset_value) = calibration(i)?;
    if i.tag(34264).is_some() {
        return fail("native COG transformation matrices are outside the validated profile");
    }
    let dir = i
        .tag(34735)
        .and_then(|t| t.value.as_u16_slice())
        .ok_or("native COG missing GeoKey directory")?;
    let double = i
        .tag(34736)
        .map(|_| doubles(i, 34736))
        .transpose()?
        .unwrap_or_default();
    let ascii = i
        .tag(34737)
        .map(|t| t.value.as_str().ok_or("invalid GeoASCII tag"))
        .transpose()?
        .unwrap_or("");
    let keys = GeoKeyDirectory::parse(dir, &double, ascii).ok_or("native COG malformed GeoKeys")?;
    let crs = CrsInfo::from_geokeys(&keys);
    let epsg = crs
        .epsg()
        .filter(|n| *n != 0 && *n != 32767)
        .ok_or("native COG requires an EPSG CRS")?;
    if !matches!(keys.get_short(1024), Some(1 | 2)) || keys.get_short(4096).is_some() {
        return fail("native COG unsupported compound/geocentric CRS");
    }
    if !matches!(keys.get_short(1025), None | Some(1)) {
        return fail("native COG PixelIsPoint is outside the validated profile");
    }
    let scale = doubles(i, 33550)?;
    let tie = doubles(i, 33922)?;
    if scale.len() != 3
        || tie.len() != 6
        || !scale.iter().chain(&tie).all(|v| v.is_finite())
        || scale[0] <= 0.
        || scale[1] <= 0.
        || tie[2] != 0.
        || tie[5] != 0.
    {
        return fail("native COG requires finite north-up scale and one 2D tiepoint");
    }
    let g = GeoTransform::from_tiepoint_and_scale_with_raster_type(
        &tie.try_into().unwrap(),
        &scale.try_into().unwrap(),
        RasterType::PixelIsArea,
    );
    let transform = [
        g.origin_x,
        g.pixel_width,
        g.skew_x,
        g.origin_y,
        g.skew_y,
        g.pixel_height,
    ];
    if !transform.iter().all(|v| v.is_finite()) {
        return fail("native COG nonfinite affine");
    }
    let bx = i
        .tile_width()
        .filter(|n| *n > 0)
        .ok_or("missing tile width")? as usize;
    let by = i
        .tile_height()
        .filter(|n| *n > 0)
        .ok_or("missing tile height")? as usize;
    let count = (i.width() as usize)
        .div_ceil(bx)
        .checked_mul((i.height() as usize).div_ceil(by))
        .filter(|n| *n <= 65_536)
        .ok_or("native COG tile metadata budget exceeded")?;
    if bx
        .checked_mul(by)
        .and_then(|n| n.checked_mul(2))
        .is_none_or(|n| n > MAX_TILE)
    {
        return fail("native COG tile exceeds 2 MiB decode limit");
    }
    let offsets = i.tile_offsets().ok_or("missing tile offsets")?;
    let counts = i.tile_byte_counts().ok_or("missing tile counts")?;
    if offsets.len() != count || counts.len() != count {
        return fail("native COG inconsistent tile arrays");
    }
    let mut tiles = Vec::with_capacity(count);
    for (n, (offset, bytes)) in offsets.into_iter().zip(counts).enumerate() {
        if bytes == 0
            || offset == 0
            || bytes > MAX_TILE as u64 + 65536
            || offset
                .checked_add(bytes)
                .is_none_or(|end| end > tiff.source().len())
        {
            return fail("native COG sparse/oversized/out-of-bounds tile");
        }
        tiles.push(Tile {
            x: n % (i.width() as usize).div_ceil(bx),
            y: n / (i.width() as usize).div_ceil(bx),
            offset: Some(offset),
            bytes: Some(bytes),
        });
    }
    let nodata = i
        .tag(42113)
        .map(|t| -> Result<f64> {
            let n = t
                .value
                .as_str()
                .ok_or("invalid nodata tag")?
                .trim_end_matches('\0')
                .parse::<f64>()?;
            if !n.is_finite() || n.fract() != 0. || !(0. ..=65535.).contains(&n) {
                return fail("native COG nodata must be representable UInt16");
            }
            Ok(n)
        })
        .transpose()?;
    Ok(Layout {
        width: i.width() as usize,
        height: i.height() as usize,
        transform,
        crs: format!("EPSG:{epsg}"),
        variable: Variable::Band { index: 1 },
        block: [bx, by],
        dtype: "UInt16".into(),
        nodata,
        scale: scale_value,
        offset: offset_value,
        tiles,
    })
}
// Preserve scientific calibration without applying it to native samples.
fn calibration(i: &Ifd) -> Result<(Option<f64>, Option<f64>)> {
    let Some(tag) = i.tag(42112) else {
        return Ok((None, None));
    };
    let xml = tag
        .value
        .as_str()
        .ok_or("native COG invalid GDAL metadata XML")?
        .trim_end_matches('\0');
    let doc =
        roxmltree::Document::parse(xml).map_err(|_| "native COG malformed GDAL metadata XML")?;
    if doc.root_element().tag_name().name() != "GDALMetadata" {
        return fail("native COG unexpected calibration XML root");
    }
    let (mut scale, mut offset) = (None, None);
    for n in doc.root_element().children().filter(|n| n.is_element()) {
        if n.tag_name().name() != "Item" {
            return fail("native COG unsupported calibration XML element");
        }
        if let Some(role) = n.attribute("role") {
            let slot = match role {
                "scale" => &mut scale,
                "offset" => &mut offset,
                _ => return fail("native COG unsupported GDAL metadata role"),
            };
            if n.attribute("sample") != Some("0")
                || slot.is_some()
                || n.children().any(|n| n.is_element())
            {
                return fail("native COG ambiguous calibration metadata");
            }
            let v = n
                .text()
                .ok_or("empty calibration value")?
                .trim()
                .parse::<f64>()?;
            if !v.is_finite() {
                return fail("nonfinite calibration value");
            }
            *slot = Some(v);
        }
    }
    Ok((scale, offset))
}
pub(crate) fn inspect(a: &Asset, band: usize) -> Result<Layout> {
    let (t, _) = open(a, 0)?;
    layout(&t, band)
}
pub(crate) struct SelectedReader {
    asset: Asset,
    tiff: TiffFile,
    source: Arc<RangeSource>,
    layout: Layout,
    #[cfg(feature = "gdal")]
    mask: Option<gdal::vector::Geometry>,
}
impl SelectedReader {
    pub(crate) fn new(asset: &Asset, s: &Selected) -> Result<Self> {
        let expected = s.layout.as_ref().ok_or("missing native COG layout")?;
        let (tiff, source) = open(asset, cache_bytes(expected, &s.window)?)?;
        let layout = layout(&tiff, 1)?;
        if serde_json::to_value(&layout)? != serde_json::to_value(expected)? {
            return fail("native COG metadata changed since planning");
        }
        #[cfg(feature = "gdal")]
        let mask = s
            .mask_geometry
            .as_ref()
            .map(|g| gdal::vector::Geometry::from_geojson(&g.to_string()))
            .transpose()?;
        #[cfg(not(feature = "gdal"))]
        if s.mask_geometry.is_some() {
            return fail("polygon masks require gdal");
        }
        source.begin_data();
        Ok(Self {
            asset: asset.clone(),
            tiff,
            source,
            layout,
            #[cfg(feature = "gdal")]
            mask,
        })
    }
    pub(crate) fn execute_node(
        &mut self,
        node: &Node,
        report: &mut ExecutionReport,
        sink: &mut impl FnMut(Chunk) -> Result<()>,
    ) -> Result<()> {
        let Operation::Read {
            read: Read::LibraryWindow {
                window, band: 1, ..
            },
        } = &node.operation
        else {
            return Ok(());
        };
        if node.asset_id != self.asset.id {
            return fail("native COG node binding mismatch");
        }
        let before = self.source.counters.lock().unwrap().clone();
        let t = Instant::now();
        let values =
            self.tiff
                .read_window::<u16>(0, window.y, window.x, window.height, window.width)?;
        #[allow(unused_mut)]
        let (mut values, _) = values.into_raw_vec_and_offset();
        let elapsed = t.elapsed().as_secs_f64();
        let after = self.source.counters.lock().unwrap().clone();
        report.read_decode_seconds += elapsed;
        report.source_retrieval_seconds =
            Some(report.source_retrieval_seconds.unwrap_or(0.) + after.seconds - before.seconds);
        report.source_data_requests =
            Some(report.source_data_requests.unwrap_or(0) + after.requests - before.requests);
        report.source_data_bytes =
            Some(report.source_data_bytes.unwrap_or(0) + after.bytes - before.bytes);
        #[cfg(feature = "gdal")]
        if let Some(g) = &self.mask {
            let t = Instant::now();
            let m = crate::spatial::mask(g, &self.layout, window)?;
            let n = self.layout.nodata.ok_or("mask requires nodata")? as u16;
            for (v, keep) in values.iter_mut().zip(m.data()) {
                if *keep == 0 {
                    *v = n;
                }
            }
            report.masking_seconds += t.elapsed().as_secs_f64();
        }
        report.decoded_bytes += values.len() * 2;
        report.non_nodata_pixels += values
            .iter()
            .filter(|v| self.layout.nodata.is_none_or(|n| f64::from(**v) != n))
            .count();
        let c = Chunk {
            node_id: node.id,
            asset_id: self.asset.id.clone(),
            datetime: self.asset.datetime.clone(),
            variable: self.layout.variable.clone(),
            indices: vec![],
            window: window.clone(),
            transform: crate::spatial::window_transform(&self.layout, window),
            crs: self.layout.crs.clone(),
            nodata: self.layout.nodata,
            scale: self.layout.scale,
            offset: self.layout.offset,
            pixels: values.into(),
            coordinates: None,
        };
        let t = Instant::now();
        sink(c)?;
        report.sink_seconds += t.elapsed().as_secs_f64();
        report.chunks += 1;
        Ok(())
    }
}
