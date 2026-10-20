# Reproduce the scientific-output gate

Run from the `codex/acquisition-planner` worktree. Generated imagery, intermediate
arrays, sparse mirrors, logs and raw measurements stay in `prototype-output/`.
Receipts in this directory contain metadata, counts and hashes, not image samples.
Choose fresh output/work directories; completed datasets are never overwritten.

## Build and Rust checks

```sh
cargo build --locked --release --no-default-features --features native-cog \
  --target-dir target/scientific-output --bin tile-ripper --examples
cargo build --locked --release --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/scientific-output
cargo build --locked --release --features native-cog \
  --target-dir target/scientific-output-gdal --bin tile-ripper
cargo test --locked
cargo test --locked --no-default-features --features native-cog
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --no-default-features --features native-cog -- -D warnings
cargo check --locked --no-default-features
cargo fmt --check
```

The GDAL comparison build requires system GDAL. It is not the GDAL-free binary.

## Independent scientific and fault oracles

The macOS reference used Python 3.10.14 and the pinned packages in
`bench/scientific-output-requirements.txt` (including separate GDAL/PROJ readers).
Install them into an isolated reference environment, not the acquisition runtime.

```sh
TILE_RIPPER_OUTPUT_WORK=prototype-output/scientific-output/new-checks \
  prototype-output/zarr-env/bin/python bench/scientific_output.py checks
TILE_RIPPER_OUTPUT_WORK=prototype-output/scientific-output/new-live \
  prototype-output/zarr-env/bin/python bench/scientific_output.py live
TILE_RIPPER_OUTPUT_REGRESSION_WORK=prototype-output/scientific-output/new-regressions \
  prototype-output/zarr-env/bin/python bench/scientific_output_regressions.py
```

The controlled fixture uses original random UInt16 COGs, independent rasterio
read/rasterization, two UTM zones/resolutions, dates in reverse discovery order,
polygon holes and partial coverage. Faults are applied only to the local server
and attempt-owned output. Final mutation is injected on a single source's HEAD
after staged pixels exist. Signal tests wait for actual staged chunk files before
SIGINT/SIGKILL; they do not merely interrupt discovery.

The live gate calls ordinary CLI flags, then opens each completed group with
Zarr/xarray and compares every value/mask to independently decoded public source
windows. It verifies CF CRS reconstruction with pyproj, affine centers, exact
time, calibration and source identity. Current provider versions must match the
receipt for a direct checksum comparison. The footprint/densification convention
is documented in the native Sentinel workflow; no Tile-Ripper geometry or pixel
decoder is imported into the reference.

## Matched performance capture

The existing frozen `prototype-output/production-streaming/real/mirror` and its
source manifest/index are required. The benchmark never fills gaps or downloads
additional imagery. `Mirror.verify_pages()` checks all captured response hashes;
uncaptured ranges fail. Use the earlier production-streaming capture protocol to
prepare that input independently if it is absent.

```sh
TILE_RIPPER_OUTPUT_BENCH_WORK=/absolute/path/to/fresh/benchmark-work \
  prototype-output/zarr-env/bin/python bench/scientific_output_benchmark.py
```

Four assets (B04/B08 on 2 and 12 January 2025) use native pixel windows
`(7300,2100,96,96)` and `(6144,1024,2048,2048)`. Both builds use two workers,
two per host, 16-MiB admission, edge 256, a 32-MiB GDAL cache and one GDAL codec
thread. Three repetitions of four modes are shuffled for each of two windows and
two link conditions, totaling 48. No concurrent build is part of the final timing
matrix. The native stream writes descriptors, not a dataset; both Zarr modes use
the identical writer; the existing GeoTIFF mode has different auxiliary/durability
work. The parent uses `wait4` for CPU/RSS and wall time. The server counts every
HEAD/GET and response body. Reference decoding and verification run outside the
timed child. Every saved Zarr/GeoTIFF sample is checked after each output run.

On macOS the existing `bench/io_profile.c` interposer records output/manifest
calls. GDAL's buffered raster calls bypass it; do not interpret observed manifest
writes as a full GeoTIFF syscall count. Unique files and GDAL RasterIO calls are
reported separately. Counts do not measure physical device traffic. Repetition
noise, warm caches, shaping and this single workstation limit performance claims.

## Clean Linux install and independent reopen

The build uses `rust:1.92-slim`, an empty container target, the host Cargo registry
source cache, and the active source mounted at `/src`:

```sh
docker run --rm -v "$PWD:/src" -v "$HOME/.cargo/registry:/usr/local/cargo/registry" \
  -w /src rust:1.92-slim sh bench/scientific_output_linux.sh
docker run --rm -v "$PWD:/src" -w /src python:3.13-slim sh -c \
  'pip install --only-binary=:all: numpy==2.2.6 xarray==2025.6.1 zarr==3.1.3 numcodecs==0.16.3 && python bench/scientific_output_runtime.py'
```

Run the live oracle first so `docs/scientific-output/live.json` exists, and place
its unchanged `polygon.geojson` at `prototype-output/scientific-output/polygon.geojson`.
`TILE_RIPPER_LINUX_OUTPUT` can select a fresh absolute output path inside the
container for a repeat. The runtime checks GDAL/PROJ/libtiff absence, invokes the
source-installed CLI, compares source versions and all arrays/metadata against
the independent live reference, and writes `deployment.json`. Only the reference
environment contains geospatial Python packages. Runtime/source-install image
digests, source/binary hashes and checks are retained in the verification receipt.
