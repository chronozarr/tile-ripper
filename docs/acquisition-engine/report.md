# Acquisition engine decision gate

Raw `.gz` run archives linked below are retained locally and excluded from Git.
The benchmark scripts and compact JSON summaries are versioned for reproduction.

Development on `codex/acquisition-planner` implements the shared scheduler and
native stream. The measured gains justify retaining them. Private-alpha
distribution remains on hold: the evidence does not establish consistent
superiority over tuned established readers. Nothing was merged, tagged or
published, and ChronoZarr was not changed.

## What changed

The baseline is committed `94c901493f8f6ec972728b1417e6557192095b1a`, rebuilt
from its exact archived source and lockfile. The new scheduler admits COG read
windows and Zarr source chunks through one bounded queue, with reusable
worker-owned native readers, shared HTTP connection pooling, parallel Zarr
identity checks, manual global/per-origin limits, deterministic output and
cooperative cancellation. The Rust iterator delivers native UInt16/Float32
values, coordinates, observation identity, missingness and provenance without
intermediate files. File adapters use the same execution path.

An initial whole-asset scheduling candidate did not improve large COG reads.
That measurement led to per-window scheduling; its rejected timing is kept
separate from the accepted final matrix. See the [pipeline profile](architecture.md)
and [native API and runnable demonstrations](streaming.md).

## Measured completion time

All 480 accepted matrix runs passed; each cell has five repetitions and no
failures. Entries below are old → new median seconds. Small differences are
not asserted as speedups; the machine-readable summary also retains bootstrap
intervals and applies a 10% difference rule.

| Selection | 8 ms / 100 Mbit/s | 0 ms / 800 Mbit/s | 2 ms / 32 Mbit/s |
| --- | ---: | ---: | ---: |
| 16 small COG intersections | 1.494 → 1.032 | 0.230 → 0.211 | 1.585 → 1.235 |
| Four large COG intersections | 3.968 → 2.286 | 0.633 → 0.383 | 8.304 → 5.635 |
| Overlapping request A | 0.889 → 0.626 | 0.194 → 0.176 | 0.877 → 0.697 |
| Overlapping request B | 0.866 → 0.600 | 0.189 → 0.177 | 0.872 → 0.699 |
| 256 Zarr source chunks | 6.999 → 2.926 | 1.582 → 1.570 | 3.379 → 2.005 |
| 1,024 Zarr source chunks | 27.253 → 10.950 | 5.625 → 5.663 | 11.933 → 7.754 |

The higher-latency Zarr reduction is 2.39–2.49×; large COG windows improve
1.47–1.74× across these conditions. Fast-link small-chunk Zarr has no clear
old/new improvement. Reducing serial network dependencies helps when requests
are expensive; durable small-object output and startup remain material when
the link is fast. Overlapping requests have no data-cache advantage: each
process rereads its selection.

Against the best GDAL setting from the 1/4/8 sweep, the new path loses on small
COG intersections and latency-sensitive overlap requests. Large COG reads show
no clear difference on the fast/high-latency links, and improve on GDAL on the
32 Mbit/s link (5.635 versus 6.379 s). Against four-connection xarray, 256-chunk
fast-link completion improves (1.570 versus 2.162 s); the 1,024-chunk
high-latency case loses (10.950 versus 9.634 s). Other Zarr comparisons do not
clear the stated difference/interval rule. These are scenario observations,
with the source-safety and dependency differences below.

On the 32 Mbit/s large-COG case, current response-body traffic implies a weak
5.370 s transfer floor against 5.635 s observed completion. Selected encoded
payload alone implies 4.719 s. Tuned GDAL's own body-traffic floor is 6.226 s
against 6.379 s completion: it is already near its shaped-link limit. The new
window access pattern transfers about 21.5 MB versus GDAL's 24.9 MB there.
Reducing transferred data is more promising than increasing concurrency for
this case. These are body-byte bounds, not full wire-byte accounting or a
universal optimum. The [complete summary](benchmarks.json) and
[480 raw accepted runs](accepted-runs.jsonl.gz) retain every cell and interval.

## Native streaming gate

A separate four-date, 4,096 × 4,096 UInt16 cube delivers 128 MiB through 1,024
native 128-KiB chunks, with a 16-MiB working reservation and queue capacity one.
Five interleaved paired repetitions per condition compare current file output
with the standalone Rust consumer. Every source chunk is read exactly once;
both paths transfer the same 134,345,183 response-body bytes. The file oracle
checks every value and coordinate; streaming checks each chunk's stored-value
bits and coordinate bits keyed by time/window, full observation coverage,
dtype, native transform, count and sum. The consumer includes checksum work.

| Link | File median | Stream median | First native chunk | File / stream maximum RSS |
| --- | ---: | ---: | ---: | ---: |
| 0 ms / 800 Mbit/s | 5.750 s | 1.625 s | 0.093 s | 103.9 / 78.3 MiB |
| 8 ms / 100 Mbit/s | 15.599 s | 16.816 s | 2.803 s | 103.3 / 80.4 MiB |

