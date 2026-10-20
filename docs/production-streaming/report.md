# Production-useful streaming decision gate

The native Rust interface now supports a separate, incrementally consuming
application with explicit provisional results and validated completion. A late
remote COG mutation uncovered during this work is now rejected by final version
checks. The architecture is suitable for a later scoped ChronoZarr adapter
prototype within the admitted source profiles. This work does not establish a
public Sentinel throughput advantage or authorize a release.

## Committed work

| Commit | Validated change |
| --- | --- |
| `03d5f81` | Preserved the completed shared acquisition scheduler and native stream. |
| `e30a011` | Preserved its benchmark harnesses, protocols and compact correctness/performance reports; raw generated run archives remain local. |
| `1b141a4` | Added pre/post remote COG validation, safe cancellation/completion behavior, the public contract and a separate path-dependent Rust statistics consumer. |
| `cadb623` | Bounded COG version-check concurrency and added opt-in identity/layout/planner stage measurements. |

The inherited engine passed 49 Rust tests, 83 HTTP correctness/fault checks,
eight STAC checks, the local scientific oracle, multi-store checks and the
performance regression smoke before the first commits. The current Rust source
passes 52 tests; two ordinary-runner HTTP tests are intentionally ignored and
covered by the explicit harnesses. Formatting and strict all-target lint pass.
The previous synthetic performance matrix remains available as its own evidence;
its speed ratios are not relabelled as real Sentinel results.

## Public streaming contract and independent consumer

