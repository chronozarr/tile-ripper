# Acquisition engine benchmark protocol

This benchmark compares acquisition execution under controlled local conditions.
It is designed to test the scheduler's intended workload: many independent
small COG windows, a few large windows, chunked Zarr reads, and repeated
partially overlapping requests. It does not establish performance on public
providers or a universal speed advantage.

## Controlled transport

A local HTTP/1.1 server serves immutable fixture objects with keep-alive,
byte-range responses, stable ETags, configurable per-request latency, optional
injected transient failures, and one shared aggregate bandwidth limiter across
all client connections. The server records request attempts, unique requests,
HEAD/GET/range counts, retries, conditional requests, and response-body bytes
under a lock. Server counters are reset between runs. The limiter is shared so
increasing concurrency cannot multiply the configured link rate. Never use
public endpoints for repeated timing runs.

The principal COG fixture is a deterministic local COG mirrored by the server.
It is synthetic and Sentinel-like in layout and UInt16/native-window behavior;
it is not a Sentinel observation. The default 16 asset IDs point to identical
fixture bytes at separate loopback routes, so this tests scheduler fan-out and
does not model scene-to-scene data diversity. Report that distinction in every
summary. Zarr fixtures are also synthetic. If a genuine local Sentinel scene is available,
record its provenance, license, hash, dimensions, and acquisition metadata before
substituting it; do not label mirrored synthetic data as real Sentinel data.

## Workloads and readers

Run at least these cells:

* Many small COG assets with small intersections, representing parallel
  independent Sentinel-like items.
* A few COG assets with large windows, to expose sustained transfer and decode.
* Zarr selections touching hundreds and roughly one thousand chunks.
* Repeated acquisitions with substantial spatial overlap, with no persistent
  output cache unless the workload explicitly evaluates cache reuse.

Compare the Tile-Ripper binary captured before the scheduler change, the
scheduler build, a threaded rasterio/GDAL worker, and an xarray/Zarr reader.
The GDAL comparison uses a separate concurrency pilot at 1, 4, and 8 workers;
it writes the same native 256-pixel window chunks as Tile-Ripper, as uncompressed
striped GeoTIFFs with matching values and metadata. It is a tuned independent
reader, not a snapshot-equivalent acquisition path: it does not pin object
versions or send Tile-Ripper's conditional range validation. Record the selected
setting and GDAL version. Keep the baseline executable in a separate path and
record its hash. All readers use the same request, source bytes, output semantics,
and shared link conditions wherever applicable. The xarray HTTP comparator is an
explicit reader-only control: its four-request connection limit matches the Rust
per-host bound, but fsspec does not pin ETags or validate source versions with
`If-Match`. Rust baseline/current rows are the matched snapshot-validation
comparison. The xarray writer applies Darwin `F_FULLFSYNC` to each completed Zarr
object, matching the repository's durable writer; its write time includes syncs.

Measure process wall time, time to first completed output (TTFC), total
completion time, output creation/flush time where observable, peak RSS, output
bytes, and all transport counts. Wall time includes process startup and is not
a decomposition into exact download/decode costs. The OS page cache is warm
after the first fixture pass; do not flush it between readers. A completed
output file's appearance is recorded separately from usable-output timing.
Tile-Ripper reports first output relative to execution; adding reported planning
time still omits process startup. The rasterio worker reports first output when
its first chunk file closes. xarray reports after its first complete Zarr store
is written. Keep these scopes explicit and do not label file creation as usable
TTFC.

The rasterio comparator uses rasterio 1.4.4 and its bundled GDAL; the Rust
binary's GDAL version may differ. Record both. The Zarr environment is Python
3.10 with xarray, Zarr 2, NumPy, rasterio, fsspec and aiohttp; pin and report
the actual installed versions. Output timing is the reader's own reported sink
or write time where available. Readers do not expose identical final manifest
boundaries.

Record failures, timeouts, retry exhaustion, and incomplete outputs as rows; never skip failed runs while
summarizing successful ones.

## Correctness and repeatability

Use deterministic fixture seeds and hash every fixture and executable. For COGs,
compare each written sample to an independent rasterio/GDAL reference window,
including native dtype, nodata, affine transform, CRS, scale and offset. For
Zarr, derive expected chunk keys from coordinate selections and native chunk
shapes, then compare values, dimensions, coordinate values/order, dtype, fill
semantics, and time coordinates against an independent xarray selection. Require
exact equality for native unscaled values and coordinates. Correctness checks
run for every timed output and their separate reference-read time is excluded
from acquisition measurements.

Interleave reader order with a fixed seed. Use multiple fresh-process
repetitions, reset counters and output directories each run, and report raw
per-run JSONL plus grouped medians, ranges, sample standard deviations, and
failure counts. Record OS/Python/Rust/GDAL/xarray/Zarr versions and environment
overrides. State OS page-cache status and workstation conditions. A bandwidth
or request-latency setting is a reproducible scenario, not a claim about any
specific public provider.

## Interpretation

Report observed network bytes and requests separately from planned tiles/chunks
and estimated payload. The server's body-byte counter is not wire bytes. Do not
attribute fused reader time to download, decode, or writing without an
independent stage probe. Differences in startup, metadata inspection, output
format, and dependency versions limit wall-time ratios. Treat the benchmark as
scenario evidence and retain all raw runs for reanalysis.

