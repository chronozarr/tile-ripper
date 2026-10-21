# Cost-aware remote Zarr v3 shard ranges

The implemented default reduces dense sharded completion from **1.6672 to
0.7452 seconds (2.24×)** on the matched 8 ms / 100 Mbit/s mirror. Warm-local HTTP
completion falls from **0.0598 to 0.0428 seconds (1.40×)**. The preserved fe6fcd1
executable is the control. This is an acquisition improvement with unchanged
scientific values, source-version checks and working-memory limits.

Tuned Python remains faster on the dense shaped case: **0.6696 seconds** versus
0.7452 seconds. Tile-Ripper wins the wide-gap shaped case: **0.7154 versus 1.4710
seconds**. Sparse reads retain precise traffic; small timing regressions remain
in some cells. Arbitrary raw-payload damage under a dishonest unchanged ETag is
an explicitly retained integrity limitation, demonstrated in both binaries.

## 1. Implemented strategy

[The planner](../../src/zarr_ranges.rs) sorts already validated physical spans
within each object. It retains logical member identity independently of byte
order. The cost rule compares the extra gap transfer time with one avoided
request's latency divided by configured concurrency. Hard limits additionally
bound gap, merged span, relative overfetch and admitted buffer memory.

The conservative default assumes 2 ms latency and 32 decimal MB/s, permits a
64 KiB gap, caps a merged range at 4 MiB, and permits at most 100% extra bytes
per group. [Configuration and execution details](workflow.md) document manual
hints and all four policies. No online controller or new dependency was added.

The scheduler considers at most 32 consecutive original logical read nodes.
It merges by physical shard offset, then delivers the original node/chunk order.
If a window's remaining serial requests would exceed its original parallel
request waves, the cost policy splits it. Budget pressure similarly splits
windows or preserves individual reads. No whole-shard fallback is introduced.

zarrs **0.23.14** still interprets indices and decodes inner chunks. Its public
`subchunk_byte_range` confirms each member against the defensive validated plan.
The existing MAX/MAX missing-entry guard remains. This changes transport and
buffer ownership, not the shard parser or Zarr codecs.

`Prepared::explain().sources[].zarr_v3` exposes logical members, physical order,
offsets/lengths, gaps, missing entries, unique/repeated byte coverage, pinned
identity/ETag and candidate merged bytes. Candidate plans precede scheduler
window/budget limits; the execution receipt records the actual batches. Detailed
physical plans are limited to sharded arrays after the experiments identified
avoidable ordinary-array diagnostic overhead.

## 2. Requests and bytes

Dense close-packed fixture, identical across both links and all repetitions:

| Quantity | fe6fcd1 | Cost default | Tuned Python |
| --- | ---: | ---: | ---: |
| HEAD | 108 | 108 | 117 |
| Total GET | 411 | 53 | 55 |
| Payload GET | 382 | 24 | 24 |
| Selected encoded bytes | 172,799 | 172,799 | 172,799 |
| Payload body bytes | 172,799 | 310,026 | 310,026 |
| Extra payload bytes | 0 | 137,227 | 137,227 |
| Index GET / bytes | 24 / 12,384 | 24 / 12,384 | 24 / 12,384 |
| All response-body bytes | 196,252 | 333,479 | 333,479 |

The tradeoff is **79.4% more payload bytes for 358 fewer payload requests**.
Indices are still fetched once per selected shard. The optimized dense receipt
records 382 prepared-index lookups and 358 merged-buffer hits. Index hits mean
logical lookups served by the prepared version-bound index; they include library
cache hydration from the retained index bytes, not extra HTTP downloads.
Selected coverage is unique; overlapping source references are rejected.

The wide fixture keeps **94 payload requests and 2,048,000 payload bytes** under
the default. Fixed Python thresholds use six requests but transfer **13,582,336
bytes**, including 11,534,336 bytes of overfetch. No measured sharded cell issues
a whole-object GET. Counts are actual HTTP/body counts, excluding headers/TLS;
Python ordinary-array counts include legitimate missing-chunk 404 probes.

A separate [64-inner-chunk interleaving audit](physical-coverage.json) exposes
an additional window tradeoff: 130,304 unique selected raw bytes become 256,512
payload bytes in 12 requests, with **126,208 repeated physical bytes** across
logical execution windows. Each selected logical chunk is decoded once; the
merged ranges from different windows can overlap. This remains within the
100% extra-byte cap, is fully charged, and matches independent values and the
old chunk delivery order. The receipt includes each pinned shard's selected
physical plan and actual ranges/unique/repeated coverage. No cross-task data
cache is added to conceal this cost.

