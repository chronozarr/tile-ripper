# Proposed 0.1.0-alpha.1 contract

Candidate source distribution only; licensing and final gate must be cleared first.
Other platforms are untested, not excluded by design.
macOS arm64 with Rust 1.92.0 / Homebrew GDAL 3.12.4 is tested. Ubuntu 24.04 arm64 with Rust 1.92.0 / system GDAL 3.8.4 also passed a clean
container source install, ERA5 fetch and serial release tests. This is the other
tested alpha combination; broader Linux coverage is unverified. No Windows or Intel macOS support claim.
No stable Rust API, stable manifest schema across releases, or standalone binary.

Supported public sources: Earth Search `sentinel-2-c1-l2a` native UInt16 bands;
WeatherBench2's specific 1959–2023 6-hourly 64×32 ERA5 store; NASA POWER's IMERG
daily spatial store. Local UInt16 custom-grid Zarr and GeoTIFF remain advanced
profiles, not a claim that arbitrary datasets work.

Zarr: v2 C-order, top-level arrays, little-endian Float32 scientific data with NaN
fill, matching `_ARRAY_DIMENSIONS`, three axes in the admitted time/geographic
orders, one-dimensional admitted geographic/time coordinates, monotonic regular
axes (bounded producer Float32 rounding allowed), native rectangular selection.
Codecs: none/gzip or admitted Blosc LZ4/Zstd with shuffle 0–2 and automatic block
size. Only the documented finite positive Float32 fixedscaleoffset filter is
admitted. Exact time units/calendar and coordinate rules are documented in the
[ERA5](../cf-zarr-workflow.md) and [IMERG](../second-zarr-workflow.md) guides.
Local UInt16 requires Tile-Ripper's explicit custom-grid metadata. Hybrid v2/v3,
auxiliary CF references, arbitrary units/calendars/filters, nested variables,
curvilinear axes, arbitrary dtype/dimension orders and remote credentials fail.

COG/GeoTIFF: GDAL GTiff, native UInt16, finite north-up affine (positive x, negative y, zero rotation),
nonempty CRS and at most 65,536 layout blocks, no
explicit mask/alpha. STAC selection is bounded and produces separate native
GeoTIFFs, not a mosaic. Geographic Polygon masks use pixel centers; rectangles
use native windows. Read [STAC restrictions](../stac-workflow.md) before other catalogs.

`plan` validates the request and admitted metadata, selects native observations,
windows and physical reads, and reports estimates/limitations. It performs network
metadata reads and identity checks. Scientific Zarr chunks are not GET during
planning; TIFF headers/directories may be read. Repeated source-stable selections
are reproducible, but timings/traffic observations are not deterministic. Catalogs
can change. Inspection JSON is not an executable frozen plan; fetch prepares the
original request again.

`fetch` preserves admitted native samples, coordinates, units, validity and
scientific metadata, writes native output plus provenance, and returns success
only with a complete manifest. IMERG's storage filter is decoded once and omitted
on output to prevent a second lossy encoding. No calibration, cloud masking,
reprojection, resampling or aggregation occurs. Sentinel physical values require
scale/offset interpretation. HTTP versions are checked per object, not atomically
for an entire dataset. These are change-detection checks, not authenticity proofs.

Caught failures remove newly owned outputs. SIGINT/termination may leave an
incomplete directory; no resume or signal-cleanup promise. Check process success
AND `manifest.json` status `complete`, preserve diagnostics, then remove the exact
incomplete directory or choose a new output. Existing directories are rejected.
Payload budget is not an RSS ceiling. No universal wire-byte cap is provided for
GDAL. Temporary test/build/cache files and output cleanup are documented separately.

Open TIFFs with GDAL/rasterio/QGIS; use gdalinfo to inspect CRS, nodata, scale and
offset. Open Zarr with `xr.open_zarr(path, consolidated=False, chunks=None)` using
xarray and Zarr v2 support. No Python is needed for acquisition. Manifest source
references, scientific source_metadata, native windows, transformations and
observations explain what was fetched and what was measured versus estimated.

No Zarr v3, HDF5 acquisition, SWOT/NISAR adapters, generic CF, remote auth,
longitude wrapping/antimeridian crossing, arbitrary CRS target grids, compositing,
Python bindings, scheduling, or broad remote-sensing format promise. A system
GDAL having other drivers does not add supported Tile-Ripper formats.
Tile-Ripper does not consistently outperform tuned GDAL/xarray. Its alpha value
is inspectable acquisition and provenance across these narrow profiles.
