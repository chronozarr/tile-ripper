# Native Zarr acquisition

The layout contract below remains current. For the current CLI, unified
provenance and source-chunk execution strategy, use [the unified guide](unified-workflow.md).
The dataset-slice read-amplification measurements here describe 46dd503.

The existing `plan`, `execute` and `fetch` Rust functions and `plan`/`fetch`
CLI commands now support one executable Zarr dataset layout. The source is
an **immutable local directory store**, read through `zarrs` and its existing
filesystem/object-key storage implementation. Remote Zarr, signing and cloud
credentials are not supported by this milestone. No downloader was added.

## Copyable demonstration

From this checkout, with system GDAL available:

```sh
cargo build --release --locked --bins --examples
uv venv prototype-output/zarr-env
uv pip install --python prototype-output/zarr-env/bin/python --exclude-newer 2026-10-11 -r bench/zarr-requirements.txt
prototype-output/zarr-env/bin/python bench/zarr_workflow.py --fixture
# Metadata and coordinate inspection only; no data-variable chunks read.
./target/release/tile-ripper plan prototype-output/zarr/request.json > prototype-output/zarr/plan.json
# Requires a NEW output directory.
./target/release/tile-ripper fetch prototype-output/zarr/request.json --output prototype-output/zarr/my-extract
# Library examples use the same Request and implementation.
cargo run --release --locked --example zarr_acquire -- prototype-output/zarr/request.json --stream
cargo run --release --locked --example zarr_acquire -- prototype-output/zarr/request.json
```

The fixture is a small reproducible scientific validation cube, generated with
Python Zarr independently of the Rust implementation: two raw UInt16 variables,
four irregular acquisition times, a projected 10 m grid, gzip compression,
explicit fill, differing variable chunk shapes, dot/slash keys and a deliberately
absent chunk. It is synthetic, not a public
satellite archive. Python is only needed to create the demonstration and run
its independent oracle; acquiring an already supported store uses the Rust CLI.

Edit the generated request's `spatial.bbox`, `time` and `variables` to retrieve
other subsets. Bounds are `[xmin,ymin,xmax,ymax]` in the exact native CRS string.
Any clipped spatial rectangle is supported, including fractional pixel bounds
and edges: selection is the outer half-open pixel envelope. Coordinates remain
pixel centers, in their original order. Intervals are inclusive RFC3339 times;
`{"kind":"instants","values":["2025-01-01T00:00:00Z","2025-01-05T00:00:00Z"]}`
selects arbitrary supported dates, returned in source order. Missing `time`
selects all times. Variables are top-level dataset paths, e.g.
`{"kind":"dataset","path":"red"}`. Different variables may have different
native chunk shapes, but must share all coordinates and the spatial grid.

## Defined layout

* Zarr v2 root group, unconsolidated metadata available (`.zgroup`, `.zattrs`).
* Root `tile_ripper_grid`: `{"crs":"EPSG:32617","transform":[500000,10,0,4000000,0,-10]}`.
  Transform is GDAL's pixel-corner affine convention: finite north-up regular
  pixels, positive x step, negative y step, no rotation. The CRS must parse.
* Named top-level data arrays: little-endian UInt16, C order, dimensions
  `time,y,x` in that order, explicit integer `fill_value`, no filters, either
  uncompressed or gzip. Dot and slash chunk separators work through zarrs.
  `_ARRAY_DIMENSIONS` is required. An optional `_FillValue` must agree with the
  storage fill. Supported scale/offset attributes are preserved, never applied.
* `x`,`y`: little-endian Float64 1D arrays, corresponding dimension names,
  strictly matching affine centers (tolerance 1e-8 of a pixel).
* `time`: little-endian Int64 1D ascending unique Unix seconds, dimension `time`,
  `units="seconds since 1970-01-01 00:00:00"` and
  `calendar="proleptic_gregorian"`. Irregular temporal gaps are allowed.
* No auxiliary coordinate/bounds/grid-mapping arrays or references to them.
  Metadata must describe this defined layout, not a larger CF dataset that
  depends on omitted objects. Arbitrary user attributes are copied verbatim;
  derived statistics/extent attributes are not recomputed. The explicit affine
  is the georeferencing authority and is updated for the extract.

