# Native COG decision experiment

Start: `codex/acquisition-planner` at `911a268`. Default reader remains GDAL.
No public data capture, reprojection, resampling, ChronoZarr, new formats, publication,
merge or deployment. Original frozen Sentinel-2 mirror index and bytes are verified
before replay. The baseline executable hashes must match the prior final report.

Compare the preserved 911a268 public Rust consumer, a GDAL-free Rust consumer using
`tiff-reader=0.9.0` / `geotiff-core=0.9.0`, and the existing independent conditional
GDAL control. Use the same native bounds, band, chunk size, source versions and
completion checks. Four frozen workloads: many-small (16 sources), several-large
and overlap-a/b (6 sources). Both unshaped loopback and 8 ms / shared 100 Mbit/s.
Fresh process per trial; tune GDAL at 1/2/4/8 workers before three randomized repeats
(seed 20261019). Keep failures. Linux deployment checks run separately from timing.

The existing independent rasterio oracle checks every chunk checksum, native sum,
validity, timestamp, dtype, scale/offset, affine/CRS, exact coverage and completion.
Compare native and baseline chunk records/order directly. Source transfer accounting
uses the existing physical TIFF-block interval analysis, including overviews and
metadata, to measure duplicate payload rather than infer it from request counts.

Record wall completion, first provisional chunk (internal, preparation included for
Rust; after imports for GDAL), CPU seconds / wall CPU percentage, process peak RSS,
GET/HEAD/body bytes, repeated physical blocks, layout/open time and read/decode time.
Native read/decode minus synchronous range time is an estimate of decoder + buffer
assembly overhead; separately replay local frozen files to isolate decode from HTTP.
Timer scopes are explicit: worker durations can overlap and cannot be added to wall.
192 MiB RSS and declared working-buffer reservations are distinct gates.

Fault/profile matrix: exact native UInt16 extremes/nodata, classic TIFF/BigTIFF,
byte order, None/Deflate/LZW, predictor 1/2, partial tiles, invalid profile rejection,
malformed IFDs/counts, corrupt/truncated payloads, decompression size errors,
HTTP 200-for-range, wrong Content-Range/length, missing or changed ETag, 412,
transient 503 and truncation recovery, exhausted retries, late mutation after
provisional delivery, cancellation and sink failure. No successful completion
receipt is allowed after a source conflict or decoding failure.

Deployment: compare executable file sizes, dependency trees, dynamic dependencies,
startup with help/empty process, clean native build in an official Rust Linux
container with no GDAL/PROJ/libtiff installation, run the actual consumer there,
and inspect its ELF dependencies. Record the remaining vendored C codec/build-tool
requirements separately from shared-library runtime requirements. Test macOS ARM64
and Linux ARM64; other platforms remain unverified.

## Reproduction

The native benchmark and fault harnesses use the same existing
`prototype-output/zarr-env` Python oracle environment as the previous milestone.
Keep captured sparse mirrors and baseline binaries; `native_cog_benchmark.py`
verifies their digest and refuses a baseline consumer hash mismatch.

```sh
cargo build --locked --release --no-default-features --features native-cog
cargo build --locked --release --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target
# Preserve these two executables under prototype-output/native-cog/native/.
# Preserve the 911a268 executables under prototype-output/native-cog/baseline/.
prototype-output/zarr-env/bin/python bench/native_cog_checks.py
prototype-output/zarr-env/bin/python bench/native_cog_benchmark.py --label new-matrix
prototype-output/zarr-env/bin/python bench/native_cog_benchmark.py --label new-local --local
prototype-output/zarr-env/bin/python bench/native_cog_deployment.py
TILE_RIPPER_NATIVE_REQUEST="$PWD/prototype-output/native-cog/checks/profile-deflate-2-NO-LITTLE.json" \
  cargo test --locked --no-default-features --features native-cog --test native_cog -- --ignored
```

For clean Linux builds, generate `prototype-output/native-cog/linux-request.json`
from a frozen request with local mirror paths rooted at `/src`, then use empty
`linux-cli-target` and `linux-consumer-target` output directories:

```sh
docker run --rm -v "$PWD:/src" -w /src rust:1.92-slim \
  sh /src/bench/native_cog_linux_build.sh
docker run --rm -v "$PWD:/src" -w /src python:3.13-slim \
  python /src/bench/native_cog_linux_runtime.py
```

The runtime harness checks GDAL/PROJ/libtiff absence, executes all sixteen original
small-window sources, inspects `ldd` and measures startup. These are local
containers, not a deployment. Reusing artifacts across disposable builder
containers caused a stale Rust proc-macro dependency failure in one repeat; the
versioned recipe uses separate clean target directories for the CLI and consumer.
The original failed logs remain in ignored `prototype-output/native-cog`.

Direct equality check (32,391,168 UInt16 samples across all four real workloads):

```sh
cargo build --locked --release --no-default-features --features native-cog --example native_cog_oracle
prototype-output/zarr-env/bin/python bench/native_cog_exact.py
```

The runtime check consumes the independent `exact/many-small.bin` oracle file.
The benchmark-only Rust consumer compares samples directly, with no checksum
collision assumption and no image encoding. Oracle generation/comparison is
excluded from acquisition timings.

The separate source-install test uses another empty target and no GDAL packages:

```sh
docker run --rm -v "$PWD:/src" -w /src rust:1.92-slim \
  sh /src/bench/native_cog_linux_install.sh
```