All 20 runs pass. Streaming removes intermediate files and improves fast-link
completion 3.54× in this test. It is about 8% slower on the higher-latency link;
avoiding output does not ensure a speed advantage when network dependencies,
ordered delivery and consumer work dominate. The fast-link body-transfer floor
is 1.343 s, excluding metadata latency, decode and consumer work. Measured RSS
stays below the 128-MiB delivered cube size for both paths, but remains much
larger than the working reservation. This establishes bounded incremental
delivery for the tested shape, not a universal RSS guarantee.

See [stream summary and hashes](bulk-streaming.json),
[raw paired runs](bulk-runs.json.gz), and the reproducible
[`acquisition_bulk_stream.py`](../../bench/acquisition_bulk_stream.py).
Local [UInt16 COG](demonstration.json) and
[Float32/NaN, two-date Zarr](float-demonstration.json) demonstrations also pass
with the final executable and explicit CLI completion records.

## Correctness and fault gate

The [final receipt](final/final-receipt.json) binds the final CLI/consumer and
recursive Rust source hashes to 83 passing HTTP checks, eight STAC checks,
two-store read/cache assertions, and 12 local oracle gates covering 149,520
values. All 49 Rust tests pass; two HTTP-driven tests are ignored by the ordinary
Rust runner and are covered by the separate harnesses. Formatting and strict
all-target lint pass. New stream tests cover mixed COG read sizes, exact native
Zarr delivery, deterministic order, slow consumers, queue bounds, cancellation,
drop, terminal source mutation and rerunning the same prepared execution.

Fault checks include 503/429 retry and exhaustion, interrupted-response recovery,
ETag mutation, missing/appearing/corrupt/truncated/oversize chunks, decoded-size
and decompression-bomb rejection, malformed metadata, coordinate/time/dtype/codec
validation, empty selections, failed-output cleanup and snapshot isolation.
Resumption means a fresh complete rerun after cancellation; persistent resume is
not implemented or claimed. The ordinary test suite also retains an
IMERG-sized native source-chunk case under its original 16-MiB working budget.

## Scope and interpretation

The benchmark uses synthetic deterministic COG and Zarr fixtures over a shaped
loopback server. Six workload selections cover 16 small COG intersections, four
large intersections, two overlapping requests and 256/1,024 Zarr source chunks.
The COG routes mirror identical source bytes and are Sentinel-like layouts,
not actual Sentinel observations. These results do not model scene diversity,
public-provider throttling or a geographically remote network.

Five fresh-process repetitions per cell interleave the archived baseline,
new one/four-worker execution, GDAL at one/four/eight workers and four-connection
xarray as applicable. Every timed output is independently checked. GDAL and
xarray are reader-only controls: they omit Tile-Ripper's source pinning and
post-validation. Python rasterio uses GDAL 3.10.3; Rust uses GDAL 3.12.4.
These differences limit causal claims between tools. The matched old/new
Tile-Ripper comparison retains source-version safety and output semantics.

The [protocol](../acquisition-benchmark-protocol.md), benchmark scripts, raw
accepted rows and summary retain bytes, requests, retries, RSS, stage times and
timing scopes. Worker read/decode timers are fused and overlap; they are not
isolated decoder measurements or additive wall-time components. High-latency
xarray first-chunk measurements were invalidated after discovering a metadata
trigger; their completion times and correctness results remain valid.

## Resource limits and unresolved costs

Defaults use up to four available CPUs/workers and four explicit operations per
origin. Shape/payload admission reduces concurrency to fit the working-buffer
reservation; users can override both limits. The gate tests static defaults
across three link conditions and a serial-worker control. It does not implement
or directly compare a feedback controller. The evidence supports retaining
simple defaults rather than introducing unmeasured adaptive complexity.

Working reservations, metadata cache (another B/2), Zarr output buffering
(another B/4) and stream queue capacity have separate bounds. GDAL caches,
decoder/runtime allocations, prepared plans and consumer retention are outside
them. The requested buffer is not a process RSS ceiling. COG internal HTTP
subrequests are opaque to the task cap. Source/node ordering can delay later
assets; it is not a fair multi-origin work-stealing scheduler.

COG discovery/identity inspection remains serial. Repeated overlapping requests
reread source data; there is no persistent data cache, range-coalescing layer or
partial checkpoint protocol. Retry delays occupy their worker while other
admitted workers can progress. Cancellation joins in-flight native calls and
can wait through configured timeouts. Successful stream completion requires
final source validation; earlier chunks remain provisional.

## Next interoperability milestone

Stabilize the native consumer contract around successful completion, coordinates,
missingness and bounded backpressure, then build a small independently tested
consumer integration. A later ChronoZarr adapter can use that contract; none is
implemented here. Before distribution or broader speed claims, test real
multi-scene mirrors and provider traces, address serial COG metadata and Zarr
destination-write costs, and repeat the gate on another machine. Persistent
resume/cache policy should be a separate explicit design decision.
