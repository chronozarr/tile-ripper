# ADR 0001 — A logical acquisition DAG with format-owned physical reads

Status: accepted for this proof of concept, before implementation. 2026-10-18.

## Evidence and decision

The October 9–10 study remains the baseline, particularly study/07 and the
actual e4_layout.py, e5_planner.py, w_floor.py, w_gdal.py and e6_pixels.py.
The SQL window → tile → offset algorithm is reusable; the handwritten TIFF
scanner and deflate/predictor decoder are experimental oracles, not production
components. The 48,000-asset result measures warm SQL over replicated layout
rows, not discovery, metadata construction or execution. The 72-asset exact
pixel result covers two native windows and one provider. Tuned GDAL was close
to the manifest executor on a noisy workstation link. No engine-speed claim
or repeat ecosystem census is justified. Rasteret already supplies layout
indexes; duckdb-cog's tested interface did not supply request manifests.

Keep one Rust package with public model, planner, COG/GDAL adapter, Zarr
planning adapter, execution and a thin CLI. A dependency DAG describes logical
reads, decoding responsibility, transformations and sink writes. A read is a
sum type: library window, object chunk, byte range, whole object or dataset
slice. It is not a universal byte-range manifest. Format adapters negotiate
capabilities and own codecs, layout, coordinate mapping and safe fallbacks.

The executable slice uses GDAL through Rust bindings for metadata and native
window reads. TIFF block offsets/counts are optional explanatory evidence;
GDAL controls actual transfer grouping. An unknown count stays unknown.
We do not pretend the explanatory tile manifest is the HTTP execution trace.
Zarr v2 unsharded arrays test chunk-object planning using .zarray metadata;
materialization remains unsupported. A v3 shard requires an index resolution
node before ranges can be selected; it cannot masquerade as an unsharded key.

## Product contract

A request separates source (direct assets / catalog+collection), spatial
selection (bounds or polygon with CRS), time (interval or explicit instants),
variable selectors (band, polarization or dataset path), quality predicates,
grid (native or explicit target+resolution+resampling), explicit mosaic or
temporal reduction, sink, memory and execution budgets. Missing time means
all supplied observations. No implicit daily/monthly axis or composite.
Native output preserves each observation's grid and identity.

The compiled prototype deliberately accepts direct assets, bounds already in
the source CRS, integer band selectors, native grids, and no quality filters.
Unsupported variants return errors before pixel reads. Polygon masking, CRS
conversion, catalogs, quality semantics, calibrated values and mosaics need
separate implementations. Raw values, nodata, scale/offset and validity must
survive extraction; never silently turn raw DN into calibrated measurements.

## Boundaries and incremental planning

Catalog::discover yields Asset descriptors without reading raster blocks.
FormatPlanner::{capabilities, plan_asset} turns one descriptor plus selection
into nodes. Executor consumes executable reads and sends georeferenced chunks
to a fallible sink callback, providing backpressure. The first implementation
is synchronous; async streams can wrap a bounded blocking worker, not GDAL
calls on an async runtime. Use bounded plan batches per asset/page so useful
pixels can arrive before a catalog completes. The CLI currently materializes
a small whole plan; this is not yet an incremental scheduler.

Planning I/O: discovery pages, object identity checks, TIFF IFDs, Zarr array
metadata, shard indexes and optional HDF5 chunk tables. Execution I/O: data
reads, data-dependent quality gates, writes. Persist layout metadata only with
validated object identity and adapter/schema version. Local cache uses file
size+mtime; remote caching requires ETag/version and conditional reads. Signed
locators and headers remain runtime-only; saved plans use opaque asset IDs.
Saved plans are diagnostic, not replayable authorizations.

## Capabilities and format matrix