## Commands

The baseline executable must be captured before rebuilding the crate. Verify
its hash in the emitted report. Create or reuse the repository's isolated Zarr
environment and install this benchmark's additional reader dependencies:

```sh
uv pip install --python prototype-output/zarr-env/bin/python \
  rasterio==1.4.4 fsspec==2026.4.0 aiohttp==3.13.3 requests==2.32.5
```

Run a short end-to-end smoke before the repeated matrix:

```sh
prototype-output/zarr-env/bin/python bench/acquisition_engine.py \
  --smoke --reps 1 --assets 4 --latency-ms 2 --bandwidth-mbps 0
```

Run five repetitions per cell under high-latency, high-throughput, and
bandwidth-limited loopback scenarios before interpreting scheduler behavior.
Store each scenario in a distinct directory so its JSONL contains only that
scenario. These are reproducible link settings, not provider claims:

```sh
prototype-output/zarr-env/bin/python bench/acquisition_engine.py \
  --reps 5 --assets 16 --latency-ms 8 --bandwidth-mbps 100 \
  --output prototype-output/acquisition-engine/high-latency-100mbit
prototype-output/zarr-env/bin/python bench/acquisition_engine.py \
  --reps 5 --assets 16 --latency-ms 0 --bandwidth-mbps 800 \
  --output prototype-output/acquisition-engine/fast-800mbit
prototype-output/zarr-env/bin/python bench/acquisition_engine.py \
  --reps 5 --assets 16 --latency-ms 2 --bandwidth-mbps 32 \
  --output prototype-output/acquisition-engine/slow-32mbit
```

Each output directory contains raw `runs.jsonl` and grouped `results.json`.
`--workloads` and `--readers` can limit a run to named cells/configurations,
which is useful when a binary changes and selected cells need fresh measurements.
Keep exploratory or interrupted runs in their own directories; never combine
them with accepted five-repeat matrices.
`--inject-503` makes the server return one transient 503 for each unique
request signature; use it for a reliability check separately from timing.
It records failed readers instead of removing them from raw output.

The larger streaming gate exceeds the working reservation and destination LRU.
Run it after the matrix, with no competing build or benchmark process:

```sh
prototype-output/zarr-env/bin/python bench/acquisition_bulk_stream.py \
  --reps 5 --latency-ms 0 --bandwidth-mbps 800 \
  --output prototype-output/acquisition-engine/bulk-fast
prototype-output/zarr-env/bin/python bench/acquisition_bulk_stream.py \
  --reps 5 --latency-ms 8 --bandwidth-mbps 100 \
  --output prototype-output/acquisition-engine/bulk-latency
prototype-output/zarr-env/bin/python bench/summarize_acquisition.py \
  prototype-output/acquisition-engine/accepted-final
```

This fixture is four dates of 4,096 × 4,096 UInt16 values (128 MiB), with
1,024 gzip-1 source chunks of 128 KiB decoded each. Both native and file paths
use the final binary/source selection and source-version validation. The native
consumer computes keyed chunk/coordinate checksums and retains their small
descriptors; it does not retain pixel arrays. The separate full file oracle
runs outside child timing. File outputs are deleted after verification.
Inspect every `success` field before interpreting medians. Do not append a new
invocation to an accepted directory: use a fresh output directory for reruns.

The accepted five-repeat run directories for the 2026-10-19 local matrix are:

* `prototype-output/acquisition-engine/accepted-final/high-latency-100mbit`
  (8 ms request latency, 100 Mbit/s shared bandwidth).
* `prototype-output/acquisition-engine/accepted-final/fast-800mbit`
  (0 ms request latency, 800 Mbit/s shared bandwidth).
* `prototype-output/acquisition-engine/accepted-final/slow-32mbit`
  (2 ms request latency, 32 Mbit/s shared bandwidth).

Each contains 160 raw rows: four COG workloads across baseline, current
workers-1/default and GDAL workers-1/4/8; plus two Zarr chunk workloads across
baseline, current workers-1/default and xarray HTTP-4. The current executable
SHA-256 is
`8553d9dea1d5f906553b553cf9478732237c211c6df3135eb9c0ccd6c84d42a0`; the
baseline is the exact pre-change build with SHA-256
`403c10b24aebf14eb4c507e9532e59f96800586b047eb578815e71d746bf2b52`. The
captured Rust source tree hash is
`4137fbf76200b89318589b704f3b208a4603323280d54c425a4aec1d055d8c4d`. Each
directory records source and applicable binary hashes per raw row, plus baseline
and current binary, environment, fixture, request, lockfile, and harness hashes
in its report.

The high-latency xarray TTFC entries are intentionally null. An early
`DurableStore` probe treated `red/.zarray` metadata as the first usable output;
that metric was invalidated in both raw rows and the grouped report, with the
reason recorded there. Its wall time, output duration and bytes, HTTP counts,
RSS, and exact-value checks remain valid. The probe was fixed before the fast and
slow runs to time the first synced non-metadata `red` chunk. The earlier
interrupted `candidate-v1` directory is exploratory and excluded from these
accepted results.
