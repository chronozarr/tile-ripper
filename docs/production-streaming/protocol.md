# Real multi-scene validation protocol

This gate uses actual Copernicus Sentinel-2 collection-1 L2A B04/B08 COGs on
native 10-metre grids. It does not convert synthetic fixtures into a public
Sentinel performance claim. Dates are selected deterministically from the
repository's existing frozen 17SPV/2025 study list. The first eight dates supply
16 distinct source objects. Large/overlapping selections use the first three
dates (six objects), retaining independent images on the same spatial tile.
This tests date/band diversity, not adjacent UTM zones or cross-grid mosaicking.

## Frozen sources and bounded capture

`bench/real_multiscene.py capture` performs a bounded STAC search for the frozen
item IDs, follows at most one terminal next page, reads the selected item
metadata, and HEAD-pins each requested COG's strong ETag and logical size. The
public STAC lookup/search times and response sizes are separate from replay.
Both readers use the same frozen item/asset manifest. This is frozen-ID catalog
resolution, not a benchmark of an unrestricted regional STAC search.

The benchmark-only mirror fetches selected original COG byte ranges with
`If-Match`, verifies response ETag, Content-Range, length and SHA-256, and writes
them at their original offsets in sparse files. No COG is reencoded. One
upstream request at a time, at most 256 successful range requests and 192 MiB
of source response bodies bound capture. The final receipt lists every range.
Metadata traffic is additional and bounded by the frozen 8-item/16-asset list.
An upstream error aborts capture; no provider retry loop or paid service is used.

Every replay first verifies all captured response hashes. The local HTTP server
returns the original logical size and ETag and fails closed on any uncaptured
range; a sparse hole can never silently supply an output pixel. Original
compression, tiling, offsets, source values and native metadata remain intact.
Source bytes, item payloads, raw timing rows, complete chunk summaries and sparse
files remain under ignored `prototype-output/production-streaming/real/`.
The versioned source manifest and compact gate report contain no imagery.
This fixture is not a new Tile-Ripper cache feature.

## Workloads

All spatial selections are native EPSG:32617 bounds, without resampling or
calibration. Source tile origin is `(600000, 4000020)` with `(10, -10)` spacing;
independent metadata checks validate the actual grids. Output chunks are at most
256 × 256 UInt16 samples. The source is 10,980 × 10,980; a large AOI is a bounded
2,048 × 2,048 intersection, not a whole-scene download.

| Selection | Source pixel window `(column,row,width,height)` | Assets | Native values |
| --- | --- | ---: | ---: |
| many-small | `(7300,2100,96,96)` | 16 / eight dates / two bands | 147,456 |
| several-large | `(6144,1024,2048,2048)` | six / three dates / two bands | 25,165,824 |
| overlap-a | `(6868,1748,768,768)` | six | 3,538,944 |
| overlap-b | `(7124,2004,768,768)` | six | 3,538,944 |

A and B overlap by 512 × 512 pixels. Every repetition is a new process and
rereads source data. The pair does not claim persistent overlap reuse. Three
accepted repetitions per reader/selection/link are interleaved with a fixed
seed. Warm filesystem page cache is retained; the workstation is not a
controlled, dedicated performance host.

## Equivalent work and tuning

Two pairs are compared separately:

* The external Rust native-statistics consumer versus rasterio/GDAL statistics.
  Both compute per-window valid counts, uncalibrated sums and checksums over the
  same native samples, retain coordinates/source identity, and produce chunk
  descriptors. Rust emits each descriptor incrementally through a rendezvous
  stream. The Python timing control retains small summary descriptors; it does
  not retain pixel arrays. It is a reader timing control, not a second
  implementation of the transactional streaming API.
* Tile-Ripper file acquisition versus threaded rasterio/GDAL acquisition. Both
  write one native uncompressed GeoTIFF per 256-pixel window, preserving dtype,
  nodata, transform, CRS, scales, offsets and time/asset identity. File outputs
  are independently verified then deleted. No file result is compared directly
  with a stream result as an equivalent-work speed ratio.

A separate one-run pilot sweeps GDAL asset concurrency 1, 2, 4 and 8 for each
selection and each output pair. The fastest setting is fixed for the three
accepted repetitions; pilots are retained separately. This is a finite tuning
sweep, not a claim of globally optimal GDAL settings. GDAL uses a 32-MiB block
cache, one native decode thread, directory probing disabled, 32-KiB initial
header ingestion, bounded HTTP retries/timeouts and serial multirange handling.
The same transport settings are supplied to Rust. Rust's scheduler defaults to
four workers/per-origin operations, subject to its 16-MiB working reservation.

Python rasterio and Rust link different GDAL versions; record both and both Cargo
lockfile hashes. Python import/startup costs are included in process wall time.
Rust additionally pins source identities and validates them before and after
execution. GDAL's reader control consumes the immutable mirror without those
extra checks. Preserve request/byte counts so this asymmetry stays visible.
No claim is made about live public-provider throughput from local replay.

## Timings, memory and correctness

Retain each fresh process's wall time, wait4 peak RSS, first native chunk time,
preparation time, file-write worker durations, final completion, transport
HEAD/GET/range counts and response-body bytes. First-chunk time is a provisional
usable buffer, not a committed complete acquisition. Rust measures it from
consumer entry including preparation; Python measures from worker entry after
imports. Process wall time includes both startups. First closed file and first
native chunk are different metrics.

