# Native HTTP efficiency decision

The native reader now reuses preparation metadata, reserves its buffers by their
actual lifetimes, and admits more payload concurrency for bounded overlap
selections. GDAL remains the default build and reader. Native selection stays
explicit; an explicitly GDAL-free request cannot fall back to GDAL.

Work starts at `1473c4c7e454ad62deed22536c2f91253e72c8ce` on
`codex/acquisition-planner`. The [candidate manifest](candidate.json) binds the
production sources and frozen executables; its `source_commit` is the base HEAD
at build time, before the local implementation commit. No merge, release,
publication, deployment, format expansion or persistent content cache is part
of this change.

## Final decision and matched measurements

**Overlap-B is now competitive with tuned standalone GDAL in this controlled
fixture:** 3.414 s versus 3.356 s, a 1.7% difference. It is 26.1% faster than
matched baseline native, with two active payload readers inside 16 MiB. This
is a competitiveness result, not a general native speed advantage or a switch
of the default backend. Native is a practical explicit choice for callers that
fit its supported scientific profile; keep GDAL as the distribution/default
compatibility path pending a separate coverage and scientist-usage decision.

The [pre-edit baseline](baseline-benchmark.json) has 72 measured trials plus 32
standalone tuning pilots. The [final comparison](benchmark.json) has **96/96
successful trials**, with three randomized repetitions per reader, workload and
network condition. All ordered scientific chunk records and applicable memory
gates pass. Baseline native is the frozen `1473c4c` binary; the preserved
Tile-Ripper/GDAL control is `911a268`. Standalone GDAL uses the separately
tuned worker counts and 32 MiB raster cache. Rust requests use two requested
workers, per-host four, 16 MiB and rendezvous delivery. GDAL runtime/cache
memory is outside the Rust reservation; actual process RSS is measured for all.

Completion medians in seconds, including process startup:

**8 ms per request, shared 100 Mbit/s**

| Workload | Baseline native | Optimized native | Tile-Ripper/GDAL | Tuned GDAL | Native change |
| --- | ---: | ---: | ---: | ---: | ---: |
| many-small | 2.833 | 2.709 | 2.860 | 2.456 | -4.4% |
| several-large | 5.040 | 4.879 | 6.253 | 4.850 | -3.2% |
| overlap-a | 4.829 | 4.675 | 6.079 | 4.892 | -3.2% |
| overlap-b | 4.618 | 3.414 | 3.496 | 3.356 | -26.1% |

**Unshaped HTTP/1.1 loopback**

| Workload | Baseline native | Optimized native | Tile-Ripper/GDAL | Tuned GDAL | Native change |
| --- | ---: | ---: | ---: | ---: | ---: |
| many-small | 0.133 | 0.129 | 0.329 | 0.277 | -3.3% |
| several-large | 0.436 | 0.436 | 0.524 | 0.298 | -0.1% |
| overlap-a | 0.308 | 0.302 | 0.404 | 0.311 | -1.9% |
| overlap-b | 0.305 | 0.178 | 0.317 | 0.311 | -41.7% |

Several-large is unchanged unshaped and 3.2% faster than baseline under
shaping. Its shaped native ranges overlap substantially: baseline 4.757–5.529 s,
optimized 4.736–5.442 s; tuned GDAL is 4.845–4.855 s. These three-repeat
medians do not establish a several-large speed advantage. Many-small remains
10.3% slower than tuned GDAL under shaping. One initial optimized unshaped
many-small process takes 0.452 s (median 0.129 s); its roughly 0.319 s outside
acquisition startup remains in the raw results. No outlier was discarded.
Historical 4.71/3.38 s values and the separate pre-edit anchor are not mixed
with this final matched comparison.

First-chunk medians under shaping use each consumer's internal clock. Rust
includes preparation; standalone GDAL excludes Python imports. Full process
completion above remains the primary comparison:

| Workload | Baseline native | Optimized native | Tile-Ripper/GDAL | Tuned GDAL |
| --- | ---: | ---: | ---: | ---: |
| many-small | 0.730 | 0.697 | 0.733 | 1.129 |
| several-large | 0.381 | 0.344 | 0.445 | 0.835 |
| overlap-a | 0.371 | 0.349 | 0.446 | 0.600 |
| overlap-b | 0.863 | 1.182 | 1.187 | 3.051 |

