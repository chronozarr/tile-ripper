# Selecting native acquisition

The default build and default COG reader remain GDAL. Native acquisition is an
explicit option for the supported Sentinel-2 and direct COG profiles. Selecting
the native COG reader never falls back to GDAL for an unsupported raster profile
or codec. A GDAL-free build also rejects unsupported native geometry operations.
Zarr v2 uses its existing reader independently of the COG backend choice.

## Build without GDAL

From this source checkout, with Rust/Cargo, a C/C++ compiler, CMake and pkg-config:

```sh
cargo install --path . --locked --no-default-features --features native-cog \
  --root /absolute/path/to/new-native-install
/absolute/path/to/new-native-install/bin/tile-ripper --help
```

This build needs no system GDAL, PROJ or libtiff. Bundled Zarr codecs and TLS still
use native build tools and OS runtime libraries. Tested platform and linkage
details are in [installation](../installation.md). A Zarr-only build can use
`--no-default-features` without `native-cog`.

`--backend native` selects the COG reader and overrides
`TILE_RIPPER_COG_BACKEND`. `--backend gdal` selects the optional GDAL reader.
An unavailable reader fails before source inspection. Without either flag the
environment setting applies; absent that setting, GDAL is selected when compiled,
otherwise native is selected when compiled. The library reads this process-wide
setting during planning and execution. Keep it fixed from before preparation
until every `Prepared` object and its streams have been dropped; do not change
it while another acquisition is active.

`--gdal-free` requires a binary compiled without the `gdal` feature. For a native
COG build it also selects native, overriding an inherited backend environment
setting. It permits the existing Zarr-only build. It rejects a GDAL-linked binary,
including a dual-reader build made with `--features native-cog`. Selecting the
native reader in that dual build still links GDAL and uses GDAL geometry. To
compare readers in a dual build, use `--backend native` and `--backend gdal`
without `--gdal-free`.

## Polygon, dates and bands

The [supplied request](../native-sentinel/request.json) selects an ordinary WGS84
triangle near Chapel Hill, North Carolina, and B04/B08 on 28 July 2025:

```sh
/absolute/path/to/new-native-install/bin/tile-ripper plan \
  docs/native-sentinel/request.json --gdal-free --human
/absolute/path/to/new-native-install/bin/tile-ripper stream \
  docs/native-sentinel/request.json --gdal-free --workers 2 --per-host 2 \
  > sentinel-chunks.ndjson
```

The same source/date/band selection is available directly. Save a GeoJSON
**Polygon geometry** in `polygon.geojson`; do not wrap it in a Feature:

```json
{"type":"Polygon","coordinates":[[[-79.06,35.91],[-79.05,35.91],[-79.052,35.92],[-79.06,35.91]]]}
```

```sh
/absolute/path/to/new-native-install/bin/tile-ripper stream \
  --source https://earth-search.aws.element84.com/v1 --format stac \
  --collection sentinel-2-c1-l2a --aoi polygon.geojson \
  --start 2025-07-28T00:00:00Z --end 2025-07-28T23:59:59Z \
  --variables B04,B08 --buffer-mib 32 --workers 2 --per-host 2 --gdal-free
```

Both interval endpoints are inclusive. `--bbox WEST,SOUTH,EAST,NORTH` can replace
`--aoi`. Direct flags cap discovery at 8 items/8 assets and one million output
pixels; use the JSON request for custom limits, source-CRS bounds and chunk sizes.
Supported reflectance selectors are B01–B09, B8A, B11 and B12, or their Earth
Search common names. B10 and derived indices are outside this workflow.
`--buffer-mib` controls the declared working-buffer budget, not process RSS.
Native admission includes encoded buffers, decoded caches, cropped selections
and retained TIFF parser-page capacities. Larger budgets can enable coalescing
or additional readers; the [efficiency report](report.md) records the strategies.

`plan` inspects metadata and source identities; it can access the network.
With direct flags, `plan --gdal-free` defaults to a callback sink when `--output`
is omitted, so its inspection describes streaming. An explicit `--output` or
JSON sink is preserved; planning an output does not enable its optional writer.
`stream` sets a callback sink and emits NDJSON chunk **descriptors**, followed by
a `complete` receipt. It does not serialize the arrays or write a raster output.
The chunks remain provisional until the final receipt and a successful process
exit. For array values and scientific analysis, use the public Rust API or the
[native statistics consumer](../../consumers/native-stats/README.md).

