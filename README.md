# Tile-Ripper

For the current development state, validation and remaining release work, see
the [consolidation checklist](docs/consolidation.md). The October 19 alpha trial
is an earlier candidate contract; it does not cover every workflow added below.

Named native product workflows now cover sensor-specific Landsat 4/5 TM, Landsat 7 ETM+ and Landsat 8/9 Collection 2 surface reflectance, HLS L30/S30 v2, OPERA DSWx-HLS, current DSWx-S1, DIST-ALERT, and the qualified OPERA RTC-S1 Float32 profile. Use `tile-ripper products` and the existing `plan` / `fetch` / `stream` commands with a product request. See [runnable examples and installation](docs/products/workflow.md), [exact real-source qualification and access limits](docs/products/qualification.md), [scientific band/quality definitions](docs/products/specifications.md), and the [25-row coverage roadmap](docs/products/roadmap.md). Stored values, native grids, calibration, classification labels, and independent quality masks are preserved.

[Wave 3 native array products](docs/products/wave3/qualification.md) add current GOES-19 CMIPC C13/C01, SMAP SPL3SMP_E v6 global AM/PM, and a real NISAR GCOV frequency-A HHHH diagonal. Their ordinary discovery/plan/stream/fetch commands preserve original scientific types, typed native grids, source fill, separate AOI/quality masks and durable Zarr. GOES is anonymous; SMAP and NISAR use separately qualified explicit Earthdata policies. [Wave 4 geographic acquisition](docs/products/wave4/qualification.md) adds WGS84 AOIs for the inspected 10/20 m NISAR UTM16N grids, separate SMAP northern-polar AM/PM profiles, and GOES-19 mesoscale-1 C02. Build with `--no-default-features --features native-cog,native-hdf5,authenticated-http`; Cargo defaults remain unchanged.

Named products accept `--provider auto|planetary_computer|earthdata|pinned`
(or `"provider"` in product JSON). For example:

```sh
tile-ripper plan hls-request.json --provider auto --human
tile-ripper plan hls-request.json --provider earthdata --auth earthdata --netrc /private/path/.netrc --human
```

Fresh automatic Landsat/HLS discovery prefers credential-free Planetary Computer
access, including accountless runtime signing. HLS uses `hls2-l30`/`hls2-s30` v2
collections; legacy HLS v1.4 is excluded. OPERA retains its current Earthdata
routes. Explicit authorization is considered only when that provider is selected;
ambient credentials are never searched. No general authenticated-route speed
advantage is established, so authorization alone does not change the default.
Supplied observations pin their physical source and are never replaced. An
explicit override that conflicts with them fails; mixed-provider observations
require separate preparations.

JSON and human plans expose the provider, policy, credentials, coverage limits,
and immutable physical source identity. HLS automatic discovery compares both
bounded catalogs, records actual observation IDs/counts and failures, and
prioritizes complete coverage. With authorization,
a strict observation superset wins; incomparable sets require an explicit
selection. Without authorization, NASA-only identities are disclosed and require
a fresh authorized plan. Explicit overrides query only their selected catalog.
Cross-provider scientific evidence is limited to independently compared samples bound to exact canonical URLs and
ETags; other assets have unknown cross-provider equivalence. A matching name,
tile or date cannot create an equivalence group. The observed PC signer grants
container read/list access; the engine confines every request to the selected
canonical object. Exact read-only blob grants are also accepted; write grants
are rejected. Signatures stay in memory. Changed objects, expired signatures and unavailable sources fail and require an
explicit fresh plan. Prepared acquisitions never switch provider, and product
checkpoints remain unsupported. See the [provider workflow, evidence and limits](docs/providers/qualification.md).

The standalone [AlphaEarth GeoZarr prototype](docs/alphaearth/workflow.md) streams bounded rank-four native Int8 embeddings using selective shard ranges and sampled coordinate validation. It preserves the live source’s original `band` dimension and positional channel order. Durable embedding output remains outside this prototype.

Bounded SWOT Raster D discovery now finds candidates by WGS84 bounding box and
date through NASA CMR. `discover` reports catalog relevance and exclusions;
`discover-job` accepts explicit native windows or WGS84 bounding boxes and
polygons, resolves geographic requests against inspected source CRS and stored
pixel centers, and writes a version-pinned job after admission. Geographic
outputs retain a distinct `aoi_mask` beside source validity and quality masks.
See the [geographic workflow and contract](docs/swot-aoi/workflow.md),
[runnable AOI selection](examples/swot-aoi-selection.json) and
[independent scientific gate](docs/swot-aoi/report.md). The original
[native-index workflow](docs/swot-discovery/workflow.md) remains available.
Geographic job generation now reuses one immutable source-grid inspection per
granule. See the [inspection API and checkpoint boundaries](docs/grid-inspection/workflow.md)
and [matched three-product measurements](docs/grid-inspection/report.md).
The [source-validation lifecycle and measurements](docs/source-validation/report.md)
document the subsequent single-binding HEAD reduction, source-failure latches,
retained authorization/completion checks, and COG/Zarr request controls.