Overlap-B completion improves at the cost of native first-chunk latency:
0.863 → 1.182 s (37% later), as two complete cropped reads share the link.
It still delivers earlier than tuned GDAL here. Queue/rendezvous wait is not
separately instrumented, so socket gaps and callback clocks cannot prove a
queue-wait attribution.

CPU seconds / peak RSS MiB / observed GETs and HEADs under shaping:

| Workload | Reader | CPU s | RSS MiB | GET / HEAD | Source body bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| many-small | Baseline native | 0.213 | 35.77 | 80 / 80 | 25,829,216 |
| many-small | Optimized native | 0.201 | 37.55 | 48 / 80 | 25,698,144 |
| many-small | Tile-Ripper/GDAL | 0.420 | 98.64 | 37 / 64 | 26,525,696 |
| many-small | Tuned GDAL | 0.493 | 148.03 | 32 / 64 | 26,361,856 |
| several-large | Baseline native | 0.469 | 45.28 | 48 / 30 | 37,746,907 |
| several-large | Optimized native | 0.568 | 50.97 | 36 / 30 | 37,697,755 |
| several-large | Tile-Ripper/GDAL | 0.797 | 127.72 | 33 / 24 | 56,918,016 |
| several-large | Tuned GDAL | 0.723 | 220.70 | 30 / 24 | 56,819,712 |
| overlap-a | Baseline native | 0.453 | 39.95 | 48 / 30 | 37,746,907 |
| overlap-a | Optimized native | 0.471 | 40.06 | 36 / 30 | 37,697,755 |
| overlap-a | Tile-Ripper/GDAL | 0.592 | 122.23 | 33 / 24 | 56,918,016 |
| overlap-a | Tuned GDAL | 0.761 | 182.72 | 30 / 24 | 56,819,712 |
| overlap-b | Baseline native | 0.295 | 39.72 | 48 / 30 | 37,746,907 |
| overlap-b | Optimized native | 0.305 | 36.36 | 36 / 30 | 37,697,755 |
| overlap-b | Tile-Ripper/GDAL | 0.414 | 125.48 | 20 / 24 | 38,109,184 |
| overlap-b | Tuned GDAL | 0.527 | 203.34 | 18 / 24 | 38,043,648 |

CPU and RSS do not improve uniformly. In several-large the optimized median
CPU is 21% higher and RSS is 13% higher than baseline despite improved median
completion. The full receipt retains all ranges, actual workers, unshaped CPU/RSS
and transfer decomposition; no unmeasured cause is assigned to these differences.
Python/GDAL runtime allocations contribute to the standalone RSS and CPU.

The six-source reduction at 16 MiB is **48 → 36 GETs (25%)**, consisting of
24 → 12 metadata GETs and the same 24 payload GETs; body bytes fall
**37,746,907 → 37,697,755** (49,152 fewer, 0.13%). Many-small is
**80 → 48 GETs (40%)** and **25,829,216 → 25,698,144 bytes** (131,072
fewer, 0.51%). HEAD counts remain unchanged. These are actual response-body
bytes including metadata, padding, duplicate ranges and unselected imagery;
HTTP/TLS framing is not included. The 32 MiB/two-worker strategy can group
overlap-B into **24 GETs** at the same 37,697,755 bytes. The one-page prefix
counterexample explicitly demonstrates the opposite tradeoff: 8 KiB adds
4,096 bytes with no GET reduction.

At 16 MiB, optimized peak working reservations are 12,243,465 bytes for
many-small, 10,480,825 for several-large, 12,577,977 for overlap-A and
**14,237,781 for overlap-B**, including retained native metadata. All are below
16,777,216 bytes. A lower reservation is not a whole-process RSS bound.

The [resource matrix](resource-benchmark.json) adds **48 trials** at requested
workers 1/4 and budgets 8/16/32 MiB, for overlap-B and several-large under both
network conditions. **36 admitted trials pass** science, ordered output and
working reservations; **12 expected 8 MiB admissions reject** without completion
or a tile-payload GET. Baseline rejects both workloads at 8 MiB; optimized
overlap-B now fits while optimized several-large still rejects. Budgets were
never increased to obtain a pass. Rejected-trial time, CPU/RSS, errors and
metadata traffic remain in the receipt.

Shaped resource results below are **single diagnostic repetitions**, not the
main three-repeat timing decision. Active counts are observed admitted tasks,
not the requested worker count. Unshaped results and all per-trial measurements
are retained in the same receipt.

