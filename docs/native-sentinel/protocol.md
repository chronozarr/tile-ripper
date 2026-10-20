# GDAL-free Sentinel acceptance protocol

Start from `40518b5` on `codex/acquisition-planner`. Keep the default GDAL feature
and compatibility reader. Scope is WGS84 longitude/latitude polygons and bounded
Sentinel-2 WGS84 UTM native grids, storage-domain UInt16 values and existing stream
completion. No raster reprojection, compositing or new storage formats.

Validate geometry with established Rust polygon operations and coordinate
transforms; independently execute GDAL/PROJ references in the oracle environment.
Exercise holes, concavity, reversed winding, exact pixel-center edges, partial
coverage, nodata, UTM zones and hemispheres, unsupported CRS and invalid geometry.
Compare native windows, affine coordinates and every delivered UInt16 sample.
Rasterize the independent GDAL reference on the full source grid, then crop;
verify that native inclusion is invariant to delivery chunk size. Exact center
ties at large UTM/pixel offsets must not depend on a subwindow's floating origin.

Run a bounded real Earth Search polygon/date/two-band request through the public
Rust consumer. Record item/date/band order, source URLs and strong ETags, geographic
AOI and native grids. Successful completion requires the final version check.
Exercise cancellation and late mutation after provisional delivery on controlled
HTTP sources. Re-run existing native COG science/fault and Zarr regression gates.

Build/install from source in an official Rust Linux container with no GDAL, PROJ
or libtiff. Run the real STAC consumer in a separate minimal runtime, inspect
dynamic linkage and measure RSS separately from declared working-buffer use.
Retain raw images/logs only under ignored `prototype-output/native-sentinel`;
commit compact receipts, reproduction scripts and documentation.

Request optimization requires physical traces establishing avoidable requests,
bounded memory, exact transferred-byte accounting and preserved conditional
versions. Compare the existing equivalent frozen four-workload benchmark with
tuned GDAL before recommending a default change or private-alpha trial.

## Reproduction

```sh
cargo test --locked --all-features
cargo test --locked --no-default-features --features native-cog
cargo test --locked --no-default-features
cargo clippy --locked --all-features --all-targets -- -D warnings
cargo clippy --locked --no-default-features --features native-cog --all-targets -- -D warnings
cargo build --locked --release --no-default-features --features native-cog \
  --target-dir target/native-sentinel --bin tile-ripper --examples
cargo build --locked --release --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/native-sentinel
prototype-output/zarr-env/bin/python bench/native_sentinel_checks.py
prototype-output/zarr-env/bin/python bench/native_sentinel_workflow.py
prototype-output/zarr-env/bin/python bench/native_sentinel_live.py
prototype-output/zarr-env/bin/python bench/native_sentinel_regressions.py
docker run --rm -v "$PWD:/src" -w /src rust:1.92-slim \
  sh /src/bench/native_sentinel_linux.sh
docker run --rm -v "$PWD:/src" -w /src python:3.13-slim \
  python /src/bench/native_sentinel_runtime.py
```

The independent reference environment includes rasterio 1.4.4 / GDAL 3.10.3 /
PROJ 9.7.1 and Shapely 2.1.2. It is only for validation. The native consumer has
none of these Python/geospatial dependencies. Source installation uses an empty
container-local target, avoiding host-shared proc-macro artifact failures observed
with the first mounted-target build. Raw failed and successful logs are retained.