Explicit one-to-four-observation SWOT Raster D PGD0 jobs now use normal
`plan` / `fetch` commands to write one scientific Zarr v2 dataset. Each granule
keeps its native grid, raw variable types, source masks and quality policy.
See the [runnable workflow and Rust API](docs/swot-job/workflow.md) and
[scientific, recovery, resource and measurement evidence](docs/swot-job/report.md).

Opt-in authenticated source-range resume for the qualified SWOT Raster D
workflow is available through the normal `plan`/`stream` CLI and Rust APIs.
See [the tested workflow](docs/transfer-resume/workflow.md) and
[the local decision gate](docs/transfer-resume/report.md). A resumed scientific
stream starts at its beginning; results remain provisional until `finish()`.
The [held-out UTM01W qualification](docs/held-out-qualification/report.md)
resolves the sparse-reference failure and verifies a nonempty second PGD0
selection, live recovery and Linux ARM64 checkpoint behavior.

[Private alpha candidate: installation and first scientist trial](ALPHA.md).

Retrieve native scientific raster subsets with one Rust library and one
`plan` / `fetch` / `stream` CLI. Inspect the selected observations, native grids and physical
reads before downloading data. Outputs include a completion/provenance manifest.
**Private-alpha candidate (unreleased; licensing approval pending):** support is limited to the profiles below. This is not a general
Zarr reader, reprojection engine, mosaic or temporal compositor. No Python is
required to acquire data. The default build uses GDAL; opt-in builds support
[GDAL-free Sentinel-2 polygon acquisition and Zarr acquisition](docs/native-sentinel/workflow.md).
Use [native CLI selection and public Rust examples](docs/native-http/cli.md) for
`--backend native` and the explicit `--gdal-free` build guard.

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
./target/release/tile-ripper fetch docs/native-sentinel/request.json \
  --gdal-free --workers 2 --output ./sentinel.zarr