Other dtypes, layouts, codecs, filters, polygons, reprojected bounds, calibration,
Zarr v3/shards, remote stores and compositing are unsupported. The original local
2D root-array **planning-only** probe and its tests remain available.

## Outputs, resource limits and API

A new directory contains `ASSET_ID.zarr` for each selected source group and
`manifest.json`. Each group contains the requested variables and subset native
coordinates. Stored values, dtypes, fill, scale/offset and array/group attributes
are preserved; shape, chunk sizes and affine origin change to describe the
extract. Data is re-encoded by zarrs, not copied byte-for-byte. Missing chunks
become native fill; existing corrupt chunks fail. Empty queries produce just a
manifest. No temporal composite or cross-source alignment is performed.

The manifest records the plan, native grids, source references, selected axis
indices, timing, observed data-store calls/bytes and processing decisions. Source
locators are local paths. Do not change a source store during plan/fetch; local
Zarr does not provide a transaction or snapshot check. Plans are diagnostic;
`fetch` and `execute` replan from the original request. Never execute untrusted
saved DAGs. A plan reads coordinate chunks and metadata, and stats selected data
objects for its **unique encoded payload estimate**; it does not read data-variable
chunks. Actual bytes can exceed the estimate when slices revisit a chunk.

`budget.chunk_edge` caps returned spatial planes; `buffer_bytes` admits at most
a quarter budget per returned buffer, half per encoded/decoded source chunk,
and a sixteenth per coordinate axis. Codec concurrency is one for data reads
and output writes; callbacks apply synchronous backpressure. This is admission
control for these working buffers, **not a hard RSS cap**: codecs, temporary
copies, metadata, plans, output codecs and host libraries add allocations.
Coordinate axes must fit; this is not a streaming coordinate-index implementation.
`max_nodes` bounds the DAG. No unbounded download or new transfer retry loop exists.
A callback failure stops execution. `fetch` refuses existing directories and
removes its newly created directory on caught errors; process death can leave an
incomplete directory. The manifest is written last as the completion marker.

The callback's `Chunk.variable` is the shared `Variable` selector; `indices`
contains source indices of nonspatial axes (the time index for this layout;
empty for COG). `datetime` identifies the selected observation. The owned `Vec<u16>` buffer
represents a native 2D plane for both formats; no GDAL buffer type appears in
the common callback API. `Selected.dimensions` lists
native shapes/chunk shapes, axis names and their selected source indices, without a Zarr-specific public
selection type. `Read::DatasetSlice` is executed by zarrs; COG keeps
`Read::LibraryWindow` executed by GDAL. Requests are homogeneous in supported
variable-selector semantics; mixed COG-band/Zarr-variable acquisition in one
request is not supported. Format-specific output sink variants remain explicit.

Diagnostic plan schema is now 2: `Layout.band` and `Chunk.band` were replaced
by `variable`; callback pixels now use `Vec<u16>`. Old version-1 layout cache entries are discarded and reinspected.
This is an intentional prototype API change; old request JSON still works.

## Independent validation and measurements

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
prototype-output/zarr-env/bin/python bench/zarr_workflow.py
```

The harness executes ten interleaved fresh processes (five Rust, five xarray),
validates raw values, dtypes, coordinates, attributes, fill, affine origin and
independently CF-decoded values/dates, and exercises additional edge/failure
cases. Results: [zarr-benchmark-results.json](zarr-benchmark-results.json).
Rust tests exercise the public shared planner/callback/fetch APIs, time slices,
missing chunks, cancellation, grid mismatch, codecs, malformed coordinates,
budgets and cleanup; existing COG/STAC tests remain in the same suite.

`source_data_requests/bytes` are observed zarrs data get/partial-get calls and
returned encoded bytes, from the library's UsageLog storage adapter, excluding
metadata and coordinates. Missing requests return zero bytes. These are local
object-key calls, **not HTTP requests or physical disk traffic**. The xarray
baseline instruments Zarr DirectoryStore data retrievals separately. Libraries
fuse encoded retrieval and decode; `read_decode_seconds` reports that combined
stage, not invented independent transfer/decode times. Wall time and per-child
peak RSS use fresh processes and include startup. Cache is warm; the tiny cube,
language startup and different output encoding/chunking prevent generalized
speed or memory claims. Discovery is absent for explicit assets.