## 3. End-to-end comparison

Medians of three rotated repetitions. Times include preparation, source checks,
selection, decoding, native samples/masks/coordinates and successful completion.
They exclude Python imports from the acquisition clock, matching the prior best
control; full child-process CPU and RSS include startup. Native first output is
a chunk; Python first output is a variable, so those boundaries differ.

| Layout / selection | Link | Previous s | Cost s | Python s |
| --- | --- | ---: | ---: | ---: |
| ordinary / sparse | local-http | 0.0136 | 0.0139 | 0.0224 |
| ordinary / sparse | 8ms-100Mbps | 0.2228 | 0.2397 | 0.2748 |
| ordinary / dense | local-http | 0.1198 | 0.1312 | 0.2211 |
| ordinary / dense | 8ms-100Mbps | 5.7167 | 5.7081 | 6.0167 |
| sharded / sparse | local-http | 0.0140 | 0.0137 | 0.0238 |
| sharded / sparse | 8ms-100Mbps | 0.2769 | 0.2801 | 0.3168 |
| sharded / dense | local-http | 0.0598 | 0.0428 | 0.0803 |
| sharded / dense | 8ms-100Mbps | 1.6672 | 0.7452 | 0.6696 |
| wide / sparse | local-http | 0.0364 | 0.0361 | 0.0456 |
| wide / sparse | 8ms-100Mbps | 0.3135 | 0.3233 | 0.3626 |
| wide / dense | local-http | 0.0871 | 0.0867 | 0.0739 |
| wide / dense | 8ms-100Mbps | 0.7225 | 0.7154 | 1.4710 |

[All 108 final cells](benchmark.json), [medians and ranges](summary.json), and raw
per-cell results/HTTP traces under `prototype-output/zarr-coalescing/benchmark/`
retain the measurements. There were no failed cells in the final comparison.
Three repetitions provide observed ranges, not a claim of statistical certainty.
The comparison ran without builds or validation jobs in parallel.

The original ordinary/sharded fixtures were copied and hashed before editing.
The second sharded layout has larger raw inner chunks, 128 KiB gaps and reversed
physical offsets; each reader receives the same scientific selection within
that layout. The reference uses current Zarr-Python **3.4.1**, xarray **2026.9.0**,
NumPy **2.3.4**, four HTTP connections/workers, conditional source validation and
library-owned 1 MiB gap / 16 MiB range coalescing.

[180 policy cells](policy-pilot.json) compare individual, adjacent, fixed, cost
and Python on both sharded layouts, sparse/dense selections and three links.
Adjacent-only merging saves no requests on this frozen close-packed selection:
selected and unselected time chunks interleave physically. Logical proximity
therefore does not establish physical adjacency.

[24 hint cells](hints.json) isolate the static model on the wide dense fixture:

| Link | Default cost s / GET | Hinted cost s / GET | Fixed s / GET | Python s / GET |
| --- | ---: | ---: | ---: | ---: |
| 8 ms / 100 Mbit/s | 0.7158 / 94 | 0.7098 / 94 | 1.7686 / 6 | 1.4883 / 6 |
| 40 ms / 1 Gbit/s | 2.3164 / 94 | 1.4556 / 6 | 1.4713 / 6 | 1.3643 / 6 |

Both hinted cases permit a 1 MiB gap and an extra/selected-byte ratio of eight;
only their expected latency/throughput differ (8 ms / 12.5 MB/s versus 40 ms /
125 MB/s). Span remains capped at 4 MiB. The model rejects the slow-link merges
and accepts the fast-link merges. Simple static hints are sufficient for these
conditions; an adaptive controller is not justified by this evidence.

The policy/hint receipts precede the final ordinary diagnostic optimization and
non-v3 configuration isolation guard; their v3 sharded read-plan/transport path
is identical. They retain their own binary hashes.
Exploratory one-repetition policy results and the earlier complete comparison
are retained separately. An interrupted comparison stopped on an erroneous
harness assertion requiring If-Match on missing-object 404s; the corrected check
applies to present objects. Its raw result and traffic remain retained, and the
interrupted pass is excluded from final conclusions.

## 4. Sparse and ordinary qualifications

All sparse cases preserve six precise payload GETs and zero overfetch. In the
three-repetition main comparison, ordinary sparse on the shaped link rises from
0.22276 to 0.23971 seconds (17.0 ms), sharded sparse from 0.27690 to 0.28015
seconds (3.3 ms), and wide sparse from 0.31347 to 0.32327 seconds (9.8 ms).

A separate [nine-pair sparse follow-up](sparse-followup.json) rotates old/new
order on the same frozen sources and links. It does not reproduce those larger
shifts. Median **paired** differences (new minus old) are:

| Sparse layout | Local delta ms | 8 ms / 100 Mbit/s delta ms |
| --- | ---: | ---: |
| Ordinary | +0.087 | +0.960 |
| Sharded | +0.511 | −1.923 |
| Wide | −0.102 | −1.333 |

Every paired difference range includes both signs. The local sharded sub-ms
cost remains visible; a zero-timing-regression guarantee is not established.
The precise-I/O gate passes, and there is no repeatable large shaped sparse
regression in these additional pairs. All native sparse main medians remain
faster than the tuned Python control.

Ordinary dense local completion rises by **11.4 ms (9.5%)** in the main run;
its ranges overlap (old 0.1183–0.1345, new 0.1161–0.1345). Shaped completion is
8.6 ms faster. Additional range/accounting work and timing variation remain;
ordinary scientific behavior is preserved without claiming identical timings.

## 5. Memory, concurrency and timing

Merged response bodies use immutable `Bytes` slices. A task retains and charges
its entire merged-body set while delivering ordered chunks and waiting for the
consumer. Success, errors, cancellation and caught worker panics clear bodies
and views before releasing its reservation. Buffers are private to a task/read
execution; shared prepared indices retain the original pinned object versions.

The task reserves its original largest per-member working charge plus twice
all merged body lengths, with one-byte read-bound allowance per body. Immutable
batch descriptions have an additional execution-lifetime reservation. Original
native/decoder/output reservations remain. Fair-share limits, smaller windows
and individual reads prevent silently raising the supplied budget. Planning
uses configured worker/per-origin limits capped by the original task count;
actual admitted workers and GET concurrency can be lower under backpressure.

For the dense close fixture, reservation peak rises **318,265 → 860,985 bytes**.
Maximum retained merged bodies in one worker are 74,980 bytes. Shaped diagnostic
consumer RSS is **39.72 → 39.78 MiB**, versus Python 121.95 MiB; payload concurrency
never exceeds four. First native chunk is **0.3961 → 0.3995 seconds**: faster
completion does not imply faster initial output. Whole-process CPU is **0.21 →
0.14 seconds**, versus Python 0.57 seconds including imports. Native aggregated
read/decode wall time is **4.5277 → 0.3564 seconds**, and per-run decoder/adapter
residual after retrieval is **0.00854 → 0.00673 seconds**. Aggregated worker wall
intervals are not pure codec CPU; Python's pure decoder time is not isolated.

[Remote resource runs](remote-memory.json) use a non-retaining consumer, four
workers and 64× more logical output (0.5 → 32 MiB). At an **8 MiB** working budget,
reserved peak remains at most **5,847,172 bytes**, with measured RSS about
17.8 → 70.0 MiB. At **4 MiB**, the larger plan skips batching rather than raising
the budget; RSS is about 80.6 MiB. These runs pass the 192 MiB RSS and <64 MiB
growth gates. The original [16 resource cells](science/memory.json) pass too.

A **192 KiB** request succeeds with individual reads and a 96,424-byte reservation;
**128 KiB** fails during retained metadata admission. The request budget covers
the declared working scope, not total RSS: process/runtime, HTTP machinery,
plan/explain/receipt bookkeeping and caller retention differ. Metadata growth
remains visible in the large resource runs.

## 6. Scientific, failure and public validation

[Scientific receipts](science/) repeat 40 codec/axis cells, three fill/key/shape
cells, nine filter/index/admission cases, two all-NaN bit cases and 27 inherited
HTTP/index/corruption/mutation/cancellation faults. [Six full benchmark science
runs](benchmark-science.json) compare every emitted native byte, mask and selected
coordinate/time field against independent Zarr-Python, outside timing runs.

[24 added cases](extended.json) cover dense/sparse members, multiple variables
and shards, mixed encoded sizes, noncontiguous timestamps, reversed physical
order, empty/all-missing indices, absent shards, present noncanonical NaN bits,
index start/end and CRC/no-CRC variants, tiny budgets, prepared reuse, damaged
and truncated merged bodies, conditional conflicts, exhausted retries, mutation
between index/payload and cancellation while a merged GET is in flight. Gzip
body damage with unchanged headers/ETag fails codec validation; exhausted,
mutated or cancelled operations return no successful finish receipt.

The separate [integrity qualification](integrity-qualification.json) deliberately
returns damaged **raw** bytes with the original strong ETag and correct framing.
Both fe6fcd1 and the optimized binary can finish successfully while the independent
oracle detects changed values. Correct framing and ETags alone do not detect unchecked raw damage. Codec
checksums detect corresponding damage; they are not authentication against a
malicious server. This
is an existing source-integrity assumption, not fixed by coalescing. No universal
raw-corruption rejection guarantee is claimed.