```

The [complete workflow](docs/native-sentinel/workflow.md) accepts ordinary WGS84
polygons, discovers Earth Search Sentinel-2 bands, maps the AOI to supported UTM
grids and streams masked UInt16 chunks with pinned provenance. The
[scientific output workflow](docs/scientific-output/workflow.md) saves actual
values to portable Zarr v2, with separate observation groups for different dates,
bands and grids. Open a group with ordinary Python Zarr/xarray; Rust code is not
needed. The [durable-output measurements and safety gate](docs/durable-output/report.md)
cover transactional synchronization, lossless encoding candidates and bounded
streaming; ordinary fetch retains uncompressed Zarr v2. Direct COG assets
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
The [native HTTP efficiency decision](docs/native-http/report.md) records current
request, byte, latency and resource comparisons for the supported native profile.
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
| Landsat 8/9 C2 Level-2 SR | Native UInt16 SR, UInt16 pixel/saturation QA and UInt8 aerosol QA; bounded Planetary Computer discovery/runtime signing; [product workflow](docs/products/workflow.md) |
| Landsat 4/5 TM and Landsat 7 ETM+ C2 SR | Separate sensor profiles; six SR bands, pixel/saturation/LEDAPS QA and opacity; raw SLC-off gaps retained; [Wave 2 evidence](docs/products/wave2/landsat.md) |
| OPERA DSWx-S1 V1.0/SAS1.2 | Four UInt8 layers and separate radar water policies; exact current PO.DAAC route; [source and output evidence](docs/products/wave2/dswx-s1.md) |
| OPERA DIST-ALERT-HLS V1 | Nine native status/confidence/onset/last-date/current-mask layers; separate accumulated detection and observation quality; [qualification](docs/products/wave2/dist-alert.md) |
| HLS L30 and S30 v2 | Distinct Int16 reflectance and UInt8 Fmask profiles; public GSFC references and separately qualified explicit Earthdata COG access; 30 m UTM groups |
| OPERA DSWx-HLS v1 | Exact UInt8 classified water and UInt16 diagnostics, separate named class masks; public reference and current PO.DAAC v1.1 authenticated access |
| OPERA RTC-S1 v1 | Exact Float32 linear gamma0 VV/VH and UInt8 mask on public references and a current authenticated ASF Cumulus Sentinel-1C burst |
| Earth Search Sentinel-2 C1 L2A | STAC dates, geographic bounds/Polygon, named native UInt16 bands; GDAL-free per-observation Zarr v2 or masked native stream, optional GDAL GeoTIFF output |
| WeatherBench2 ERA5 64×32 product above | Anonymous HTTP Zarr v2, Float32/NaN, `(time,longitude,latitude)`; native Zarr subset |
| NASA POWER IMERG daily spatial | Anonymous HTTP Zarr v2, Float32/NaN, `(time,lat,lon)`, verified daily time and storage filter; native Zarr subset |
| Archived GOES-19 RRQPE and cloud-top-temperature NetCDF-4 samples | Opt-in `native-hdf5`; exact UInt16/Float32 rank-two array slices, stored one-dimensional axes, conditional compressed-chunk ranges and provisional streaming; [profile and decision gate](docs/native-hdf5/report.md) |
| Inspected SWOT Raster Version C 100 m UTM19T scene | Native Float32 `/wse` with matching compressed Float64 `/x`,`/y`; exact index slices qualified against h5py/NetCDF-C on an identity-pinned bounded mirror. The Version C live archive profile is not qualified; [product, access and science gate](docs/swot-wse/report.md) |
| Inspected SWOT Raster Version D PGD0 100 m UTM19T scene | Aligned native WSE, UInt8 summary quality, UInt32 detailed flags/counts and Float32 uncertainty, separate explicit quality policies, exact stored coordinates and bounded streaming; qualified against h5py/NetCDF-C on a pinned mirror and directly from NASA through opt-in authenticated conditional ranges; [live gate](docs/earthdata-auth/report.md), [authentication workflow](docs/earthdata-auth/workflow.md) and [science](docs/swot-quality/report.md) |
| Held-out SWOT Raster Version D PGD0 100 m UTM01W selection | Same five-layer CLI/reader and Earthdata policy, 1,310 good/suspect accepted cells, both independent references and live source-range resume; [bounded selection, resource diagnostics and platform evidence](docs/held-out-qualification/report.md). This qualifies the documented selection and policy. |
| Public GFS v3 archive | Native Float32 subset, ordinary/sharded codecs and conditional selective ranges; [exact v3 profile](docs/zarr-v3-input/workflow.md) |
| Local custom-grid Zarr / direct GeoTIFF | Narrow UInt16 profiles; [advanced lifecycle](docs/unified-workflow.md) and [local Zarr](docs/zarr-workflow.md) |

HDF5 v1 index pruning is an explicit experiment: set
`TILE_RIPPER_HDF5_INDEX_POLICY=selective`. Full traversal remains the default
because pruning cannot validate unread subtrees. Bounded metadata pages are
available independently; see the [large-file traversal results](docs/hdf5-selective/report.md).

Public requests: [Sentinel-2](examples/unified-stac.json),
[ERA5](examples/weatherbench2.json), [IMERG](examples/imerg.json).
For IMERG, follow [the paired precipitation guide](docs/second-zarr-workflow.md).
Other stores must fit the [exact admitted geographic/codec profile](docs/second-zarr-workflow.md);
V3 has a separate [admitted profile and measured limitations](docs/zarr-v3-input/report.md).
This is not generic CF support. Unsupported metadata, dtypes, dimensions and codecs fail
explicitly. Zarr supports rectangular selection, not polygon masking. No remote
authentication providers, arbitrary calendars, curvilinear grids, target
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

`Prepared::execute` also delivers owned native `Pixels::UInt8`, `Pixels::UInt16`, `Pixels::Int16`, `Pixels::UInt32`, or `Pixels::Float32`
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

### Version D SWOT Raster quality-aware stream

The opt-in `native-hdf5` adapter additionally retains stored `Pixels::UInt8` and
`Pixels::UInt32`. Integer quality flags are never cast to Float32. Existing
UInt16/Float32 accessors and consumers are preserved; exhaustive matches need
handlers for the two additional variants. HDF5 remains callback/stream output.

For the inspected Version D schema, select the five `swot::PATHS` datasets in
one ordinary native request, then apply an explicit separate policy:

```rust,ignore
let prepared = tile_ripper::swot::prepare(
    &request,
    tile_ripper::swot::Policy::cookbook(), // good (0) or suspect (1)
    &mut cache,
)?;
let mut stream = prepared.into_stream()?;
for tile in stream.by_ref() {
    let tile = tile?; // five aligned raw layers and separate derived validity
    consume(tile)?;
}
let report = stream.finish()?; // final source validation authorizes completion
```

Raw source masks and named required/excluded bitwise conditions are available
without overwriting science values. Preparation checks the source definitions;
there is no universal good-pixel policy. The real PGD0 file, physical layout,
C-versus-D differences, independent science/fault/resource checks, benchmark
are documented in the [Version D workflow](docs/swot-quality/workflow.md).
The separate [authenticated live gate](docs/earthdata-auth/report.md) qualifies
this exact D product directly from NASA with all five layers. Build with
`native-hdf5,authenticated-http`, construct a caller-owned
`authenticated_http::AuthenticatedHttp::earthdata` session from the canonical
URL and a private netrc file path, then pass it to
`swot::prepare_authenticated`. [Usage and scope](docs/earthdata-auth/workflow.md).
Ordinary `prepare` remains anonymous; credentials and signed URLs never enter
request JSON or provenance.