Opt-in stage traces separate COG version-check batches from native layout
inspection and the planner. `planning_compute_seconds` is native planner wall
time less the sequential timed layout inspections; it includes selection,
geometry and node construction. `prepare_seconds` also includes source binding,
identity checks and inspection/provenance construction. Identity batch traces
occur in preparation, prevalidation and postvalidation. File materialization
seconds are summed sink worker durations; overlapping durations cannot be added
to reconstruct process wall time. Fused read/decode timers are not pure download
or pure decoder costs. HTTP counts are source response bodies, not wire bytes.
Discovery times are a common separate addend, not credited as a speedup.

Every timed result passes an independent local rasterio/NumPy oracle: exact
native sample bits/checksums, counts/sums, full nonoverlapping per-asset coverage,
CRS/affine, source windows, nodata, scale/offset and date/variable identities.
The synthetic external-consumer gate additionally uses xarray on UInt16 and
Float32 Zarr with sparse fill, noncanonical NaN payloads, signed zero and swapped/
reversed axes; it exercises cancellation and mutation after provisional delivery.
Oracle work and source capture are excluded from acquisition timing. Failures
are retained; accepted summaries state failure counts.

For the native consumer, enforce a measured 192-MiB process RSS gate, including
GDAL/runtime, with queue capacity zero, a maximum delivered payload of 2 MiB,
and no retained pixel arrays. The 16-MiB scheduler budget is not an RSS limit:
GDAL blocks/decoder scratch, source plans/reports and runtime allocations are
additional. Report file-path and GDAL-control RSS separately. Do not substitute
reserved-buffer counters for observed RSS.

## Reproduction

Use the repository's existing isolated `prototype-output/zarr-env` with NumPy,
rasterio, requests, xarray and Zarr; versions are in the compact receipt.

```sh
cargo build --release --locked --bins --examples
CARGO_TARGET_DIR=target cargo build --release --locked --manifest-path consumers/native-stats/Cargo.toml
prototype-output/zarr-env/bin/python bench/consumer_validation.py
prototype-output/zarr-env/bin/python bench/real_multiscene.py capture
prototype-output/zarr-env/bin/python bench/real_multiscene.py benchmark --reps 3 --label accepted-fast
prototype-output/zarr-env/bin/python bench/real_multiscene.py benchmark --reps 3 --label accepted-latency --latency-ms 8 --bandwidth-mbps 100
```

Output labels must be fresh; the harness refuses to append into an existing run
directory. Preserve the frozen source manifest/index, original response hashes,
binary/source/lockfile/harness hashes and raw rows locally. A future reproduction
against a changed public version must start a different mirror and manifest.
The optional `--binary-dir` runs the preserved serial binaries with their bound
source-hash receipt for the focused before/after comparison. Rebuild the serial
commit in a separate target directory if those local binaries are unavailable.

Compare medians and full observed ranges. Treat differences smaller than 10%
as ties for the reader ranking; three repetitions are scenario evidence rather
than a statistical or universal superiority claim. Evaluate a narrowly timed
metadata optimization on its stage reduction and its actual end-to-end effect,
including any remaining loss to GDAL. Stop at the decision gate; no release,
merge, tag, paid infrastructure, new scientific format or ChronoZarr adapter.

## Matched source-validation control

The final decision uses `gdal_snapshot_worker.py` through
`snapshot_comparison.py`. This additional GDAL workflow pins the same ETags,
checks every object initially, checks again after each layout inspection before
reading samples, attaches the correct per-asset `If-Match` header to GDAL reads,
and validates all source versions/sizes before its final result or file manifest.
It removes a failed new output directory. An explicit late-mutation test checks
that an already-read source cannot silently produce a completed control result.
Its metadata HEADs have a separate cap of four; data asset concurrency is tuned
at 1/2/4/8. Native grids, pixel work and source-safety work are therefore matched,
while implementation-specific metadata/open counts remain measured differences.
The reader-only controls are retained separately as diagnostics.

```sh
prototype-output/zarr-env/bin/python bench/snapshot_comparison.py --label matched-fast --reps 3
prototype-output/zarr-env/bin/python bench/snapshot_comparison.py --label matched-latency --reps 3 --latency-ms 8 --bandwidth-mbps 100
```

Both comparisons include their consumer work: Rust's scalar sample checksum and
Python's native zlib checksum implementations differ. These are end-to-end
acquisition-plus-consumer measurements, not isolated decoder throughput.
Differences in versions, startup, ordered delivery and checksum implementation
limit attribution. Byte/request differences are directly measured; attributing a
wall-time loss to a particular internal cache or decoder needs a separate profile.

For a fresh reproduction of this exact source version, first seed an empty
`prototype-output/production-streaming/real/` with the committed `sources.json`.
Then run capture. Do not overwrite a different existing mirror's manifest.
`If-Match` makes a changed public object fail instead of silently substituting
new imagery. The manifest's discovery timings describe the original bounded
catalog resolution; seeded replay does not rerun or remeasure that search.

## RSS failures and fixed memory follow-up

The matched default-worker latency matrix retains three RSS misses. The initial
assertion-only gate discarded their exact process RSS/wall metrics after testing
the limit; their raw failures and completed scientific outputs remain preserved.
The harness now records resource failures with their measured metrics.

`bench/stream_memory_gate.py` rechecks the default overlap-A case diagnostically
and runs all four selections on both links, three repetitions each, with the
fixed `TILE_RIPPER_WORKERS=2` recipe. These memory follow-ups do not replace the
original performance rows or imply a new adaptive policy. The 192-MiB gate is a
measured test criterion; the library's working reservation is not an RSS sandbox.

The final harness also corrects two reporting details: preserved binaries bind
to their saved source-hash receipt, and uninstrumented serial planner time is
reported as unavailable rather than zero. These changes affect receipt/metric
reporting, not source selection, checksum/statistics work or scheduling. Original
raw reports and pre-correction harness snapshots remain local with their hashes.
