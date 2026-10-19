# Sentinel-2 STAC to native pixels

This workflow is usable without Python. It supports the public Earth Search
`sentinel-2-c1-l2a` collection, native UInt16 spectral assets, and an AOI polygon
(with holes) or bounding box. It writes a separate native GeoTIFF per selected
item/asset and a machine-readable manifest. It does not mosaic observations,
resample bands or create a temporal composite.

## Build and run the small real example

Use Rust >=1.88, system GDAL with GTiff, curl, GEOS and PROJ available to pkg-config.
The tested versions are recorded with the benchmark evidence. Build with the
committed dependency lock. The example is a roughly 1 km triangle near Chapel
Hill, North Carolina, on 2025-07-28, requesting B04 and B08.

```sh
cargo build --release --locked --bins --examples

# STAC discovery only: no raster HEAD, header or pixel requests.
./target/release/tile-ripper stac-plan examples/stac-request.json --discover-only

# Inspect native grids, selected windows, masks, source versions and estimates.
# This opens COG metadata, but does not execute pixel-window reads.
./target/release/tile-ripper stac-plan examples/stac-request.json > plan.json

# The destination must not already exist.
./target/release/tile-ripper stac-fetch examples/stac-request.json \
  --output ./sentinel-subset > acquisition-report.json
```

The checked example returned item `S2A_T17SPV_20250728T161108_L2A`. It produced
`asset-0000.tif`, `asset-0001.tif`, and `manifest.json`. Each image has 91 × 113
pixels at the native 10 m resolution; 5,004 pixel centers per band fall inside
the triangle. The 20,566 stored pixels, including outside-AOI nodata, matched
independent remote GDAL reference reads exactly. Catalog/source availability
can change; a future run should be checked against its own manifest.

The two source objects together are about 467 MB; the tool requests subsets
through GDAL's range reader. The plan contains estimates, not proof of actual
wire volume. The live run did not independently count HTTP bytes/requests.
Controlled benchmark servers measured these separately.

## Request contract

Copy `examples/stac-request.json` and change endpoint, dates, bands, AOI and
output. The supported provider adapter is `earth_search`, for public absolute
HTTPS COGs. It needs no account or signing. A service requiring bearer tokens,
SAS refresh, requester-pays or another collection requires a separate explicit
adapter and is outside this release. Do not assume the endpoint field makes
all STAC providers compatible.

AOI examples:

```json
{"kind":"bounds","crs":"EPSG:4326","bbox":[-79.060,35.910,-79.050,35.920]}
```

```json
{"kind":"polygon","crs":"EPSG:4326","coordinates":[
  [[-79.060,35.910],[-79.050,35.910],[-79.052,35.920],[-79.060,35.910]]
]}
```

Coordinates use x/y (longitude/latitude for EPSG:4326), not authority-axis
latitude/longitude order. Other valid CRS definitions can be supplied; AOIs
are mapped through GDAL/PROJ to WGS84 for discovery and each source CRS for
selection. Rings must be closed, valid polygons; interior rings are holes.
Multipart AOIs, antimeridian wrapping, polar AOIs, and more than 5 degrees per
axis are not supported. Latitude must lie within ±80 degrees. Input geometry
is limited to 10,000 positions; densification to 100,000 positions.

Dates are RFC3339 timestamps with explicit time zone. Intervals include both
endpoints. Discovery follows STAC pagination and preserves separate item IDs,
including distinct acquisitions/reprocessed items. Repeated IDs across pages
are deduplicated; there is no implicit best-scene or cloud-cover selection.

Bands may be Earth Search asset keys such as `red`/`nir`, or Sentinel aliases:
B01, B02, B03, B04, B05, B06, B07, B08, B8A, B09, B11 and B12. A missing asset
fails clearly. B10 is not an L2A spectral asset. Quality/SCL masking is not
implemented. **Clouds remain data**; this is an acquisition tool.

## Spatial and scientific decisions

The STAC query sends an exact `intersects` geometry and date interval. Returned
item geometries are independently intersected with the AOI; bbox overlap is
insufficient. After opening each native raster, planning clips the AOI to its
grid and checks rasterized masks for selected pixel centers in each planned
read. A footprint intersection without any selected pixel centers produces no
extract.

Geometry edges are straight in the supplied CRS. Before transformation, they
are densified at at most 0.001 degrees for geographic coordinates or 100 source
units for projected coordinates. This is an explicit approximation within the
bounded AOI regime, not a general global raster warp. Pixel membership uses
GDAL's default polygon rasterization (`ALL_TOUCHED=false`). The reference
checks use independently executed rasterio/GDAL geometry transformations and
masking. For boundary-critical work, verify this declared pixel convention.

