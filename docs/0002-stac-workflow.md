# ADR 0002 — STAC discovery prepares a native acquisition plan

Continue from 6277ee6 without replacing the logical DAG or Zarr planner.
Use stac/stac-io's typed searches and paginated client. Public Earth Search
Sentinel-2 C1 is the supported provider adapter; signing is unnecessary for
its public HTTPS COGs. Unsupported authentication is rejected explicitly.
No copied HTTP/STAC engine or TIFF decoder.

A separate STAC request facade owns endpoint, collection, AOI, time, band names,
output and conservative limits. Discovery preserves item/asset provenance,
filters exact item geometry rather than bbox only, and maps requested STAC
asset keys/common band names to single-band native COG assets. It produces
runtime asset bindings separately from sanitized diagnostic plans.

Native COG planning transforms a densified AOI through PROJ/GDAL to each source
CRS, intersects the actual grid, selects native windows, and rasterizes AOI
masks with GDAL's default pixel-center rule. Data remain on the native grid;
coordinate transformation of geometry does not reproject imagery. Polygon
holes are retained. Large/wrapped/polar AOIs are rejected for this narrow
workflow rather than trusting a four-corner envelope. Each planned read must
contain selected pixel centers; footprint intersection alone is insufficient.
Source nodata is preserved. AOI masking requires an explicit source nodata
sentinel in this release. Independent grids remain separate outputs.

Execution consumes a prepared plan in memory, not a serialized plan supplied
by another caller. Pixel/mask buffers stay chunked, GDAL owns retries and
codec work, and one tiled GeoTIFF is written per asset through window writes.
A manifest records source item/key, sanitized reference, datetime, band/grid,
radiometry metadata, AOI and boundary decisions. It never stores credentials.
No mosaicking, calibration, cloud mask, resampling or time reduction is implied.

Planning may read COG metadata; --discover-only performs no raster opens.
Request/item/asset/output caps reject over-budget jobs rather than silently
truncating discovery. Native-grid pixel buffers, GDAL caches, metadata and
output limits are distinct resources; no false hard-RSS guarantee.

Measurements distinguish discovery, planning, GDAL fused read+decode, AOI
masking, writing and process wall. Exact transfer counts come from independent
HTTP server/proxy instrumentation where available. GDAL's fused decode is not
invented as a separately observed transfer/decode timer. Live public tests
are small and serial; repeated throughput tests use bounded local mirrors.

Object identity was made concrete during implementation: strong ETags are
resolved through the existing HTTP client, and scoped raw GDAL headers send
`If-Match` during metadata and pixel reads. GDAL's URL-only cache is cleared
before binding an identity and before executing a prepared snapshot, through
its existing `VSICurlPartialClearCache` API. This avoids reuse of another
prepared job's cached version. A same-process, two-snapshot library test is
part of the local harness. The cost is repeated metadata opens, reported
rather than hidden. No persistent remote layout cache is introduced here.
