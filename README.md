# Tile-Ripper

[Private alpha candidate: installation and first scientist trial](ALPHA.md).

Retrieve native scientific raster subsets with one Rust library and one
`plan` / `fetch` CLI. Inspect the selected observations, native grids and physical
reads before downloading data. Outputs include a completion/provenance manifest.
**Private-alpha candidate (unreleased; licensing approval pending):** support is limited to the profiles below. This is not a general
Zarr reader, reprojection engine, mosaic or temporal compositor. No Python is
required to acquire data. The default build uses GDAL; opt-in builds support
[GDAL-free Sentinel-2 polygon acquisition and Zarr acquisition](docs/native-sentinel/workflow.md).

## Build and try a real dataset

Install Rust/Cargo and system GDAL. On macOS with Homebrew:

```sh
brew install gdal pkg-config
cargo build --release --locked --bin tile-ripper
./target/release/tile-ripper --help
```

For Sentinel-2 polygon/date/band acquisition without GDAL:

```sh
cargo build --release --locked --no-default-features --features native-cog
cargo build --release --locked --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/native-consumer
TILE_RIPPER_WORKERS=2 ./target/native-consumer/release/tile-ripper-native-stats docs/native-sentinel/request.json
```

The [complete workflow](docs/native-sentinel/workflow.md) accepts ordinary WGS84
polygons, discovers Earth Search Sentinel-2 bands, maps the AOI to supported UTM
grids and streams masked UInt16 chunks with pinned provenance. Direct COG assets
and native bounds remain supported for [the admitted TIFF profiles](docs/native-cog/report.md).
GeoTIFF writing requires the optional `gdal` feature. Zarr-only builds can omit
both features. C/C++ codec build tools remain required; see
[installation](docs/installation.md). GDAL remains the default build and reader.

Run from a checkout of this branch. [Installation and platform status](docs/installation.md)
cover native dependencies, local installation and release preparation.

This small public ERA5 example retrieves air temperature at two times without
credentials. The source chunk is roughly 0.6 MB even though the subset is tiny:

```sh
./target/release/tile-ripper plan \
  --source https://storage.googleapis.com/weatherbench2/datasets/era5/1959-2023_01_10-6h-64x32_equiangular_conservative.zarr/ \
  --format zarr_v2 --bbox 10,30,25,50 \
  --start 1959-01-01T00:00:00Z --end 1959-01-01T06:00:00Z \
  --variables 2m_temperature --human
./target/release/tile-ripper fetch examples/weatherbench2.json --output ./era5-subset --human
```

The second command uses the equivalent complete request file. Choose a **new**
destination; existing directories are never overwritten. The result is
`era5-subset/era5.zarr` plus `manifest.json`. For direct flags, use the same
selection with `fetch --output ./new-subset`; its asset/group name is `subset.zarr`.
`--buffer-mib 32` is the direct interface's default payload budget, not a process
memory limit. STAC defaults cap discovery at 8 items/8 assets and output at one
million pixels. Use JSON for advanced budgets and catalog limits.

Open the result with familiar tools (optional Python with xarray and Zarr v2):

```python
import xarray as xr
subset = xr.open_zarr("era5-subset/era5.zarr", consolidated=False, chunks=None)
print(subset)
print(subset["2m_temperature"].values)  # native Kelvin; NaNs remain invalid
```

## Incremental Rust consumers

The development branch adds bounded native chunk streaming and shared acquisition
scheduling. [API, CLI demonstrations and resource limits](docs/acquisition-engine/streaming.md)
include cancellation, completion checks and manual concurrency overrides.
[Controlled benchmark protocol](docs/acquisition-benchmark-protocol.md) describes
the performance gate. [Measured results and the interoperability recommendation](docs/acquisition-engine/report.md)
retain wins, ties and losses; no public-provider speed advantage is promised.

The [standalone native statistics consumer](consumers/native-stats/README.md) uses
the public API with provisional chunks and a validated completion receipt. The
[real Sentinel multi-scene gate](docs/production-streaming/report.md) compares
version-pinned native acquisitions with a tuned GDAL reader.

## Supported sources and limits

| Source | Supported selection and output |
| --- | --- |
| Earth Search Sentinel-2 C1 L2A | STAC dates, geographic bounds/Polygon, named native UInt16 bands; GDAL-free masked native stream, optional GDAL GeoTIFF output |
| WeatherBench2 ERA5 64×32 product above | Anonymous HTTP Zarr v2, Float32/NaN, `(time,longitude,latitude)`; native Zarr subset |
| NASA POWER IMERG daily spatial | Anonymous HTTP Zarr v2, Float32/NaN, `(time,lat,lon)`, verified daily time and storage filter; native Zarr subset |
| Local custom-grid Zarr / direct GeoTIFF | Narrow UInt16 profiles; [advanced lifecycle](docs/unified-workflow.md) and [local Zarr](docs/zarr-workflow.md) |

Public requests: [Sentinel-2](examples/unified-stac.json),
[ERA5](examples/weatherbench2.json), [IMERG](examples/imerg.json).
For IMERG, follow [the paired precipitation guide](docs/second-zarr-workflow.md).
Other stores must fit the [exact admitted geographic/codec profile](docs/second-zarr-workflow.md);
this is not generic CF support. Unsupported metadata, dtypes, dimensions and codecs fail
explicitly. Zarr supports rectangular selection, not polygon masking. No remote
authentication providers, Zarr v3, arbitrary calendars, curvilinear grids, target
grids, resampling, unit conversion, mosaicking or temporal aggregation.