[Feature checks](rust.json) pass: default **74**, native **52**, Zarr-only **37**,
all features **92**, with existing ignores unchanged. Strict all-target clippy
passes in all four configurations. Final post-diagnostic-edit v3 integration
checks (five cases), strict all-feature clippy, and inherited COG consumer checks
pass on the final source. COG/v2 are also tested with an intentionally invalid
Zarr coalescing setting: the setting is ignored outside prepared v3 sources.

[Inherited receipts](inherited/) retain remote/CF v2 tests, 61 native COG science/
fault checks, 32,391,168 independently compared real samples, seven external
consumer tests, four scientific-output cases plus eight inherited cases, and
31 publication-fault cases. Output defaults and publication implementation are
unchanged.

The [restrained public GFS regression](public.json) preserves the frozen selected
object identities, exact eight native sample bit patterns, masks, coordinates,
two 2023 timestamps and the declared 6,371,229 m spherical CRS. It still reads
one **3,985,993-byte** payload and a **1,028-byte** index from a **141,648,404-byte**
shard, with no whole-shard GET. Measured RSS is about **56.9 MiB**. Optimization
uses mirrors; the public test makes no provider speed claim. This public receipt
precedes ordinary-array diagnostic optimization and the non-v3 configuration
guard; neither changes its sharded path.

## 7. Default decision

Keep cost coalescing as the remote-sharded default for the explicitly selected
v3 profile. It produces measured dense completion gains, bounds overfetch and
memory, and retains precise sparse traffic. It is a conservative policy for
unknown links, not a universal optimum or a claim of superiority over Python.
Known high-latency/fast providers can use explicit hints and wider overfetch
limits; the larger transfer/reservation is then visible in the receipt.

The sparse timing uncertainty, ordinary local cost and raw-integrity assumption remain
qualifications. An online controller, more permissive automatic whole-shard
reads or weaker version checks are not warranted.

## 8. Profile and durability boundary

The [existing admitted profile](../zarr-v3-input/workflow.md) remains: rank-three,
time-first UInt16/Float32 data; regular explicit spatial coordinates and native
CF CRS; supported little-endian bytes/gzip/Zstd/Blosc pipelines; regular ordinary
or singly indexed shards with supported start/end indices. Curvilinear grids,
other data types/ranks/calendars, sharded coordinates, external masks, outer or
nested sharding codecs, reprojection and arbitrary storage transformers remain
outside admission. Strong per-object identities do not make an atomic dataset
snapshot. Experimental v3 output remains disabled by default; uncompressed v2
output and GDAL policy remain unchanged.

The [validated durable COG scientific sink](../durable-output/report.md) still
synchronizes each original writer descriptor, finalizes metadata/manifest after
successful stream completion, synchronizes staging directories/ancestors,
checks a macOS pre-publication device barrier, performs an atomic no-replace
sibling rename, synchronizes the parent and checks a post-publication barrier.
Its preserved large-case audit observed **941 F_FULLFSYNC calls becoming 952
host fsync calls plus two checked F_FULLFSYNC calls**. Those are syscall
observations, not measured hardware flushes. Application-visible fault tests
support its publication contract; filesystem/device crash consistency depends
on the OS/storage honoring synchronization. No power-cut certification is made.
Post-rename synchronization failures retain complete published output and report
unconfirmed durability. This assignment removes no synchronization.

The native Zarr group subset materializer retains its own established completion
and error-cleanup behavior; it does not gain the COG sink's atomic publication
protocol from this transport change. Streaming output stays provisional until
successful `finish()`.

## 9. Next engineering decision

The range bottleneck is understood and a bounded implementation is justified;
stop this assignment here. Keep the v3 release boundary and investigate remaining
ordinary/ordered-delivery overhead separately if those measured costs matter.
Before another format implementation, settle whether source ETags/codec checks
are sufficient or checksum-bearing payload pipelines are a release requirement.
The next format milestone should be a scoped NetCDF-4 acquisition design over
its restricted HDF5 representation, with explicit scientific admission, remote
chunk/index access and independent correctness/resource controls. This is narrower
than arbitrary HDF5: the [official format specification](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html) distinguishes that subset. No such input support was added here.

Implementation commit: `daa79e3` on `codex/acquisition-planner`.

Only local commits are authorized. No merge, push, tag, release, publication or
deployment is part of this work. [Verification/source hashes](verification.json)
and [baseline preservation](baseline.json) bind the retained evidence.
