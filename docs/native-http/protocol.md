# Native HTTP efficiency protocol

The native performance baseline is the preserved `1473c4c7e454ad62deed22536c2f91253e72c8ce` consumer and CLI. `baseline.json` records the clean source tree at freeze, each production source hash, both Cargo lock hashes, the prior verification receipt, and copied binary hashes. The GDAL Tile-Ripper control is the independently preserved `911a268` consumer already used by the inherited COG benchmark. The standalone control is `bench/gdal_snapshot_worker.py`, with source validation before and after reading and conditional range requests.

`bench/native_http_benchmark.py` compares the frozen native binary, an explicitly selected frozen optimized native binary, the frozen Tile-Ripper/GDAL binary, and tuned standalone GDAL. It reuses the existing frozen immutable real Sentinel-2 fixture; it never captures public provider data. Mirror page verification must match `364121a9ef46437772834fae8a0d84797063320087bc1acbfdf154afb5e70122`.

The four original windows are unchanged: many-small, several-large, overlap-a and overlap-b. All readers receive the same assets, bands, source versions, native pixel bounds, 256-pixel chunk edge, and source order. The six-source cases use the first six frozen assets; many-small uses all frozen assets. Rust readers must produce identical ordered public chunk records, excluding internal node IDs. Independent rasterio/GDAL oracles verify sample checksums, valid pixels, native sums, dtype, nodata, transforms and full rectangle coverage outside every timed process. A successful complete record remains necessary.

The final optimized consumer is frozen at `prototype-output/native-http/phase-candidate-v2/tile-ripper-native-stats` with SHA-256 `a1cee1d9a56a5a2f2272b2b08f55209c2fbe49b7e4bfe93af4d971ac2365cf4e`. Its build receipt binds production source fingerprint `5975d41252fd57ea6adbbdc2efa76d5947a15bdef1ddbef3f64069ed4292a4a5`.

The default request has a 16 MiB Rust working budget, two requested workers, a per-host limit of four, and rendezvous streaming. The harness sets the actual `buffer_bytes` in every request and passes the requested worker count into the process; observed worker counts and scheduler reservations are retained separately. Rejected resource admissions are reported with the unchanged request, error, HTTP activity, process time and peak RSS. The harness never changes the budget to turn a failed trial into a pass. Standalone GDAL retains its documented 32 MiB raster cache and has no Rust reservation claim.

The optimized native strategy can retain a selection up to 2 MiB only when the first planned logical chunk already touches the selection's complete physical tile set. This preserves its first-read imagery fetch set. Phased reservations include transport buffers, upstream decode bounds and adapter delivery capacities. Coalescing is admitted only when spare working budget preserves the intended concurrency; the recorded strategy and actual capacity peaks make this choice visible. Large selections retain bounded windowed reading. The metadata prefix remains 4 KiB; separate 4/8 KiB and one-page tests record the request/byte tradeoff.

The original HTTP/1.1 loopback server and its existing `TCP_NODELAY` setting are unchanged. Each condition is either unshaped or 8 ms delay per HEAD/GET plus a shared 100 Mbit/s body link. There is no TLS or provider routing. Each reader is a fresh process. Standalone GDAL receives a separately timed selection pilot at each worker count 1, 2, 4 and 8 for each workload and network condition; the fastest pilot selects the standalone control's worker count. Three measured repetitions use deterministic randomized reader order. A later run can reuse recorded pilot decisions with `--tuning-from`.

Measurements retain process completion, first chunk, preparation, process CPU, peak process RSS, HEAD/GET counts, response-body bytes, scheduler reservation and worker peaks, native read/retrieval observations, and server request timing/concurrency/connection reuse. Peak RSS is measured by the operating system for the child process. Working reservations are the consumer's explicit scheduler counters; they are not inferred from RSS. First-chunk time is process-internal: Rust includes preparation, while the standalone worker starts its clock after Python imports. Full process completion includes those imports for all Python controls. The consumer completion record also supplies an internal acquisition duration; `outside_consumer_seconds` is process wall time minus that duration and can include process launch, request parsing and exit. It remains in the primary wall measurement.