Raster samples never warp. Different UTM zones, resolutions, origins and
shapes remain independent native outputs. The output is the clipped native
pixel envelope; cells outside the AOI and source nodata remain nodata. Masking
requires a source nodata value representable in UInt16; sources needing an
explicit internal mask/alpha plane are rejected in this release.

Stored DN values are copied without radiometric conversion. TIFF scale/offset
and nodata are preserved. The manifest also retains STAC `raster:bands`,
processing baseline and BOA-offset metadata. Consult these when deriving
physical values; the tool does not infer calibration or harmonize providers.

A strong ETag and object size are resolved during planning. COG metadata and
pixel reads use `If-Match`; a version change rejects the run. HTTP URL query
strings/fragments and userinfo are omitted from saved references; opaque IDs,
item ID, asset key, timestamp and ETag preserve provenance. Runtime requests
can contain sensitive locators and must be handled separately. Persistent
remote layout caching remains disabled; local layout caches are still usable.

## Limits and failures

Optional `limits` fields (omitted fields use defaults):

| Field | Default | Meaning |
|---|---:|---|
| max_items | 32 | Complete query cap; exceeding it fails, never silently truncates |
| page_size | 8 | Requested STAC page size; pagination still follows server links |
| max_assets | 64 | Selected band assets before raster inspection |
| max_output_pixels | 16,000,000 | Sum of native output envelopes across assets |
| buffer_bytes | 4,194,304 | Pixel plus mask working buffers, excluding GDAL/runtime caches |
| chunk_edge | 512 | Largest native read/write chunk edge |
| max_nodes | 100,000 | Materialized plan node cap |
| gdal_cache_mb | 32 | Requested GDAL block cache, process-global once initialized |
| max_retries | 2 | Bounded transient discovery/HEAD attempts and GDAL retry setting |
| timeout_seconds | 45 | Per HTTP request timeout |

Execution is serial and bounded by chunks. Metadata has the existing 65,536
tile-record/32 MiB cache-file caps. This is **not a hard process-RSS or
wire-byte guarantee**: GDAL compressed-tile decode, curl caches, geometry,
STAC metadata and runtime overhead exist outside the pixel-buffer limit.
A caller retaining callback buffers also owns that memory. Provider throttling
is handled with bounded retries; no adaptive high-concurrency testing is used.

An existing destination is never overwritten. A caught extraction/write error
removes the newly created incomplete directory; success writes `manifest.json`
last. An empty query returns an empty manifest and performs no raster I/O.
An OS kill or interrupt can leave an incomplete directory; an absent manifest
means it is not a completed acquisition. Choose a new directory for a rerun.
The library is synchronous: call it from a blocking thread in async programs.
GDAL configuration/diagnostics in a hosting application remain shared concerns.
Version binding clears GDAL's URL cache: serialize acquisitions with other
GDAL operations on the same URLs; do not keep external active handles to those
objects while preparing/executing this workflow.

## Rust API: separate discovery, planning and execution

```rust,no_run
use tile_ripper::{cog::LayoutCache, stac_workflow, Result};
fn acquire(request: &stac_workflow::StacRequest) -> Result<()> {
    let discovered = stac_workflow::discover(request)?;
    let mut cache = LayoutCache::default();
    let prepared = discovered.prepare(&mut cache)?;
    println!("{}", prepared.explain());
    prepared.fetch()?;
    Ok(())
}
```

Or call `prepared.execute(|chunk| ...)` for a fallible georeferenced-buffer
sink. The callback provides backpressure. Runtime bindings remain private;
prepared plans cannot be deserialized from diagnostic JSON. CLI `stac-fetch`
discovers and prepares afresh; it does not replay a stale saved plan.

A complete copyable library example is `examples/stac_acquire.rs`:

```sh
cargo run --release --locked --example stac_acquire -- examples/stac-request.json
# Add --fetch only when its configured output directory is new.
```

## Verification and evidence

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check

# Developer-only Python references use the preserved locked study environment.
/path/to/study/experiments/.venv/bin/python bench/stac_workflow.py --reps 5
/path/to/study/experiments/.venv/bin/python bench/verify_live.py \
  examples/stac-request.json ./sentinel-subset --report reference.json
```

See `stac-decision-gate.md` for results and limitations. Python is used for
independent verification and measurement, not for Tile-Ripper acquisition.