| Workload | Requested workers | Budget MiB | Baseline s | Optimized s | Active baseline / optimized | Optimized GETs | Optimized peak reserved bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| overlap-b | 1 | 8 | reject | 4.775 | — / 1 | 36 | 7,204,025 |
| overlap-b | 1 | 16 | 4.884 | 4.809 | 1 / 1 | 24 | 10,489,309 |
| overlap-b | 1 | 32 | 5.141 | 4.749 | 1 / 1 | 24 | 10,489,309 |
| overlap-b | 4 | 8 | reject | 4.672 | — / 1 | 36 | 7,204,025 |
| overlap-b | 4 | 16 | 4.907 | 3.390 | 1 / 2 | 36 | 14,237,781 |
| overlap-b | 4 | 32 | 4.851 | 3.282 | 1 / 4 | 36 | 28,421,782 |
| several-large | 1 | 8 | reject | reject | — / — | 12 metadata only | — |
| several-large | 1 | 16 | 5.293 | 5.199 | 1 / 1 | 36 | 10,480,825 |
| several-large | 1 | 32 | 5.106 | 5.010 | 1 / 1 | 36 | 10,480,825 |
| several-large | 4 | 8 | reject | reject | — / — | 12 metadata only | — |
| several-large | 4 | 16 | 4.956 | 4.555 | 1 / 1 | 36 | 10,480,825 |
| several-large | 4 | 32 | 5.233 | 4.539 | 1 / 3 | 36 | 31,221,313 |

The 16 MiB/one-worker overlap-B trial coalesces to 24 GETs, yet takes
4.809 s; the two-worker main strategy keeps 36 GETs and finishes in 3.414 s.
This is the measured request-count/concurrency tradeoff. With 32 MiB/four
requested workers, overlap-B preserves four active readers and individual tile
requests: 36 GETs, 28,421,782 bytes reserved and 3.282 s in that diagnostic run.
The clean Linux 32 MiB/two-worker exact test independently observes 24 GETs
and the same body bytes; it is not a three-repeat macOS timing comparison.

[The saved critical-path analysis](critical-path.json) covers all 12 shaped
overlap-B trials, including the three GDAL controls. Native baseline keeps one
payload active; the optimized path holds two active for a median 2.752 s of
roughly 3.17 s with payload requests in flight. Time inside the request span
with no HTTP request active falls from 0.240 to 0.033 s. Body-interval union is
3.147 s for optimized native versus 3.049 s for tuned GDAL; the ideal shared-link
byte floors are approximately 3.016 and 3.043 s respectively. These are server
interval observations, not an instrumented link-utilization measurement.

Eliminating 12 metadata GETs removes 96 ms of injected per-request delay in
service-time sum, whereas matched wall improves by 1.204 s. Fewer GETs alone
cannot explain that improvement: concurrent payload retrieval and admission are
the supported main explanation. CPU changes 0.295 → 0.305 s, versus 0.527 s for
tuned GDAL. Worker elapsed-time sums overlap and cannot be added as wall phases;
queue wait is not separately measured. No exact causal split between transport,
decode and rendezvous waiting is claimed.

Consumer-internal acquisition is 3.404 s for native and 3.212 s for tuned GDAL
(6% slower). Full-process competitiveness also reflects roughly 0.010 s outside
the native clock versus 0.144 s for the Python/GDAL control. The wall comparison
does not establish equal reader-internal latency.

[Capacity observations](resource-summary.json) retain every admitted row. Main
optimized transport capacity peaks are 1,519,139–1,699,550 bytes per reader;
adapter delivery peaks are 18,432–1,310,720 bytes. Coalesced resource cases reach
4,935,133 encoded bytes per reader, with the corresponding reservation charged.
Across both matrices optimized native RSS peaks at 55.4 MiB, below the separate
192 MiB consumer gate. The largest optimized working reservation is 31,221,313
bytes in a 32 MiB request. Upstream codec allocations remain conservatively
reserved rather than claimed as directly observed.


## Changes and alternatives tested

Preparation retains only immutable TIFF parser pages, bound to asset ID,
locator, strong ETag and size. Each `Prepared` owns its own snapshot; another
preparation or source version cannot replace it. Capacity is capped at the
smaller of half the working budget and 8 MiB, trimmed to preserve mandatory
task headroom, and included in scheduler admission. Execution reparses the
TIFF using these pages. This removes duplicate metadata GETs without sharing a
mutable TIFF reader or weakening execution HEADs and conditional range reads.