Bounds are `west,south,east,north` in geographic x/y order with no antimeridian
crossing. Use the source longitude convention (ERA5 uses 0–360); no automatic
longitude wrapping. Zarr selects native cells whose area intersects the rectangle,
so outer centers may lie outside it. Both interval endpoints are included.
COG Polygon extraction instead masks by pixel center. Scientific coordinates
are preserved; EPSG:4326 on these Zarr profiles is an interpretation of geographic
axes, not a provider-declared precise datum. Original codec decoding is applied
once; outputs do not repeat IMERG's lossy storage filter.

## Inspect a plan

```sh
./target/release/tile-ripper plan examples/imerg.json --human
./target/release/tile-ripper plan examples/imerg.json --json > inspection.json
./target/release/tile-ripper fetch examples/imerg.json --dry-run --human
```

`plan` and `--dry-run` read bounded metadata/coordinate objects and inspect selected
object identities; they do not GET scientific Zarr chunks or write outputs.
GDAL may read TIFF headers/directories. Planning is not an offline operation.
Human output summarizes variables, dates, grids, windows, reads and estimates.
JSON exposes physical chunk keys/tile selections, output records, transformations,
validation details, limitations and measured planning traffic. Logical output byte
estimates exclude encoding, coordinates and filesystem overhead. Estimated reads
are library operations, **not** measured HTTP requests for GDAL.

Saved inspections cannot be replayed: `fetch` accepts the original request and
prepares it again. Metadata and data identities are checked before/after execution;
HTTP Zarr uses conditional ETags per object, not an atomic dataset snapshot.
Diagnostics redact remote locators; runtime request files may contain sensitive
information and should not be published. Anonymous Zarr rejects URL credentials,
query tokens and redirects. Use explicit supported endpoints.

## Fetch data

```sh
./target/release/tile-ripper fetch examples/imerg.json --output ./precipitation --human
./target/release/tile-ripper fetch examples/unified-stac.json --output ./sentinel --human
```

JSON is the default stdout format for scripts. `--human` adds concise stderr
progress and a completion path; the manifest records observations separately from
estimates, source references, native metadata, processing decisions and timings.
Zarr has measured HTTP totals; GDAL HTTP totals may be unavailable. A successful
manifest has `status: "complete"`; failures return nonzero and remove newly created
outputs. An interrupted process can leave an incomplete directory: inspect the
manifest and choose a new path before retrying. Never treat partial files as success.

Direct STAC syntax uses `--source ENDPOINT --format stac --collection NAME`,
`--variables B04,B08`, dates and `--bbox …` or `--aoi polygon.geojson` (a GeoJSON
**Polygon geometry**, not a Feature). Do not mix JSON selection and direct flags;
`--output`, `--human`, `--cache` and `--dry-run` still work with request files.

## Rust library

The CLI translates flags into the public `Request`; it calls the same library as
applications. There is no CLI dependency in the planner:

```rust,no_run
use tile_ripper::{acquisition, cog::LayoutCache, Request, Result};
fn fetch(request: &Request) -> Result<()> {
    let prepared = acquisition::prepare(request, &mut LayoutCache::default())?;
    println!("{}", prepared.human());
    let manifest = prepared.fetch()?;
    assert_eq!(manifest["status"], "complete");
    Ok(())
}
```

[The runnable example](examples/acquire.rs) accepts any shared request;
[a typed request example](examples/scientific_subset.rs) constructs the ERA5 query directly:

```sh
cargo run --release --locked --example acquire -- examples/weatherbench2.json --fetch
```

`Prepared::execute` also delivers owned native `Pixels::UInt16` or `Pixels::Float32`
buffers synchronously, with backpressure, coordinates, validity and provenance.
Use `.pixels.len()`, `.as_u16()` or `.as_f32()`; callback consumers must not retain
unbounded buffers. Results are provisional until execution returns success.
The planner models assets, variables, grids, windows and reads; runtime adapters
own codecs and source bindings. `cog::LayoutCache` is a historical naming/coupling
rough edge; its default works for both formats. API is not yet stable.

## Validation and benchmarks

Native values, coordinates, NaNs/nodata and metadata are checked independently
against GDAL and xarray. [Current private-alpha gate](docs/alpha/closeout.md)
records the fresh-context agent trial, installation evidence and remaining alpha blockers.
[Alpha contract](docs/alpha/contract.md), [small scientific examples](docs/alpha/examples.md),
and [licensing audit](docs/alpha/licensing.md) define the candidate scope.
[Previous scientific gate](docs/second-zarr-decision.md) and
[raw repeated benchmarks](docs/second-zarr-benchmark.json) preserve methods and limits.
There is no demonstrated consistent end-to-end speed advantage. xarray is preferable
for broad CF processing and analysis; GDAL/rasterio for general raster operations.
See [ordinary alternatives and matching-work limits](docs/alternatives.md).
Tile-Ripper's present advantage is a shared inspectable acquisition lifecycle with
bounded native selection and provenance, not a new decoder or universal speedup.

## Development and architecture

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

See [architecture](docs/0003-unified-lifecycle.md),
[implementation map](docs/implementation-map.md) and
[historical study](study/README.md). Detailed milestone reports in `docs/` retain
historical scope and results; use this README and the current gate for current support.
No ChronoZarr dependency, plugin system, Python binding or paid service is required.

The current COG efficiency decision gate uses a two-worker default, source-local
GDAL reader reuse and bounded parallel layout inspection. The
[COG efficiency report](docs/cog-efficiency/report.md) records measured traffic,
matched GDAL comparisons and the tested 192-MiB resource profile. Working-buffer
reservations are not a process RSS ceiling. This branch remains unreleased.
