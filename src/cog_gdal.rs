use super::*;
use gdal::{Dataset, Metadata};
pub fn open(asset: &Asset) -> Result<Dataset> {
    let _quiet = crate::diagnostics::QuietNative::enter();
    let locator = if remote(&asset.locator) {
        format!("/vsicurl/{}", asset.locator)
    } else {
        asset.locator.clone()
    };
    Dataset::open(Path::new(&locator))
        .map_err(|_| "GDAL could not open asset (locator omitted)".into())
}
pub fn inspect(asset: &Asset, band: usize) -> Result<Layout> {
    let _quiet = crate::diagnostics::QuietNative::enter();
    let _headers = ConditionalHeaders::for_asset(asset)?;
    let ds = open(asset)?;
    if ds.driver().short_name() != "GTiff" {
        return fail("COG adapter requires GeoTIFF");
    }
    let b = ds.rasterband(band).map_err(|_| "invalid band")?;
    if b.band_type().name() != "UInt16" {
        return fail("prototype COG execution supports UInt16 only");
    }
    if !b.mask_flags()?.is_all_valid() && !b.mask_flags()?.is_nodata() {
        return fail(
            "explicit mask/alpha requires a validity-carrying adapter; unsupported in this slice",
        );
    }
    let transform = ds.geo_transform()?;
    if !transform.iter().all(|x| x.is_finite())
        || transform[2] != 0.
        || transform[4] != 0.
        || transform[1] <= 0.
        || transform[5] >= 0.
    {
        return fail("prototype requires a north-up grid with positive x and negative y spacing");
    }
    let srs = ds.spatial_ref()?;
    let crs = srs.authority().unwrap_or(ds.projection());
    if crs.is_empty() {
        return fail("missing CRS");
    }
    let (width, height) = ds.raster_size();
    let (bx, by) = b.block_size();
    if bx == 0
        || by == 0
        || width
            .div_ceil(bx)
            .checked_mul(height.div_ceil(by))
            .ok_or("layout tile count overflow")?
            > MAX_LAYOUT_TILES
    {
        return fail("layout exceeds prototype metadata budget of 65536 tiles");
    }
    let mut tiles = Vec::new();
    for y in 0..height.div_ceil(by) {
        for x in 0..width.div_ceil(bx) {
            let value = |prefix: &str| {
                b.metadata_item(&format!("{prefix}_{x}_{y}"), "TIFF")
                    .and_then(|s| s.parse().ok())
            };
            tiles.push(Tile {
                x,
                y,
                offset: value("BLOCK_OFFSET"),
                bytes: value("BLOCK_SIZE"),
            });
        }
    }
    Ok(Layout {
        width,
        height,
        transform,
        crs,
        variable: crate::Variable::Band { index: band },
        block: [bx, by],
        dtype: b.band_type().name(),
        nodata: b.no_data_value(),
        scale: b.scale(),
        offset: b.offset(),
        tiles,
    })
}
/// A scoped per-thread conditional, restored before the next asset/caller.
pub(crate) struct ConditionalHeaders {
    previous: String,
}
impl ConditionalHeaders {
    pub(crate) fn for_asset(asset: &Asset) -> Result<Self> {
        let previous = gdal::config::get_thread_local_config_option("GDAL_HTTP_HEADERS", "")?;
        if let Some(etag) = &asset.etag {
            if etag.starts_with("W/") || etag.is_empty() || etag.bytes().any(|b| b < 32 || b == 127)
            {
                return fail("a strong, single-line ETag is required");
            }
            // A trailing CRLF selects GDAL's raw-header parser; its CSV parser
            // strips ETag quotes. The mutation test checks the exact header.
            let value = if previous.is_empty() {
                format!("If-Match: {etag}\r\n")
            } else {
                format!("{previous}\r\nIf-Match: {etag}\r\n")
            };
            gdal::config::set_thread_local_config_option("GDAL_HTTP_HEADERS", &value)?;
        }
        Ok(Self { previous })
    }
}
impl Drop for ConditionalHeaders {
    fn drop(&mut self) {
        if self.previous.is_empty() {
            let _ = gdal::config::clear_thread_local_config_option("GDAL_HTTP_HEADERS");
        } else {
            let _ =
                gdal::config::set_thread_local_config_option("GDAL_HTTP_HEADERS", &self.previous);
        }
    }
}

// GDAL exposes this stable C API, but gdal 0.19 has no high-level wrapper.
unsafe extern "C" {
    fn VSICurlPartialClearCache(prefix: *const std::os::raw::c_char);
}
/// Before binding a new ETag, discard GDAL's URL-only header/range cache.
/// Called before this workflow has any open datasets for that URL.
pub(crate) fn clear_remote_cache(asset: &Asset) -> Result<()> {
    if remote(&asset.locator) {
        let prefix = std::ffi::CString::new(format!("/vsicurl/{}", asset.locator))?;
        // SAFETY: a live NUL-terminated C string, and no handles opened by this
        // acquisition exist yet. GDAL owns the cache and retains no pointer.
        unsafe {
            VSICurlPartialClearCache(prefix.as_ptr());
        }
    }
    Ok(())
}

/// Snapshot caller-local overrides before starting native worker threads.
/// Environment and process-global options remain GDAL's fallback.
pub(crate) fn thread_configuration() -> Result<Vec<(&'static str, String)>> {
    [
        "GDAL_HTTP_HEADERS",
        "GDAL_DISABLE_READDIR_ON_OPEN",
        "GDAL_INGESTED_BYTES_AT_OPEN",
        "GDAL_NUM_THREADS",
        "GDAL_CACHEMAX",
        "GDAL_HTTP_MAX_RETRY",
        "GDAL_HTTP_RETRY_DELAY",
        "GDAL_HTTP_TIMEOUT",
        "GDAL_HTTP_MULTIRANGE",
        "CPL_VSIL_CURL_CHUNK_SIZE",
        "CPL_VSIL_CURL_CACHE_SIZE",
        "CPL_DEBUG",
        "CPL_CURL_VERBOSE",
    ]
    .iter()
    .map(|key| Ok((*key, gdal::config::get_thread_local_config_option(key, "")?)))
    .collect()
}
pub(crate) fn apply_thread_configuration(configuration: &[(&str, String)]) -> Result<()> {
    for (key, value) in configuration {
        if !value.is_empty() {
            gdal::config::set_thread_local_config_option(key, value)?;
        }
    }
    Ok(())
}
