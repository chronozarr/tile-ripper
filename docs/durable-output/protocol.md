# Reproduce the durable-output gate

Run in the `codex/acquisition-planner` worktree. The parent is `82e21b1`.
Everything generated stays in ignored `prototype-output/durable-output/` and
`target/`; compact receipts go in this documentation directory. Use fresh work
directories for independent gates. Acquisition never overwrites a dataset.

## Preserve/build the original writer

The measurement used the original binary preserved before edits, with SHA-256
`a44a42df44908ef93d3c895e4022fe28a6f7690acb0d575167227d5333b6be62`.
To rebuild that source independently without switching the active worktree:

```sh
mkdir -p prototype-output/durable-output/baseline-source
git archive 82e21b1 Cargo.toml Cargo.lock src examples \
  | tar -x -C prototype-output/durable-output/baseline-source
cargo build --locked --release --no-default-features --features native-cog \
  --manifest-path prototype-output/durable-output/baseline-source/Cargo.toml \
  --target-dir prototype-output/durable-output/baseline-target --bin tile-ripper
cp prototype-output/durable-output/baseline-target/release/tile-ripper \
  prototype-output/durable-output/baseline-82e21b1
```

Record the rebuilt binary's hash/toolchain; different build paths can change
binary hashes. Do not copy the optimized binary into the baseline slot. Both
versions must retain their actual file/directory synchronization policies.

## Builds and Rust checks

The exercised macOS and Linux toolchain is Rust 1.92.0. macOS default/all-feature
tests also require system GDAL. Codec C/C++ build tools remain required.

```sh
cargo build --locked --release --no-default-features \
  --features native-cog,experimental-output --target-dir target/scientific-output \
  --bin tile-ripper --examples
cargo build --locked --release --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/scientific-output
cargo test --locked
cargo test --locked --no-default-features --features native-cog
cargo test --locked --no-default-features --features native-cog,experimental-output
cargo test --locked --all-features
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --no-default-features \
  --features native-cog,experimental-output -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo check --locked --no-default-features
cargo fmt --check
git diff --check
```

The experimental feature enables the existing zarrs sharding code, not a new
reader or an ordinary CLI default. The root and separate consumer lock files
retain all previous registry versions/checksums and add only the scoped path
copy of zarrs_filesystem 0.3.13. Upstream source licenses and the local change
description are under `vendor/zarrs_filesystem/`.

## Independent environments

Reuse the existing Python 3.10 / Zarr-Python 2.18.3 reference environment at
`prototype-output/zarr-env`. Its pinned packages are in
`bench/scientific-output-requirements.txt`. Legacy direct-Zarr regression
harnesses depend on Zarr-Python 2's `DirectoryStore`.

The second independent environment uses Python 3.13:

```sh
uv venv --python 3.13 prototype-output/durable-output/reader3
uv pip install --python prototype-output/durable-output/reader3/bin/python \
  numpy==2.2.6 xarray==2025.6.1 zarr==3.1.3 numcodecs==0.16.3 \
  rasterio==1.4.4 shapely==2.1.2 pyproj==3.7.1 requests==2.32.5
```

No Python dependencies are added to acquisition. These environments and all
imagery remain outside version control.

## Scientific, signal and memory gates

```sh
TILE_RIPPER_DURABLE_WORK=prototype-output/durable-output/checks-final \
  prototype-output/durable-output/reader3/bin/python \
  bench/durable_output_validation.py checks
TILE_RIPPER_DURABLE_WORK=prototype-output/durable-output/checks-final \
  prototype-output/zarr-env/bin/python bench/durable_output_validation.py reopen-v2
TILE_RIPPER_DURABLE_WORK=prototype-output/durable-output/live-final \
  prototype-output/durable-output/reader3/bin/python \
  bench/durable_output_validation.py live
TILE_RIPPER_DURABLE_WORK=prototype-output/durable-output/faults-verified \
  prototype-output/zarr-env/bin/python bench/durable_output_validation.py faults
TILE_RIPPER_DURABLE_WORK=prototype-output/durable-output/memory-final \
  prototype-output/durable-output/reader3/bin/python \
  bench/durable_output_validation.py memory
TILE_RIPPER_DURABLE_WORK=prototype-output/durable-output/regressions \
  prototype-output/zarr-env/bin/python bench/durable_output_validation.py regressions
```