The [4 KiB/8 KiB prefix experiment](prefix-comparison.json) tests all four frozen
workloads under both network conditions. The 8 KiB prefix reduces requests on
these fixtures but does not improve the six-source completion times. The
[one-page counterexample](prefix-onepage.json), with three repetitions per
condition, transfers an extra 4,096 bytes at the same GET count with an 8 KiB
prefix. The production default stays 4 KiB; there is no provider-name heuristic
or claim that fewer GETs always means faster acquisition. Prefix experiment
binaries predate the final buffering strategy and are explicitly hash-bound.

For ordinary planned windows, a simulated LRU reference sequence chooses the
smallest decoded cache that preserves the old cache's tile-miss count. It uses
actual planned windows, including mask-skipped nodes. COG caches belong to one
selection task and are dropped before releasing that task's reservation. The
previous per-worker lifetime could reserve unused caches and prevent additional
payload work. Existing Zarr coordinate retention remains unchanged.

A selection can instead retain at most 2 MiB of cropped UInt16 samples with no
decoded tile cache. This is allowed only when there is more than one output
node, the first planned node already needs every selection tile, and the union
of planned tiles equals that complete selection. No additional imagery tile is
fetched to accelerate a later chunk. Unmasked stored samples are sliced into the
same ordered chunks; the existing source-global polygon masks are applied to
each delivered chunk. Larger selections and other tile-reference patterns use
the ordinary window strategy.

Coalescing joins only selected compressed ranges separated by at most eight
COG wrapper bytes, within the existing 8 MiB encoded-range ceiling. Members must
be unique consecutive misses; ordinary window reads additionally require them
to be first needed in the same planned read. The merged buffer and each returned
member copy are reserved. The selection strategy preserves the maximum admitted
payload concurrency before spending spare capacity on coalescing. A larger
declared budget can therefore enable range grouping; a smaller budget retains
individual tile requests. There is no unaccounted encoded cache.

HTTP bodies now allocate exactly the requested length and probe EOF with a
one-byte stack buffer. This avoids the previous `read_to_end(len+1)` capacity
growth. Atomic client initialization ensures parallel inspection shares the
same initial connection pool. Selection-affine readers already reused handles;
they remain synchronous and bounded rather than adding an unrestricted request
queue. Source identities, retries, timeouts and final validation are retained.

The early coalescing candidate reduced the six-source fixture to 24 GETs but
retained one payload reader and did not improve completion time. It was not
accepted as the final 16 MiB strategy. Profiling supported changing admission
and selection buffering instead. The original server already used
`TCP_NODELAY`; these results do not attribute the regression to delayed ACKs.

## Allocation and completion contract

Reservations use capacities, checked arithmetic and the maximum of distinct
allocation phases. During selection decode this includes the raw cropped
selection, decoded tile capacity, encoded copies, codec working space and any
retained merged range. Raw-to-UInt16 conversion then requires two selection
buffers. Delivery retains the typed selection plus one chunk and mask allowance.
The scheduler reserves the maximum phase before admitting the task.

The block bound includes the greater of two decoded tile capacities plus an
encoded buffer, or **three encoded buffers**, plus 128 KiB codec working space.
The three-copy case covers an upstream malformed-wrapper fallback, not just
ordinary valid COGs. The adapter may retain a merged encoded range in addition.
The decoded bound also covers the upstream over-limit probe's possible capacity
growth. Conservative capacities for non-power-of-two tiles are included.

`scheduler.native_buffers` reports observed transport and adapter capacities.
It does not claim to instrument every upstream allocation; upstream codec and
conversion capacities are covered by the conservative reservation. Reservation
is not RSS. Process/runtime allocations, parsed layout/plan/geometry objects,
Zarr prepared metadata, optional GDAL cache and caller-retained chunks have
separate scopes. Retained native parser-page capacities are included. The
independent consumer also retains its separate 192 MiB process-RSS gate.

Cancellation remains cooperative, including while a bounded HTTP request is in
flight. Chunks stay provisional until final source checks and successful
`finish()`. Errors, source mutation, cancellation and sink failure never produce
a successful completion receipt. Per-reader successful-range counters exclude
failed retry attempts and partial failed bodies; benchmark-server GET/HEAD and
body-byte observations are the authority for actual transfer totals.

## Scientific and fault acceptance

