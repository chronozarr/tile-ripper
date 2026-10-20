# Sentinel-2 acquisition without GDAL

Build from this source checkout using Rust 1.92, a C/C++ compiler, CMake and
pkg-config. No GDAL, PROJ, libtiff or Python installation is needed to acquire
and consume data. Existing Zarr/TLS native-code dependencies still require the
C/C++ build tools; the runtime retains OS libraries including C++.

```sh
cargo install --path . --locked --no-default-features --features native-cog \
  --root /absolute/path/to/new-native-install
cargo build --release --locked --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/native-consumer
/absolute/path/to/new-native-install/bin/tile-ripper plan docs/native-sentinel/request.json --json
TILE_RIPPER_WORKERS=2 target/native-consumer/release/tile-ripper-native-stats \
  docs/native-sentinel/request.json > sentinel-statistics.ndjson
```

The supplied request is an ordinary longitude/latitude triangle near Chapel Hill,
North Carolina, on 28 July 2025, selecting Sentinel-2 B04 and B08. Edit its polygon,
inclusive date interval and bands. The consumer uses public Rust APIs to discover
observations, pin strong object ETags, plan native UTM windows, acquire COG tiles,
mask pixels and incrementally compute stored-value counts and sums. Discovery
retains deterministic item-ID order and requested band order. This is not a
cloud-free composite or calibrated reflectance calculation.

`provisional_start` includes WGS84 query bounds, longitude/latitude order,
source footprints, item/asset IDs, acquisition dates, STAC calibration and pinned
ETags. Each chunk includes its native CRS, six-term GDAL affine transform,
source pixel window, dtype, source nodata and TIFF scale/offset. Pixel centers
are available through `Chunk::pixel_center`; values remain on each source grid.
The consumer's NDJSON stores provisional per-chunk statistics and a checksum.
Only a final `complete` record **and successful exit** authorize using those
statistics. Cancellation, consumer failure or a late source-version conflict
invalidates earlier chunks; EOF alone never commits results.

For your own analysis use the same public stream:

```rust
use tile_ripper::{acquisition, cog::LayoutCache, stream::StreamOptions, Request, Result};

fn analyze(request: &Request) -> Result<()> {
    let prepared = acquisition::prepare(request, &mut LayoutCache::default())?;
    let mut stream = prepared.into_stream(StreamOptions { capacity: 0 })?;
    for chunk in stream.by_ref() {
        let chunk = chunk?;
        let values = chunk.pixels.as_u16().ok_or("expected UInt16")?;
        for (i, valid) in chunk.validity().enumerate() {
            if valid {
                // Accumulate provisional analysis from values[i].
                let _sample = values[i];
            }
        }
    }
    let _receipt = stream.finish()?;
    // Commit the provisional analysis only here.
    Ok(())
}
```

The [standalone consumer](../../consumers/native-stats/README.md) adds bounded
payload checks, output flushing and cancellation demonstrations. CLI `stream`
provides chunk metadata and a completion record; the Rust API supplies arrays.
Use a callback sink for this workflow. GeoTIFF writing remains optional and
requires the GDAL feature. No new array format is introduced.

## Geographic and raster contract

Native-only geometry accepts finite, closed, simple two-dimensional Polygons
with valid nonoverlapping holes. Ring orientation is immaterial. User input is
capped at 10,000 positions, transformed/restored geometry at 100,000. Geographic
queries must use exactly `EPSG:4326`, longitude then latitude, within ±80°
latitude and at most 5° per axis. Antimeridian crossings, invalid topology,
alternate axis/unit/datum semantics and other query CRS transformations fail.
Public query MultiPolygons/collections are outside the request schema; internal
clipping may produce multiple Polygon pieces and preserves all of them.

Transformation is restricted to WGS84 UTM `EPSG:32601..32660` and
`EPSG:32701..32760`, matching hemisphere and within ±6° of the source zone's
central meridian. Nearby overlapping Sentinel grids can therefore be selected
without forcing every observation into one zone. Projected queries can mask
their identical native UTM grid, but inverse and cross-zone query transforms
are unsupported. Existing direct bounds in the exact source CRS retain their
previous generic selection behavior.

Queries have straight edges in their declared CRS. Geographic edges are
densified to at most 0.001° before transformation. The raster is never warped,
resampled or calibrated. Source grids must be finite north-up PixelIsArea;
native TIFF profile restrictions remain those in the
[COG report](../native-cog/report.md). Out-of-source pixels are clipped away;
cells outside the Polygon are set to representable UInt16 source nodata. Nodata
inside the AOI remains invalid. Masking without representable source nodata fails.
Subwindows with no included centers do not emit chunks. Empty selections can
complete with zero chunks.

The window is the floor/ceil outer pixel envelope clipped to the source. Masks
use GDAL's default scanline convention (`ALL_TOUCHED=false`) on the full source
grid, then crop by integer pixel offsets. Inclusion is independent of delivery
chunk size. Scan at row+0.5,
cross nonhorizontal edges over `[min_y,max_y)`, and fill crossing pairs over
`[floor(left+0.5),floor(right+0.5))`. Thus vertical center ties belong to the
polygon on their left. Directed horizontal center edges follow GDAL's burn rule
after winding normalization, including hole boundaries. The implementation uses
explicit inverse-affine floating-point evaluation; exact center ties are tested
against independently executed GDAL, including large UTM coordinates. No epsilon
expands or shrinks the AOI. The reference rasterizes the full source grid before
cropping; a separately rasterized GDAL subwindow can round an exact center tie
differently. See the source comments and
[correctness receipt](correctness.json) for the tested boundary cases.

This path uses pinned [proj4rs](https://docs.rs/proj4rs/0.2.1/proj4rs/)
for WGS84 UTM coordinates and [geo](https://docs.rs/geo/0.33.1/geo/)
for validity and planar clipping. It contains no general CRS database, datum-grid
selection, geodesic polygon model or raster reprojection engine.

## Completion and resources

The existing version-bound reader and planner retain their byte accounting,
decode limits, bounded caches and working-buffer admission. Source objects are
validated before acquisition and after provisional delivery. Strong ETags are
trusted provider identities, not whole-object content hashes. A rendezvous
consumer retains one delivered payload; caller-retained arrays are additional.
Plans, geometry, metadata, decoder scratch and runtime memory are separate from
the request's `buffer_bytes`. Use the measured RSS receipts as deployment evidence,
not a claim that `buffer_bytes` is a process-memory sandbox.

The default build remains GDAL. A dual-feature build selects GDAL geometry and
links GDAL even if the raster backend environment variable selects native.
Use `--no-default-features --features native-cog` to obtain the validated complete
GDAL-free path. macOS ARM64 and Linux ARM64/glibc are the tested platforms.

Evidence and reproduction: [protocol](protocol.md), [decision report](report.md),
[independent geometry/array checks](correctness.json),
[STAC/fault checks](workflow-checks.json), [live Sentinel receipt](live.json),
[Linux runtime](linux-runtime.json) and [request diagnosis](request-profile.md).