Use those exact work names only for a first reproduction in a clean output
tree; choose new names for further runs. `reopen-v2` intentionally points at
the already completed `checks` work directory and performs read-only reopening.
The earlier native/scientific report files remain unchanged by these gates.

`checks` replays the inherited actual mid-pixel-write signal cases and adds
independent mixed-grid comparisons for raw edges 128/512, gzip 256, Blosc/Zstd
256 and experimental sharded gzip 256. It tests empty/tiny selections, late
ETag mutation after pixels exist, source errors, subsecond times, calibration,
nodata and AOI masks. Source values/geometry come from independent GDAL/PROJ
readers; no Tile-Ripper raster/geometry implementation is imported into the
reference calculation. The inherited helper imports are lazy so the same
scientific oracle can run under either Python/Zarr version.

`live` first uses ordinary CLI flags for the six-observation January 2025
Sentinel-2 selection, then repeats the same selection through each encoding
candidate. Each output is fully checked against public source windows with
pinned versions. It validates B04/B08/B11, two dates, 10/20 m grids, exact time,
CRS, affine centers, raw UInt16, both masks and TIFF/STAC provenance. The
geographic polygon densification convention is inherited from the earlier
native Sentinel workflow. Results for changed provider versions must be
requalified; hashes are not assumed invariant across changed source objects.

`faults` compiles `bench/durable_output_io.c` on macOS. Its environment variables
are test interposer controls, never production options. It pauses immediately
before a real syscall and writes an observable marker. Only then does the
parent inject SIGINT/SIGKILL or create the competing destination. EIO injection
covers both writing handles and final transaction calls. Pre-commit kills leave
only an identifiable hidden sibling; retries ignore it. Post-commit failures
retain a complete, independently readable result and cannot claim durable
success. A SIGINT inside the rename call is already past the final cancellation
poll and may commit. The harness deliberately tests that boundary.

`memory` constructs deterministic 1024- and 8192-square TIFFs in 256-square
tiles. Two timestamps yield 8 and 512 MiB of logical values/masks. Both source
creation and independent every-pixel verification are tile-bounded. Each child
is measured by `wait4`; the gate checks RSS below 192 MiB, less than 64 MiB of
growth and a constant declared encode reservation. Do not interpret those
screens as an aggregate 16-MiB RSS guarantee or unlimited-node proof.

`regressions` replays the existing 61 native COG profile/fault cases, the full
32,391,168-sample real-source exact oracle, and seven public-consumer cases for
UInt16/Float32 Zarr, axis order, missingness, cancellation and version mutation.
Their frozen range mirror and original source receipt must already exist.

## Matched performance matrix

The unchanged sparse mirror under
`prototype-output/production-streaming/real/mirror` and its source manifest are
required. `Mirror.verify_pages()` checks all captured response hashes and the
index digest. The benchmark neither downloads new imagery nor fills uncaptured
pages; an uncaptured range fails closed. Original COG offsets and compressed
bytes are preserved. Reference reads use only verified captured windows.

After **all builds, gates and other jobs from this task have finished**:

```sh
TILE_RIPPER_DURABLE_WORK=prototype-output/durable-output/matched-final \
  prototype-output/durable-output/reader3/bin/python \
  bench/durable_output_benchmark.py benchmark --reps 3
```

Four original B04/B08 observations on two dates use pixel windows
`(7300,2100,96,96)`, `(6144,1024,512,512)` and
`(6144,1024,2048,2048)`. Each has three shuffled repetitions on warm local HTTP
and an 8-ms/100-Mbit/s simulated link. Eight modes make 144 cells: original v2,
optimized raw v2, gzip v2, Blosc/Zstd v2, output-only edges 128 and 512, native
stream and experimental v3 sharding. Source planning/delivery stays at edge
256, so the layout experiment includes any destination read/modify/write work.