`fetch` writes GeoTIFF for COG/STAC sources and requires the `gdal` feature.
GDAL-free COG fetch fails before discovery with the stream/API alternative.
Existing Zarr v2 fetch and Zarr output work without GDAL.

## Public Rust library

Use the checkout as a local dependency; it is not published to a registry:

```toml
[dependencies]
tile-ripper = { path = "/absolute/path/to/tile-ripper", default-features = false, features = ["native-cog"] }
serde_json = "=1.0.145"
```

`acquisition::prepare_gdal_free` checks that Tile-Ripper was compiled without its
`gdal` feature before preparation. It rejects dependencies that enable this
feature elsewhere in the Cargo dependency graph. It cannot detect GDAL linked
independently by a host application; inspect that application's final executable
to establish a whole-process linkage guarantee. `cog::require_gdal_free` exposes
the same guard, and
`cog::backend` returns the selected public `cog::Backend` or an unavailable-reader
error. These calls do not change process environment settings.

```rust
use tile_ripper::{
    acquisition, cog::LayoutCache, stream::StreamOptions, Request, Result, Sink,
};

fn analyze(path: &str) -> Result<()> {
    let mut request: Request = serde_json::from_slice(&std::fs::read(path)?)?;
    request.sink = Sink::Callback;
    let prepared = acquisition::prepare_gdal_free(&request, &mut LayoutCache::default())?;
    let mut stream = prepared.into_stream(StreamOptions { capacity: 0 })?;
    let mut provisional_sum = 0u64;
    for chunk in stream.by_ref() {
        let chunk = chunk?;
        let values = chunk.pixels.as_u16().ok_or("expected native UInt16")?;
        for (i, valid) in chunk.validity().enumerate() {
            if valid {
                provisional_sum += u64::from(values[i]);
            }
        }
    }
    let _receipt = stream.finish()?;
    // Publish or save the result only after successful final source validation.
    println!("{provisional_sum}");
    Ok(())
}
```

Values are stored UInt16 samples. Nodata, affine coordinates, native CRS,
scale/offset and timestamps accompany every chunk; scale/offset are not applied.
A rendezvous stream (`capacity: 0`) bounds delivery buffering. Caller-retained
arrays require additional memory. Cancellation, read errors and late source
mutation fail completion and invalidate provisional analysis.

## Supported profile and the GDAL boundary

Native COG accepts self-contained tiled single-band MinIsBlack UInt16 TIFF or
BigTIFF in either byte order, lossless uncompressed/LZW/Deflate and predictor
1/2, north-up PixelIsArea affine metadata and an EPSG horizontal CRS. Decoded
tiles are limited to 2 MiB. Metadata, tile-count and decode-cache bounds still
apply. No sidecar masks, PAM or worldfiles are consulted. The
[native COG profile](../native-cog/report.md#3-admitted-and-unsupported-profiles)
gives the complete raster restrictions.

For GDAL-free Sentinel selection, WGS84 query geometry must use exact
`EPSG:4326`, longitude then latitude, within ±80° latitude and span no more than
5° per axis. Polygons must be finite, closed, simple and two-dimensional, with
valid nonoverlapping holes. Transformations are limited to nearby WGS84 UTM
`EPSG:32601..32660` / `EPSG:32701..32760`, matching hemisphere and within ±6°
of the source zone's central meridian. Direct bounds in the exact source CRS
retain their generic native selection behavior. The
[geographic contract](../native-sentinel/workflow.md#geographic-and-raster-contract)
defines edge inclusion, nodata, clipping, densification and coordinate limits.

Use GDAL for GeoTIFF writing, supported GDAL formats outside this TIFF profile,
sidecar-dependent data and geometry/CRS semantics outside the native contract.
Tile-Ripper still rejects unsupported operations rather than promising every
operation GDAL itself can perform. Neither path offers general raster
reprojection, resampling, mosaicking or compositing through this API.

The [HTTP efficiency report](report.md) records the resource strategies and
performance gate. These CLI choices do not themselves imply a speed advantage.