The [documented contract](../acquisition-engine/streaming.md#public-consumer-contract)
defines chunk identity, deterministic ordering, 2D row-major dimensions, native
dtypes, source windows, exact coordinates/native CRS, time/variable identity,
missingness, provenance, queueing, cancellation and source mutation. A Zarr read
node can emit several planes/windows; `node_id` alone is not a chunk key.

Receiving a chunk does not commit a dataset. Only successful `finish()` returns
the final receipt. An observed terminal error remains an error from `finish()`;
cancellation also invalidates completion after iterator EOF. Dropping joins the
producer after closing the channel. `finish()` drains unread chunks, so a
statistics consumer must process each item and compare its counts with the
report. Consumer side effects require the consumer's own staging policy.

[`consumers/native-stats`](../../consumers/native-stats/README.md) is a separate
Cargo package using the public API, with its own locked dependency graph. It
requests several assets, computes valid-pixel counts and native-value sums,
streams coordinate/provenance/checksum summaries, retains one payload and O(1)
aggregate state, and never writes an intermediate raster. Only its last
`complete` record authorizes committing the result; errors emit `aborted`.

Independent rasterio/NumPy and xarray checks exercise two remote UInt16 COGs,
multiple remote UInt16 Zarr stores with sparse fill, and remote Float32 Zarr
with reversed/swapped axes, noncanonical NaN payload bits and negative zero.
Tests verify native value checksums, counts/sums, coverage, native coordinates,
scaling metadata, time indices and independently decoded datetimes. Cancellation,
mutation after delivery (including an already-read COG), catalog streaming and
final validation all pass. The independent GDAL snapshot control also has its
own successful-read, late-mutation and failed-output-cleanup checks.

Sonnet review/design invocations were attempted as requested, but both returned
an expired OAuth-token error before reading the repository. No Sonnet correctness
review is claimed. The external consumers and scientific oracles provide
independent implementations; this remains a single-agent code review.

## Real-source scope

The [frozen manifest](sources.json) identifies 16 original public Sentinel-2 C1
L2A objects: B04/B08 on eight acquisition dates for tile 17SPV. The small-window
selection intersects all 16 objects; the large and overlapping selections use
six objects on three dates. This is real band/date diversity on one native UTM
grid, not adjacent-tile or cross-CRS validation.

The benchmark captured 82,313,216 source bytes (78.5 MiB) in 60 conditional range
requests, below its 192-MiB/256-range-request limits. Sparse mirrors preserve the
original COG offsets, compression, tiling, ETags, logical sizes and source bytes.
Every replay checks captured response hashes and rejects uncaptured ranges.
No whole-scene imagery, raw timing data or credentials are committed.

The bounded frozen-ID STAC search took 0.529 s over two requests (the second
confirmed the terminal page); eight item lookups totaled 1.532 s and initial
16-object identity HEADs totaled 2.074 s. These original capture observations
are shared discovery/manifest costs, separate from repeated acquisition timing.
They do not measure unrestricted STAC search or prove a discovery speedup.

The [protocol](protocol.md) gives native windows, tuning, settings and exact
reproduction commands. Source capture and oracle work are excluded from timed
acquisition. Warm local replay and a shaped loopback link do not measure public
provider throughput, throttling or internet reliability. Python and Rust use
different GDAL versions, and the statistics consumers include different checksum
implementations. Record those limits with every comparison.

## Matched real-scene completion

Three interleaved repetitions per cell complete the scientific work correctly. Three default native runs exceed the separate RSS gate; their scientific outputs were independently rechecked and their failures are retained. Both GDAL controls in this table perform pinned conditional reads and initial/pre-read/final version checks. Times are median fresh-process wall seconds; all observed ranges, stage times, requests, bytes, output-write durations, RSS, tuning pilots and failures are in [the compact summary](summary.json).

| Link / selection | Rust native | GDAL native | Native result | Rust TIFFs | GDAL TIFFs | File result |
| --- | ---: | ---: | --- | ---: | ---: | --- |
| Unshaped loopback: many-small | 0.284 | 0.326 | win | 0.291 | 0.334 | win |
| Unshaped loopback: several-large | 0.674 | 0.364 | loss | 0.654 | 0.469 | loss |
| Unshaped loopback: overlap-a | 0.457 | 0.381 | loss | 0.471 | 0.396 | loss |
| Unshaped loopback: overlap-b | 0.498 | 0.389 | loss | 0.495 | 0.385 | loss |
| 8 ms / 100 Mbit/s: many-small | 2.953 | 2.510 | loss | 2.958 | 2.502 | loss |
| 8 ms / 100 Mbit/s: several-large | 8.208 | 4.885 | loss | 8.004 | 4.906 | loss |
| 8 ms / 100 Mbit/s: overlap-a | RSS failure (2/3) | 4.881 | RSS gate failed | 8.260 | 4.881 | loss |
| 8 ms / 100 Mbit/s: overlap-b | RSS failure (1/3) | 3.357 | RSS gate failed | 5.343 | 3.370 | loss |

The win/tie/loss labels use a 10% elapsed-time rule. Three repetitions on one workstation are scenario evidence. Small-request wins can include startup/consumer effects; large-window losses are not isolated decoder benchmarks. The reader-only controls are retained separately, without silently comparing their lighter validation work to the final matched workflow.

## Demonstrated metadata optimization

On the same 16-asset, 8-ms/100-Mbit/s selection, bounded version checks retain every validation and preserve identical native plans and delivered chunk records across all six before/after acquisitions. The focused comparison uses the preserved serial build and the final build, with the same frozen imagery versions and request/byte counts.

| Metric | Serial | Bounded batches | Reduction |
| --- | ---: | ---: | ---: |
| Complete native acquisition | 3.450 s | 2.906 s | 15.8% |
| Preparation | 0.693 s | 0.478 s | 31.1% |
| First provisional native chunk | 1.419 s | 1.046 s | 26.3% |

Every compared native acquisition still sends **80 HEADs and 48 GETs**, transferring **26,886,144 response-body bytes**. Final preparation still spends about **0.423 s** in serial layout inspection versus **0.0034 s** in planner computation. The measured improvement justifies bounded version checks; it does not make the data scheduler optimal.

For the RSS-passing native rows in unshaped loopback, Rust peak RSS reaches **175.9 MiB**; the corresponding GDAL control reaches 179.1 MiB. The latency matrix additionally retains three native RSS misses above 192 MiB. Runtime/cache/decoder allocations remain outside the 16-MiB working reservation.
On several-large in that condition, native Rust transfers 75.99 MB versus GDAL's 56.82 MB. This directly demonstrates excess source traffic for this access pattern; source-block/cache reuse is a justified profiling target, not a proven universal cause of every timing difference.

For the RSS-passing native rows in 8-ms/100-Mbit/s replay, Rust peak RSS reaches **190.8 MiB**; the corresponding GDAL control reaches 216.0 MiB. The latency matrix additionally retains three native RSS misses above 192 MiB. Runtime/cache/decoder allocations remain outside the 16-MiB working reservation.
On several-large in that condition, native Rust transfers 75.99 MB versus GDAL's 56.82 MB. This directly demonstrates excess source traffic for this access pattern; source-block/cache reuse is a justified profiling target, not a proven universal cause of every timing difference.

The final matched matrix contains **96 runs: 93 pass both gates and three retain RSS failures**, plus 64 separate concurrency pilots. The reader-control/focused before-after matrices retain another 108 accepted runs; these are not substituted for the matched comparison. Scientific correctness passes; the default four-worker profile does not reliably satisfy the 192-MiB RSS gate. [Validation receipt](validation.json).

## Fixed memory recipe

The default four-worker profile has three RSS misses (two overlap-A, one overlap-B). They are not dropped or relabelled as scientific errors. The original gate retained the >192-MiB failure but lost exact process RSS/wall metrics at its assertion; the harness now retains those fields on resource failures. The raw completed chunk outputs were rechecked independently.

A focused follow-up uses the **same binary**, **TILE_RIPPER_WORKERS=2**, a 32-MiB GDAL cache, rendezvous delivery and the same 16-MiB working budget. All **24 acquisitions** (four selections, both links, three repetitions) pass science and RSS checks. Maximum measured native-process RSS is **154.2 MiB**. This is a fixed documented recipe, not an adaptive scheduler or a hard allocator-level RSS guarantee. Follow-up timings and every raw RSS observation are retained in the compact summary; they are not substituted into the original default-worker performance ranking.


## Memory and remaining bottlenecks

The native consumer uses a rendezvous queue, a 2-MiB delivered-payload check and
no retained pixel arrays. Real COG chunks contain at most 128 KiB of native
values. Scheduler reservation peaks remain within the request's 16-MiB working
budget. That budget is not an RSS ceiling: metadata/plans/reports, GDAL cache,
source-block decoding and runtime allocations are additional. The separate
native-consumer process RSS gate is 192 MiB. The default four-worker profile misses it in three runs; a fixed two-worker recipe passes
all 24 follow-up acquisitions. This does not guarantee a hard RSS limit for
arbitrary plans or caller retention. File and GDAL-control RSS are
reported separately in the machine-readable summary.

The remaining concrete costs justify profiling source-block reuse and COG range
traffic, and investigating serial GDAL layout inspection with version-safe,
deterministic plans. End-to-end timings also include consumer checksums, ordered
delivery, native open/decode work and runtime startup. They do not isolate a
single decoder or prove which internal cache caused every excess request.
No speculative scheduler, production persistent cache or adaptive controller
was added. Data scheduling and scientific reader semantics remain unchanged.

## Decision

The functional streaming primitive gate passes for the supported UInt16 COG and
UInt16/Float32 Zarr profiles, with an explicit two-worker recipe for the tested
192-MiB native-consumer budget: public consumption, coordinates/provenance,
backpressure, cancellation, late-failure invalidation and final completion are
concrete and tested. A later adapter can stage frame/data work until `finish()`
succeeds, preserve per-source grids/identities and choose an explicit policy for
missing source chunks. That adapter is not implemented here.

The default-worker 192-MiB resource gate and the broader distribution/performance
leadership gate remain closed. Real-source results must be kept distinct from the earlier synthetic scheduler gains. Broader
scene/grid coverage, another machine and targeted COG I/O/decoder profiling are
needed before general performance claims. Nothing was merged, tagged, published
or integrated with ChronoZarr. No new scientific format, HDF5, Zarr v3,
distributed scheduler, paid infrastructure or production persistent cache was added.