| Family | Physical selection | Decode owner | Safe fallback / limitation |
|---|---|---|---|
| COG/GeoTIFF | native windows; optional tile ranges | GDAL now; async-tiff candidate after parity audit | GDAL handles codec/strip/interleave complexity; georeferencing required |
| Zarr v2 | chunk objects with dimension separator and fill semantics | zarrs candidate | prototype planning only; no invented geospatial transform from .zarray |
| Zarr v3 | chunk keys or shard-index-dependent ranges | zarrs | must negotiate codec chain and sharding; v2 prototype rejects v3 |
| HDF5/NetCDF-4 | dataset slices; verified internal chunks if safe | GDAL multidimensional API / HDF5/netCDF libraries | stage whole object only when explicitly allowed and disk budget fits; VLEN, filters and external links may defeat reference reads |
| GeoParquet | predicates, columns and row groups | DuckDB / Arrow-Parquet | can be catalog, features or sink; no raster assumption; exact geometry follows coarse pruning |

Kerchunk/VirtualiZarr references are optional layout adapters: require immutable
source identity, supported filters/codecs, dimension metadata and fill rules.
They do not solve irregular SWOT coordinates or NISAR's dataset-specific grid
semantics. Coordinate arrays may require a coarse selection followed by a
pixel/feature predicate. HDF5 compression does not guarantee safe raw chunk
addressability. Reader capabilities are per asset, not just filename suffix.

## Discovery and DuckDB

STAC API, STAC GeoParquet, direct manifests and CMR implement discovery
independently. Cache normalized descriptors separately from object layouts.
Use DuckDB for persisted large catalog/layout tables, bbox covering and time
predicates, column projection, deduplication and batched SQL tile selection.
Avoid loading assets JSON or huge footers unnecessarily; include live tails
and exact footprint tests. Direct asset requests need no SQL engine. The
prototype reuses E5's window math in Rust and retains SQL as an independent
manifest oracle; embedding DuckDB would add cost without serving this slice.

## Cost and execution policy

Start with bytes / measured throughput + ceil(requests / concurrency) * RTT
plus separately measured decode, transform and sink costs. This is an estimate,
not a latency guarantee; overlap and shared bottlenecks require calibration.
Report unknowns explicitly, including GDAL's actual range grouping. No learned
optimizer. Cache-build cost belongs in first-use totals; observed counters are
separate from planned compressed payload and ideal request counts.

ChronoZarr read-only lessons: performance.py caps requests using CPU/file
limits and backs off on failures/throttling; mosaic.py reserves memory before
admission, reads quality before data, splits into strips, streams writes and
clears failed GDAL URL-cache entries before retry. These are useful policies,
not evidence that 16 workers or Sentinel SCL applies universally. This slice
uses serial bounded chunks, library retries and explicit unsupported quality
operations. No hedging or adaptive controller until isolated measurements show
value. GDAL caches, codec allocations, output buffers and runtime overhead
must all be included when claiming an RSS limit.

## Decision gate

A shared logical DAG is feasible; a shared physical-byte abstraction is not.
The useful independent product is an explainable native extraction library,
with explicit fallback and a streaming sink contract. This is useful without
ChronoZarr. Existing tools cover most primitives; no unique throughput
opportunity is established. Next smallest useful milestone: one supported
public STAC collection → AOI/date/bands → validated native GeoTIFFs, with
polygon masks, authenticated locators, conditional metadata cache, cancellation
and a two-session tuned-reader comparison. Stop here after correctness and
local benchmark evidence; no universal engine, deployment or publication.

## Targeted current-source checks

No broad ecosystem re-audit was needed. The installed GDAL required Rust gdal
0.19 rather than 0.18: [the release changes](https://github.com/georust/gdal/blob/v0.19.0/CHANGES.md)
include GDAL 3.12 bindings. [GDAL's GTiff documentation](https://gdal.org/en/stable/drivers/raster/gtiff.html)
confirms native types, masks, compression and georeferencing remain reader
responsibilities. [The Zarr v2 specification](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html)
defines chunk keys, array metadata and fill values; the prototype's explicit
geospatial fixture convention is additional metadata. [HDF5's dataset API](https://docs.hdfgroup.org/documentation/hdf5/latest/group___h5_d.html)
exposes dataset reads and chunk address/filter information, supporting a
separate conditional chunk adapter rather than a universal byte-read claim.
[CMR's search API](https://cmr.earthdata.nasa.gov/search/site/docs/search/api.html)
is another discovery adapter, independent of file readers. These checks do
not update historical performance numbers or certify every upstream library.