Every HTTP response is matched against all physical TIFF blocks and metadata storage. Body byte totals include repeated ranges; they are never replaced by a merged-range union. The accounting separates selected compressed imagery, unselected imagery, exact TIFF metadata, padding/other bytes, and duplicate response bytes. Raw request records retain service durations, time to headers, per-socket identity, conditional headers and request start times. Per-socket gaps include caller work, decode, queueing and backpressure; they are diagnostic observations, not a direct measurement of a TCP delayed-ACK mechanism.

For an admission rejection, the physical receipt retains exact body bytes and raw ranges without applying the successful-trial requirement that every selected compressed block was fetched. No incomplete imagery is counted as scientifically validated.

Raw requests, full scientific chunk streams and measurement rows remain in ignored `prototype-output/native-http`. Public receipts bind harness and binary hashes and retain all trials, including failures. Timing comparisons should run without concurrent builds or other intensive validation. Lower request counts alone do not establish an improvement: compare elapsed time and full transferred bytes together.

## Acceptance reproduction

The preserved mirror, baseline executables and Python oracle environment are
local ignored fixtures. Reproduction requires those exact fixtures, not new
public-source captures. Build a separate candidate target without replacing the
baseline executables:

```sh
cargo build --locked --release --no-default-features --features native-cog \
  --bin tile-ripper --example native_cog_oracle --example native_sentinel_oracle \
  --target-dir target/native-http
cargo build --locked --release --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/native-http

prototype-output/zarr-env/bin/python bench/native_http_checks.py
prototype-output/zarr-env/bin/python bench/native_http_verify.py
prototype-output/zarr-env/bin/python bench/native_http_live.py

docker run --rm -v "$PWD:/src" -w /src rust:1.92-slim \
  sh bench/native_http_linux.sh
docker run --rm -v "$PWD:/src" -w /src python:3.13-slim \
  python bench/native_http_runtime.py
```

Run these commands sequentially. The snapshot fixture and feature verification
share Cargo's debug executable path; concurrent feature builds can replace the
CLI used by an integration test. Release candidate targets stay separate from
the frozen baseline. `native_http_verify.py` checks production source hashes
against `candidate.json`; it verifies this frozen candidate, not arbitrary later
edits. `native_http_live.py` redirects the inherited oracle's output so historical
native-Sentinel receipts are preserved. `platform.json` additionally records
image digests, build-resource logs and host linkage from this acceptance run.

## Controlled timing reproduction

```sh
# Establish the committed native baseline against both GDAL controls.
prototype-output/zarr-env/bin/python bench/native_http_benchmark.py \
  --label baseline-anchor --baseline-only --reps 3 \
  --output docs/native-http/baseline-benchmark.json

# Compare the candidate with the same three controls and the selected tuning.
prototype-output/zarr-env/bin/python bench/native_http_benchmark.py \
  --label optimized-decision --reps 3 \
  --native-binary prototype-output/native-http/phase-candidate-v2/tile-ripper-native-stats \
  --tuning-from docs/native-http/baseline-benchmark.json \
  --output docs/native-http/benchmark.json

# Targeted worker and budget matrix: every request keeps its declared budget.
prototype-output/zarr-env/bin/python bench/native_http_benchmark.py \
  --label resource-matrix --reps 1 --workloads overlap-b several-large \
  --workers 1 4 --budget-mib 8 16 32 \
  --modes native-baseline native-optimized \
  --native-binary prototype-output/native-http/phase-candidate-v2/tile-ripper-native-stats \
  --tuning-from docs/native-http/baseline-benchmark.json \
  --output docs/native-http/resource-benchmark.json
```

`bench/native_http_analysis.py` regenerates the final critical-path and resource-summary receipts from saved process rows and HTTP records, without starting consumers or timing trials. It binds its source, both input reports, and every raw input file it reads. Worker elapsed-time sums overlap in time; per-request injected delays cannot be added directly to wall time. Exact worker rendezvous wait is not separately measured.

```sh
prototype-output/zarr-env/bin/python bench/native_http_analysis.py
```