[Independent validation](validation.json) repeats the inherited 95 geometry
checks, 755,798 exact polygon-mask samples, 32,391,168 exact frozen-source
samples, 61 native profile/transport/fault checks, seven COG/Zarr public-consumer
cases and eight STAC workflow cases. Geometry, mask conventions, STAC ordering,
calibration, native dtype/nodata and the independent scientific oracles are
unchanged. The snapshot test independently prepares different versions using
the same layout cache, executes the newer preparation twice, and rejects the
older version before delivery.

The **44 [additional selection checks](selection-faults.json)** cover individual and
coalesced payload reads, exact GDAL references, transient and exhausted 503/short
responses, wrong ranges/ETags/412, sink failure, cancellation and late mutation.
They exercise buffered selection across all 20 inherited combinations of
uncompressed/LZW/Deflate, predictor 1/2, classic/BigTIFF and both byte orders.
Direct comparison adds 3,391,488 bit-exact samples across these profiles, both
HTTP buffer strategies and the malformed-wrapper case, against independently
written GDAL reference bytes.
A padded compressed block with a readable mismatched wrapper forces the
three-copy fallback: 1,600,000 bytes rejects before a tile-range GET; 4 MiB
completes with exact original decoded values. This is a direct check of the
corrected error-path admission.

[Feature verification](verification.json) passes 56 default, 74 dual-feature,
39 native-only and 24 Zarr-only tests, native/dual Clippy with warnings denied,
formatting and the separate fixture-dependent convenience-API mutation test.
Ordinary Cargo ignored-fixture counts are retained; snapshot and convenience
fixtures are run separately rather than counted as ordinary automatic tests.

[Live acquisition](live.json) independently repeats Earth Search discovery and
the two-band Sentinel-2 check: eight chunks, 20,566 exact samples, pinned source
versions and GDAL full-source masks. Its public-provider timing is not a
controlled optimization comparison.

[Clean deployment evidence](deployment.json) runs actual live acquisition and
bit-exact frozen HTTP overlap-B at both 16 and 32 MiB in Python-slim without
geospatial Python libraries, GDAL, PROJ or libtiff. Each frozen run checks
3,538,944 samples. [Platform evidence](platform.json) records a clean Rust 1.92
source install, separate empty container-local build targets, image digests,
dependency tree and Linux/macOS linkage. Both tested platforms are ARM64;
other architectures, Windows, musl and WASM remain unverified. Compilation uses
about 1.5 GiB peak RSS and requires C/C++ tools, CMake and pkg-config; this is
separate from runtime admission.

## Native-first use and remaining coverage

The [CLI and Rust guide](cli.md) makes the no-GDAL build, polygon/date/band
request, public streaming API and supported profile discoverable. `--backend`
overrides the inherited COG backend setting and rejects unavailable readers
before source I/O. `--gdal-free` rejects a GDAL-enabled build and selects native
in a native-only executable. Direct GDAL-free planning defaults to callback
unless output is explicitly requested; streaming always uses a callback sink.
Native-only GeoTIFF fetch errors name the stream/API alternative.

The Rust API exposes `cog::Backend`, `cog::backend`, `cog::require_gdal_free` and
`acquisition::prepare_gdal_free`. The guard proves Tile-Ripper's `gdal` feature is
disabled; a host application must separately inspect its own linkage. Library
backend environment settings must remain fixed throughout preparations and
streams. A dual-feature native reader still uses GDAL geometry and links GDAL.

The raster and geographic profile has not expanded. Native remains a narrow
single-band lossless UInt16 tiled TIFF/BigTIFF reader with the established
north-up/native-grid and WGS84-to-nearby-UTM polygon restrictions. GDAL supplies
optional GeoTIFF writing and compatible source/geometry semantics outside that
profile. Neither API promises general raster reprojection or every operation
GDAL can perform.

Further format coverage requires explicit storage-domain sample, nodata,
calibration, coordinate/axis and source-identity contracts; bounded decode and
transport admission; independent pixel/geometry oracles; and equivalent fault,
mutation, cancellation and platform gates. No HDF5, NetCDF, Zarr v3, ChronoZarr
adapter, persistent cache or general reprojection support was added. Broad
default selection also needs a separate coverage/platform and independent
scientist-usage decision. This work keeps GDAL default while making native an
explicit supported workflow.

The [protocol](protocol.md) records benchmark controls and reproduction. The
dgov file-claim plan was structurally compiled; this checkout has no configured
dispatch provider. Session collaborators executed and reviewed the work without
automatic dispatch, merge or publication.