Each fresh child reports wall time, CPU and RSS. The fixture counts all
HEAD/GET requests/body bytes. Native reports retain working reservations and
source read/decode/retrieval durations. The interposer counts and times real
file opens/creates, reads, writes, host fsync, full device flushes, mkdirs and
rename; task-diagnostic files are excluded and ancestor directory syncs are
included. These are not device IOPS. External stage traces distinguish setup,
manifest generation/serialization/write, final staging/ancestor sync,
pre-publication flush, rename, parent sync and post-publication flush.

Every successful output is fully read by Zarr-Python and xarray and compared
against the independent GDAL source windows, then receives full/small-subset
warm read wall/CPU probes. Validation and reading are outside write timing.
Generated datasets are removed only after verification; compact per-run records
are saved after every cell. Completed cells can be resumed from `runs.json`;
do not mix binary versions or source mirrors when resuming. Binary hashes are
checked across each invocation; final receipts retain their identities.

The known experimental error `Sharding did not allocate a large enough buffer`
is retained as a failed, unpublished attempt, never a successful fast duration.
Any unexpected error, stable-mode failure, leaked staging or independent
value/mask mismatch aborts the benchmark. The final overall `passed=false`
records the rejected sharding candidate, while `completed=true` and
`stable_modes_passed=true` distinguish the completed/adoptable v2 gate.

The `baseline` action provides a smaller original-writer/stream exploratory
matrix. Its old writer encode-only field is uninstrumented; do not misinterpret
its default zero as measured zero encoding work.

## Linux build and independent reopen

The source test/build uses a fresh temporary target in a disposable Linux
ARM64 container, the existing host registry source cache, and no geospatial
libraries. Linux timings are not part of the macOS performance claims.

```sh
docker run --rm -v "$PWD:/src" \
  -v "$HOME/.cargo/registry:/usr/local/cargo/registry" -w /src \
  rust:1.92-slim sh bench/durable_output_linux.sh
docker run --rm -e TILE_RIPPER_LINUX_LABEL=linux-verified \
  -v "$PWD:/src" -w /src python:3.13-slim sh -c \
  'pip install --only-binary=:all: numpy==2.2.6 xarray==2025.6.1 zarr==3.1.3 numcodecs==0.16.3 && python bench/durable_output_linux.py'
```

The live macOS receipt must exist first. `TILE_RIPPER_LINUX_LABEL` chooses fresh
output names. The separate Python runtime contains no rasterio/GDAL/PROJ/libtiff;
it matches every raw-value/mask hash, timestamp and scientific CRS attribute
against the earlier independent GDAL receipt. V3's legacy dimension-name
attribute is normalized only after independently asserting scalar/image ranks.
Container mount behavior tests application publication, not power-loss safety.

## Compact receipt derivation and candidate API

The checked run's raw logs are under `prototype-output/durable-output/`; their
names are explicit in the summary script. To derive profile, failure and build
receipts from those artifacts:

```sh
prototype-output/durable-output/reader3/bin/python bench/durable_output_summary.py
```

The API for opt-in v2 compression/layout experiments is:

```rust
prepared.fetch_scientific_with_options(
    tile_ripper::ScientificOutputOptions {
        chunk_edge: 256,
        encoding: tile_ripper::ScientificEncoding::BloscZstd,
    },
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
)?;
```

The supplied `durable_output_candidate` example accepts a normal JSON
COG/STAC `zarr_directory` request, `raw|gzip|blosc_zstd|sharded`, and the output
edge. Its `sharded` choice requires the experimental build feature and remains
subject to the documented real-data encoder failure. Ordinary `fetch` remains
uncompressed Zarr v2; there is no new ordinary CLI v3 default or source reader.
