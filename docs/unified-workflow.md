# One acquisition workflow

Use `plan` to inspect, then `fetch` to acquire. The same request model and Rust
lifecycle support public Earth Search Sentinel-2 COGs and the defined local
Zarr v2 layout. Both preserve native values and grids. There is no implicit
resampling, reprojection, calibration, interpolation or temporal composite.

## Build and inspect

Requires Rust and system GDAL/PROJ discoverable through pkg-config. Dependencies
remain pinned; this milestone adds no dependency or crate.

```sh
cargo build --release --locked --bins --examples
./target/release/tile-ripper --help
./target/release/tile-ripper plan examples/unified-stac.json --human
./target/release/tile-ripper plan examples/unified-stac.json --json > prototype-output/stac-inspection.json
./target/release/tile-ripper fetch examples/unified-stac.json --output ./sentinel-subset --human
```

`sentinel-subset` must be a new directory. This small remote request discovers
B04/B08 over a Chapel Hill polygon on 2025-07-28. The provider's public HTTPS
assets need no signing. `plan` performs discovery, version checks and raster
metadata inspection but no extraction of source pixel windows. To inspect
catalog matches without opening raster metadata:

```sh
./target/release/tile-ripper plan examples/unified-stac.json --discover-only
```

Only the existing Earth Search `sentinel-2-c1-l2a` provider adapter is supported.
Pagination is complete within explicit caps; exceeding caps fails without silent
truncation. The AOI must be non-wrapping, within ±80° latitude and at most 5° per
axis. COGs are UInt16 GeoTIFF spectral assets. Scene footprint intersection is
only a discovery filter: the planner separately selects native pixels and
applies the documented pixel-center AOI mask. Geometry may be mapped through
GDAL/PROJ; raster values are never warped. Independent native grids stay in
separate files. Explicit internal validity/alpha masks remain unsupported;
source nodata/all-valid masks are supported, and polygon masking requires a
representable UInt16 nodata sentinel. No authentication adapter was added.

## Complete local Zarr example

Create the independent deterministic demonstration cube, then use the same
commands. Python creates test data here; it is not part of acquisition.

```sh
uv venv prototype-output/zarr-env
uv pip install --python prototype-output/zarr-env/bin/python --exclude-newer 2026-10-11 -r bench/zarr-requirements.txt
prototype-output/zarr-env/bin/python bench/zarr_workflow.py --fixture
./target/release/tile-ripper plan examples/unified-zarr.json --human
./target/release/tile-ripper plan examples/unified-zarr.json --json > prototype-output/zarr-inspection.json
./target/release/tile-ripper fetch examples/unified-zarr.json --output ./zarr-subset --human
```

The exact supported layout remains [the Zarr layout contract](zarr-workflow.md):
local unsharded v2 group, C-order little-endian UInt16 named `(time,y,x)` arrays,
none/gzip compression and no filters; explicit integer fill; Float64 x/y pixel
centers agreeing with a finite north-up affine and declared CRS; ascending
unique Int64 Unix-second time with the documented units/calendar. Variables
share coordinates, but may use different chunks and dot/slash chunk separators.
No coordinates are inferred from shape or filenames. Missing chunks use native
fill, while corrupt chunks fail. Auxiliary CF objects, other dtypes/codecs,
v3/shards, polygons/reprojected bounds and remote Zarr are unsupported.

Spatial bounds use the exact native CRS identifier and select the clipped outer
half-open pixel envelope, including fractional/partial source windows. Time
intervals are inclusive; explicit instants select existing timestamps, returned
in native coordinate order. Omitted time selects all observations. Output
coordinates preserve their stored numeric values and dtypes, including irregular
time gaps. Scale and offset are metadata, not instructions to modify stored DN.

## Shared request contract

Both examples use:

* `source`: `stac` with endpoint/collection/provider limits, or `assets` with
  opaque unique IDs, location and explicit `cog`/`zarr_v2` format. The original
  `catalog` source spelling now maps to STAC with conservative default limits.
* `spatial`: typed bounds or polygon with CRS; supported operations depend on
  source capabilities. Zarr accepts native bounds, COG also supports AOI masks.
* `time`: inclusive RFC3339 interval, explicit instants or null. STAC discovery
  currently requires an interval. Zarr resolves time against its coordinate
  array, not an invented per-asset date.
* `variables`: named STAC bands (`kind: named`), integer direct COG bands
  (`kind: band`), or Zarr dataset paths (`kind: dataset`). Mixed selectors/formats
  in one request are unsupported; issue separate native acquisitions.
* `grid: {"kind":"native"}`; nonempty quality filters, transformations,
  mosaics/composites or target grids fail explicitly before discovery/data I/O.
* `sink: {"kind":"directory","path":"./new-output"}` preserves each source
  format. Explicit legacy GeoTIFF/Zarr sink spellings still work for their
  supported source formats. `--output` overrides the destination path.
* `budget`: buffer_bytes, chunk_edge, max_nodes. Catalog `source.limits` adds
  item/page/asset/output-pixel caps and existing provider timeout/retry/GDAL
  cache settings. Buffer/DAG limits live only in `budget`; duplicate fields in
  catalog limits are rejected instead of silently ignored.

Limits admit bounded pixel buffers/chunks/coordinate axes and a bounded plan;
they are not hard RSS or wire-byte guarantees. Underlying GDAL/zarrs own I/O,
codecs and their additional allocations. Native Zarr chunks are retrieved once
and split into bounded callback planes, without a cache or scheduler. Callbacks
are synchronous and fallible; retaining buffers is the caller's responsibility.

## Library lifecycle

