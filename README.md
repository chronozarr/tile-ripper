# Tile-Ripper

A Rust acquisition planner with a working Sentinel-2 STAC-to-pixels workflow,
a native GeoTIFF reader path, and local/anonymous HTTP Zarr v2 acquisition paths. One library
and a thin CLI, with GDAL and zarrs handling storage and decoding. No ChronoZarr dependency.

Start with [the unified workflow](docs/unified-workflow.md):

```sh
tile-ripper plan request.json --human
tile-ripper fetch request.json --output ./subset
```

Complete requests: [remote STAC/COG](examples/unified-stac.json) and
[local Zarr](examples/unified-zarr.json), and
[controlled HTTP Zarr](examples/remote-zarr.json). All use the same prepared Rust library
lifecycle and preserve native grids. See [the current decision gate](docs/remote-zarr-decision.md) and
[the HTTP Zarr demonstration and supported contract](docs/remote-zarr-workflow.md).
The [public-data compatibility assessment](docs/remote-zarr-public-compatibility.md)
explains why arbitrary public Zarr is not yet supported.
Earlier milestone guides and benchmarks remain historical evidence;
[ADR 0003](docs/0003-unified-lifecycle.md) describes the unified lifecycle.
The original [study](study/README.md) remains historical evidence.

## Build and demonstrate

Requires Rust/Cargo and system GDAL discoverable through `pkg-config` (tested
with GDAL 3.12.4). Dependencies are pinned in Cargo.toml and Cargo.lock; use
`--locked`. GDAL and its native dependencies are supplied by the host, not
vendored or reproducibly pinned by Cargo.

```sh
cargo build --release --locked --bins --examples
# Use the existing study/experiments Python environment, or uv sync --locked there.
/path/to/study/experiments/.venv/bin/python bench/make_fixture.py prototype-output/fixtures
./target/release/tile-ripper plan prototype-output/fixtures/small.json --cache prototype-output/layout.json
./target/release/tile-ripper fetch prototype-output/fixtures/small.json --output prototype-output/my-extract
./target/release/tile-ripper plan prototype-output/fixtures/zarr.json
```

The fixture creator writes complete example requests and deterministic source
pixels. `fetch` requires a **new** output directory. It writes one GeoTIFF per
native buffer and a manifest linking each file to its asset, band, window and
time. Outputs remain separate observations; there is no mosaic or composite.
`plan` prints diagnostic JSON without source locators or credentials. It may
read metadata; it does not retrieve pixel windows. Saved plans are not replay
inputs: `fetch` uses the original runtime request and replans.

Minimal request (replace the local source with your own north-up UInt16 TIFF):

```json
{
  "source": {"kind":"assets", "assets":[
    {"id":"observation-1", "locator":"/absolute/path/source.tif", "format":"cog",
     "datetime":"2025-06-01T00:00:00Z"}
  ]},
  "spatial":{"kind":"bounds", "crs":"EPSG:32617", "bbox":[601000,3997420,602600,3999020]},
  "variables":[{"kind":"band", "index":1}],
  "grid":{"kind":"native"},
  "sink":{"kind":"geotiff_directory", "path":"./new-output"},
  "budget":{"buffer_bytes":524288, "chunk_edge":512, "max_nodes":10000}
}
```

Bounds must already use the source CRS authority string, in x/y order. They
select the outer pixel envelope, clipped to the source. `time` is optional;
when given, timestamps must be RFC3339 and source datetimes must be supplied.
Intervals include both endpoints. HTTP(S) sources use GDAL `/vsicurl/`.
Runtime request files can contain sensitive locators; handle them accordingly.
The CLI suppresses native driver diagnostics that could echo signed locators.
Local layout caches contain canonical local paths but no remote locators.

## Library

```rust,no_run
use tile_ripper::{Request, Result, execute, cog::LayoutCache};
fn acquire(request: &Request) -> Result<()> {
    let mut cache = LayoutCache::default();
    execute(request, &mut cache, |chunk| {
        // Georeferenced owned UInt16 buffer. Returning an error cancels the run.
        println!("{}: {} pixels", chunk.asset_id, chunk.pixels.data().len());
        Ok(())
    })?;
    Ok(())
}
```

`execute` calls the sink synchronously, providing backpressure. It is not yet
an async stream or an incremental planner. Library hosts own GDAL process
configuration and diagnostics; the CLI's conservative settings are visible in
src/main.rs. `buffer_bytes` bounds one returned pixel buffer. It **does not**
cap process RSS, GDAL's caches/codec working space, the metadata cache or a
caller that retains buffers. `max_nodes` bounds the materialized plan. A separate prototype metadata cap
limits inspection and the local layout cache to 65,536 tile records; loading
a cache larger than 32 MiB is rejected.

## What works and what does not

- COG/GeoTIFF: direct assets, source-CRS bounds, integer bands, optional time
  selection, native UInt16 buffers/GeoTIFF outputs, local layout cache.
- Zarr v2: planning only, local 2D array metadata, C order, `.` or `/` chunk
  separators. Explicit `tile_ripper_grid` CRS/affine attributes are a prototype
  fixture convention; `.zarray` itself has no universal georeferencing.
- The STAC facade now supports Earth Search discovery, AOI CRS mapping, polygon
  masks and per-asset outputs; see its separate request contract above.
- Unsupported: other provider adapters, target grids, other dtypes,
  quality masks, internal mask/alpha TIFFs, Zarr materialization/v3 sharding,
  HDF5/NetCDF and GeoParquet reads, mosaicking and temporal reduction.
- TIFF ranges shown in plans are explanatory block-layout evidence. GDAL
  chooses actual HTTP reads. Request/byte observations are null unless
  measured externally; payload estimates are not wire-byte guarantees.
- Persistent remote layout caching is disabled until ETag/version validation
  and conditional execution exist. Local identity uses size/mtime, not a
  content hash. The STAC workflow now pins strong ETags through conditional reads;
  legacy direct requests without ETags still assume unchanged remote objects.

## Verification and benchmarks

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
/path/to/study/experiments/.venv/bin/python bench/compare_study.py
/path/to/study/experiments/.venv/bin/python bench/run.py --reps 5
```

The parity check reruns the original SQL and compares Rust windows and tile
offsets/counts against its saved manifests. The benchmark creates local COG
fixtures, interleaves fresh processes, independently checks all output pixels
and metadata, and counts HTTP requests/response-body bytes at a loopback
server. It records process peak RSS, planning/read+decode/sink timings and wall
time, fixture/lock hashes and versions. GDAL fuses read and decode, so those
times are combined. Direct-asset discovery has no catalog request cost.

Cold/warm refers to local persisted layout metadata. OS caches stay warm.
HTTP runs are index-free; no false warm-remote label is used. GDAL versions
differ between the host Rust binding and the existing rasterio environment.
Python import and output-manifest overhead differ too: these measurements
validate behavior/accounting, not a Rust-vs-Python speed claim. Rasteret and
the original direct range executor remain prior-study baselines; they were
not rerun on this new fixture. Results and limitations are in docs/.