```rust
use tile_ripper::{acquisition, cog::LayoutCache, Request};
// request is a deserialized Request, identical to the CLI input.
let prepared = acquisition::prepare(&request, &mut LayoutCache::default())?;
let inspection = prepared.explain(); // safe machine-readable inspection
println!("{}", prepared.human());
let manifest = prepared.fetch()?;    // native outputs, validated runtime bindings
// Or prepared.execute(|chunk| { /* consume owned native values */ Ok(()) })?
```

Runnable for either example:

```sh
cargo run --release --locked --example acquire -- examples/unified-zarr.json
cargo run --release --locked --example acquire -- examples/unified-zarr.json --stream
cargo run --release --locked --example acquire -- examples/unified-stac.json --fetch
```

`acquisition::plan` and `acquisition::fetch` are convenience operations using
this same lifecycle. The original root `plan/execute/fetch` signatures and
format adapter APIs remain usable; root operations now also accept catalog
sources and native directory sinks. The old STAC facade is retained for Rust
compatibility, with `acquisition::from_stac` as an explicit migration helper.
There is one logical planner, not a second format-specific public planner.
A callback buffer is identified by asset, variable, nonspatial indices and
spatial window; `node_id` identifies the source read and can repeat when one
native chunk emits several bounded planes. Callback consumers can receive
partial data before an error and must discard/roll back their own partial work.

Inspection documents are tagged `inspection_only`, schema 3, and explicitly
not reusable execution plans. Runtime bindings and version checks are private;
there is no API or CLI to replay diagnostic JSON. A prepared object executes
its in-memory plan. New CLI invocations prepare again from the original request.
Local source sizes/mtime are validated before and after execution, covering
relevant metadata/coordinates and selected Zarr chunks, including absent keys.
Remote COGs use strong ETag conditional reads and the existing GDAL cache
rebinding safeguards. These checks are not an atomic local-store transaction
or a cryptographic content snapshot: keep input stores immutable during a job.

Serialized inspection/provenance strips URL queries, fragments and userinfo;
resolved locators remain runtime-only. Do not share request files that contain
credentials. Acquisition operations suppress URL-bearing native diagnostics with a scoped
thread-local GDAL handler and restore the host handler, including on unwinding.
This uses GDAL's [documented temporary handler stack](https://gdal.org/en/stable/api/error_handling.html),
not a global library override. The CLI also disables native debug logging.
Applications must avoid logging runtime bindings/request Debug values or
enabling credential-bearing transport debug logs. Coordinate concurrent
same-URL GDAL activity as documented in the STAC workflow.

## Read the inspection and result

JSON is the default; `--human` gives a concise source/variable/time/dimension
summary. JSON includes native CRS/affine windows, dimensions, COG tiles or exact
Zarr chunk keys, physical read backend, output strategy, geometry masks,
capabilities/limitations and metadata cache status. Unsupported requests return
an error rather than an executable-looking partial plan. Estimated encoded
payload/library reads are separate from observed planning times and execution
measurements. GDAL HTTP merging/retries are unknown unless externally observed;
local Zarr data-key counts exclude metadata and coordinate I/O.

A successful result has `document_type: acquisition_result`, `schema_version: 3`
and `status: complete`. Its manifest consistently provides:

* sanitized source IDs/references, item/band/acquisition provenance where present;
* output `file`, variable, timestamps, dimensions, native CRS/affine, canonical
  `uint16` dtype plus reader-native dtype label,
  nodata, radiometry metadata, mask decision and source selection;
* the inspection, processing decisions, limitations and observed counters/times.

Physical output formats stay native: STAC writes one tiled GeoTIFF per source
asset, direct COG retains its per-buffer GeoTIFF convention, and Zarr writes a
subset group per asset with one array record per variable. Coordinate arrays
are real Zarr arrays, not invented raster-band metadata. Multiple array records
can name the same group. No common pixel grid or temporal composite is imposed.
The source chunk grid and the output buffer grid are distinct.

Writers refuse existing output directories. Caught failures remove the newly
created output; cancellation stops callbacks. Local mutation found after writing
also invalidates/removes the result. The common completion manifest is published
by same-directory rename. A hard interruption can leave partial files (or an
older native-writer manifest lacking the schema-3 completion marker); these are
not complete acquisitions. There is no resume/checkpoint feature. Verify the
schema and completion status before consuming a result automatically.

## Migration and verification

`stac-plan` and `stac-fetch` are aliases for the shared commands. They accept
both old STAC request JSON and the common request model. Old direct asset
requests still work. CLI result JSON now uses the common envelope: read the
logical DAG from `.plan` and execution counters from `.observed`. Original
root Rust `plan` still returns the core `Plan`, and root `fetch` still returns
`ExecutionReport`. Historical benchmark JSON is retained without rewriting it.

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
prototype-output/zarr-env/bin/python bench/unified_workflow.py zarr
/path/to/study/experiments/.venv/bin/python bench/unified_workflow.py stac
```

Before/after benchmark reproduction also needs the 46dd503 executable at
`prototype-output/unified/previous-46dd503`. The recorded run captured that
existing release binary before rebuilding. To rebuild it independently, export
`git archive 46dd503` to a fresh ignored directory, build its Cargo.toml with
`--release --locked` and a separate target directory, and copy the executable
there. Results record hashes of both binaries, current Rust sources and lock.
Only local files and loopback STAC/HTTP mirrors are used in repeated benchmarks.

See [the decision gate](unified-decision-gate.md),
[Zarr physical-read/benchmark evidence](unified-zarr-benchmark.json) and
[STAC/GDAL benchmark evidence](unified-stac-benchmark.json). Production readers
fuse read/decode; independent transfer-only and decode-only probes are reported
separately and are not additive pipeline stages. No unobserved timer is invented.
