# COG I/O efficiency and memory admission decision gate

Tile-Ripper eliminates the demonstrated duplicate selected COG-block traffic
without a new data cache or decoder. The several-large source-body count falls
from **75,988,992 to 56,918,016 bytes**, within **0.17%** of matched GDAL's
**56,819,712 bytes**. Source I/O is now close to that control; completion time
still needs its own comparison. The constrained default is two workers with the
existing explicit resource settings preserved. A 16-MiB working reservation is
not a process RSS ceiling.

## Physical accounting and retained implementation

The entire original **19,169,280-byte** gap is repeated physical byte coverage;
both original readers fetch the same union of source bytes. The independent
[request/block accounting](trace.md) identifies each requested compressed block,
its intervals and repetitions across all three original latency repetitions.
Of the extra bytes, 18,885,278 are repeated selected native imagery, 258,636 are
repeated unselected/overview imagery, 24,142 are exact TIFF metadata and 1,224 are
other structural/padding bytes. Opening ranges contain overview imagery, so they
cannot all be labelled metadata.

Controlled fresh-process GDAL experiments distinguish concurrent independent
handles from decoded-cache capacity: one reused handle and four serially used
handles transfer the same 8,978,432 bytes; four concurrent handles transfer
12,009,472 bytes, repeating exactly 3,031,040 bytes. All window checksums agree.
A zero decoded-cache setting leaves single-reader source bytes unchanged;
source-block window order also leaves them unchanged. Reopening every window
reduces sequential read-ahead but costs roughly four times as much wall time.
These measurements justify reader locality, rather than increasing a cache size.
The independent baseline memory ablations still transfer 75,988,992 bytes on
several-large with two workers: lowering worker count alone does not remove this
excess source traffic.

Each COG selection now admits its existing ordered read nodes together on one
worker-local GDAL dataset. It reserves the largest single output window and
uses the existing rendezvous delivery for every chunk. Windows retain their
identities, values and plan order; cancellation is checked between them. Zarr
read-node admission is unchanged. Scheduler `completed_tasks` retains the
original read-node count; `admitted_jobs` additionally reports selection batching.
A single COG selection uses one data worker, an explicit tradeoff for source
block reuse. Independent assets can run concurrently within the worker limits.

The redundant execution-time URL-cache invalidation is removed. Invalidation
remains before ETag binding/layout inspection, and execution retains the
existing pre/post source-version checks. Legacy direct planning also invalidates
before opening remote handles. Same-version headers can therefore survive into
execution. Later header evictions still cause small repeated opening ranges;
these are reported rather than hidden. The first diagnostic candidate has no
repeated selected native payload on any workload. Final exact accounting is in
[final-trace-accounting.json](final-trace-accounting.json).

Both implementations retain substantial unselected imagery from GDAL range
rounding/sequential read-ahead: original several-large controls fetch about
19.15 MB of unselected native/overview imagery. This common overhead does not
explain their original byte difference. No default range-merging, read-ahead,
TIFF-decoder or generalized persistent-cache implementation was added.

## Bounded layout inspection

Independent remote, single-band asset layouts are inspected in bounded batches,
using the existing worker/per-origin limits and caller-local GDAL settings.
Plans are assembled serially in source order. Repeated URLs within a batch and
multiband requests retain serial inspection; the latter avoids collecting many
full band layouts before the existing plan-node limit can fail. Inspection
handles remain on their own threads. Local layout-cache behavior is preserved.

The [isolated receipt](layout-comparison.json) compares preserved baseline and
layout-only binaries, with data scheduling, source versions and worker widths
held constant. Every native plan and provisional chunk matches exactly. Each
cell has three fresh-process repetitions on the 8-ms/100-Mbit/s link.

| Fixed workers | Serial layout | Parallel layout | Complete acquisition before / after | First provisional chunk before / after |
| --- | ---: | ---: | ---: | ---: |
| 2 | 0.423 s | 0.322 s | 3.129 / 3.036 s | 0.892 / 0.787 s |
| 4 | 0.418 s | 0.206 s | 2.881 / 2.664 s | 1.051 / 0.835 s |

Layout inspection improves 23.8% at two workers and 50.6% at four. The isolated
end-to-end improvements are 3.0% and 7.5%; they are smaller than the matrix's
10% win threshold. Every compared run retains **80 HEADs, 48 GETs and
26,886,144 source-body bytes**. Default-width observed RSS ranges overlap: 101.2–103.0 MiB before and
100.7–103.3 MiB after. The four-worker prototype's peak rises from 115.0 to
117.6 MiB, which remains below the resource gate but is an observed concurrency
cost.
The final candidate's RSS is measured separately. Parallel inspection is retained
for its reproducible stage and first-chunk benefit, bounded metadata, unchanged
requests/science and passing tested resource profile. Unshaped end-to-end
regressions remain visible in the final matrix; this is not a universal speedup.

## Default resource behavior and scientific contract

The default changes from up to four workers to up to **two**. Explicit
`TILE_RIPPER_WORKERS`, `TILE_RIPPER_PER_HOST`, CLI worker options, working budgets,
queue capacity, request cache limits and GDAL process cache settings remain
available. The CLI now respects explicit GDAL environment settings instead of
replacing them with its defaults. Embedded callers configure GDAL's process-wide
block cache before its first use; the library does not reset a host's initialized
cache.

The [independent memory profile](memory-components.md) separates live decoded
GDAL blocks, the compressed HTTP-byte cache, retained zlib decoder state, output
payloads, partial planner/PROJ metadata and an explicitly unassigned RSS residual.
The decoded-cache setting is not a strict peak bound: a 1-MiB setting still shows
4/8 MiB of active blocks at two/four workers. Output reservations are only
0.25/0.5 MiB in those diagnostics. Increasing the decoded cache to 64 MiB does not
reproducibly eliminate the old excess source traffic. No cache-capacity change
is justified by these measurements.

The tested constrained profile is **two workers, 32-MiB GDAL block-cache setting,
one decode thread, 16-MiB working budget, queue zero and no retained pixel
arrays**. The native consumer also checks a 2-MiB delivered-payload maximum;
these actual COG chunks are at most 128 KiB. These controls do not account for
all GDAL/runtime/caller allocations and cannot enforce an absolute RSS ceiling.

The original matched four-worker matrix retains **three RSS failures**: two
overlap-A and one overlap-B. The original assertion lost their exact RSS/wall
metrics; those values remain unavailable, with scientifically rechecked raw
outputs and failed outcomes preserved. Its fixed-two-worker follow-up passed
24/24 acquisitions with maximum RSS 154.2 MiB. The new memory diagnostics retain
explicit two/four-worker rows: candidate maxima are 127.9/132.1 MiB across eight
unshaped profiled acquisitions. They do not replace either the original failures
or the final uninstrumented matrix. All 32 instrumented baseline/candidate
observations and two allocation snapshots remain separately documented.

The public native streaming API and scientific semantics remain unchanged.
Only successful final completion commits results. Native UInt16 values,
coordinates, source windows, nodata, scaling metadata, time/variable identity,
provenance and deterministic delivery remain independently verified. Cancellation
and mutations of already-read sources must still invalidate completion. Existing
UInt16/Float32 Zarr consumer cases remain regression checks, with no format work.

## Final matched real-source matrix

All **96/96** accepted runs pass scientific verification; all **24/24 native
consumer runs** pass the measured 192-MiB RSS gate. The separate GDAL tuning
sweep retains 64 scientifically verified pilots. Every final Tile-Ripper run
has **zero repeated selected-native-block payload bytes**. Detailed ranges,
source versions, metadata/first-output stages, RSS, sink timings, request counts,
binary/source/harness hashes and every row are in [summary.json](summary.json).

Median fresh-process wall seconds follow. Rankings use the existing 10% rule;
GDAL controls are tuned separately for each workload/output/link.

| Link | Selection | Rust native | GDAL native | Result | Rust TIFFs | GDAL TIFFs | Result |
| --- | --- | ---: | ---: | --- | ---: | ---: | --- |
| Unshaped | many-small | 0.384 | 0.318 | loss | 0.398 | 0.330 | loss |
| Unshaped | several-large | 0.636 | 0.367 | loss | 0.604 | 0.439 | loss |
| Unshaped | overlap-a | 0.463 | 0.355 | loss | 0.472 | 0.378 | loss |
| Unshaped | overlap-b | 0.315 | 0.325 | tie | 0.309 | 0.339 | tie |
| 8 ms / 100 Mbit/s | many-small | 3.001 | 2.465 | loss | 2.994 | 2.466 | loss |
| 8 ms / 100 Mbit/s | several-large | 6.169 | 4.836 | loss | 6.155 | 4.860 | loss |
| 8 ms / 100 Mbit/s | overlap-a | 6.044 | 4.896 | loss | 6.081 | 5.309 | loss |
| 8 ms / 100 Mbit/s | overlap-b | 3.517 | 3.360 | tie | 3.561 | 3.367 | tie |

There are no >10% completion-time wins against the newly tuned GDAL controls;
overlap-B ties in both output pairs on both links. Relative to the previous
four-worker Tile-Ripper matrix, the shaped several-large native median improves
**8.208 → 6.169 s (24.8%)**, and TIFF completion improves **8.004 → 6.155 s
(23.1%)**. Shaped overlap-B TIFF completion improves **5.343 → 3.561 s (33.3%)**.
The original shaped overlap-A/B native RSS failures are not replaced with a
before/after speed ratio. Unshaped many-small regresses **0.284 → 0.384 s** for
native output and **0.291 → 0.398 s** for TIFFs. Shaped many-small completion is
roughly unchanged. The final default is two workers versus four previously;
these are combined-change comparisons, not isolated estimates of the COG fix.

Preparatory inspection and native first-chunk timings below use the Rust
consumer's clock, with preparation included in first chunk. RSS columns retain
both implementations. Different first-chunk clock epochs/startup costs and
checksum implementations are described in [protocol.md](protocol.md).

| Link | Selection | Rust preparation | Rust layout | Rust first provisional chunk | GDAL first chunk | Rust RSS max MiB | GDAL RSS max MiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Unshaped | many-small | 0.135 s | 0.125 s | 0.153 s | 0.099 s | 100.1 | 142.8 |
| Unshaped | several-large | 0.059 s | 0.051 s | 0.079 s | 0.080 s | 121.7 | 188.0 |
| Unshaped | overlap-a | 0.059 s | 0.052 s | 0.074 s | 0.077 s | 121.4 | 186.8 |
| Unshaped | overlap-b | 0.049 s | 0.043 s | 0.096 s | 0.115 s | 134.8 | 170.8 |
| 8 ms / 100 Mbit/s | many-small | 0.426 s | 0.322 s | 0.788 s | 1.114 s | 100.8 | 166.5 |
| 8 ms / 100 Mbit/s | several-large | 0.152 s | 0.114 s | 0.443 s | 0.837 s | 129.7 | 219.2 |
| 8 ms / 100 Mbit/s | overlap-a | 0.154 s | 0.118 s | 0.444 s | 0.595 s | 125.7 | 186.3 |
| 8 ms / 100 Mbit/s | overlap-b | 0.159 s | 0.123 s | 1.215 s | 3.076 s | 127.9 | 213.6 |

Peak native Rust RSS across both links is **134.8 MiB**; the largest Rust file
process reaches **140.6 MiB**. GDAL native/file maxima are **219.2/217.1 MiB**.
The 192-MiB gate is enforced on the Rust native consumer; it is not silently
applied as a rejection/filter to the tuned GDAL controls. A broad claim about
speed at equal RSS limits would require a separately constrained GDAL matrix.

Physical source I/O below is for native results. Counts are medians of all
repetitions, including real observed control variability. Body MB are decimal;
HEAD responses transfer zero counted body bytes, but still contribute request
latency. HTTP header/TCP wire bytes are not counted.

| Link | Selection | Rust body MB | Rust GET / HEAD | GDAL body MB | GDAL GET / HEAD | Rust repeated block-payload bytes: selected / all levels |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Unshaped | many-small | 26.558 | 38 / 64 | 26.362 | 32 / 64 | 0 / 171,344 |
| Unshaped | several-large | 56.918 | 33 / 24 | 56.820 | 30 / 24 | 0 / 85,670 |
| Unshaped | overlap-a | 56.918 | 33 / 24 | 63.144 | 38 / 24 | 0 / 85,670 |
| Unshaped | overlap-b | 38.109 | 20 / 24 | 38.044 | 18 / 24 | 0 / 57,112 |
| 8 ms / 100 Mbit/s | many-small | 26.526 | 37 / 64 | 26.362 | 32 / 64 | 0 / 142,788 |
| 8 ms / 100 Mbit/s | several-large | 56.918 | 33 / 24 | 56.820 | 30 / 24 | 0 / 85,670 |
| 8 ms / 100 Mbit/s | overlap-a | 56.918 | 33 / 24 | 56.820 | 30 / 24 | 0 / 85,670 |
| 8 ms / 100 Mbit/s | overlap-b | 38.109 | 20 / 24 | 38.044 | 18 / 24 | 0 / 57,112 |

Several-large native requests fall from the original **48 GET / 48 HEAD** to
**33 GET / 24 HEAD**. Its remaining **98,304 duplicate response bytes** are
three repeated opening ranges: **85,670 overview payload bytes, 12,070 exact
TIFF metadata bytes and 564 structural/padding bytes**. No selected native
block byte is repeated. Other workload residuals and block counts, including
control duplicates, are retained in the exact accounting. The newly tuned
unshaped GDAL control sometimes repeats payload too: several-large body counts
range **56,819,712–58,310,656 bytes**, and overlap controls vary more. Their
observed medians and complete rows are preserved; GDAL is not assumed always
to fetch a block exactly once.

| Retained native resource evidence | RSS outcome | Maximum measured RSS |
| --- | --- | ---: |
| Original four-worker matched matrix | 21/24 pass; **3 failures retained** | Exact failed-row peaks unavailable |
| Original fixed-two-worker follow-up | 24/24 pass | 154.2 MiB |
| New default-two-worker matched matrix | **24/24 pass** | **134.8 MiB** |
| New explicit-four-worker memory diagnostics, unshaped two-workload subset | 4/4 pass; instrumented | 132.1 MiB |

## Final validation and decision

[validation.json](validation.json) binds the final binaries and Rust source.
The Rust suite passes **53 tests**, with two ordinary-runner HTTP cases
intentionally ignored. Formatting and strict all-target lint pass. Seven
external-consumer checks pass, including native COG values/provenance, native
Zarr bit patterns, cancellation, catalog handling and mutation of already-read
sources. Three independent GDAL snapshot checks pass, including late mutation
and failed-output cleanup. Final native descriptors and TIFF manifests match
frozen ETags; native node IDs retain increasing plan order. Every timed output
passes the independent complete-coverage/value/grid/metadata oracle.

**Decision:** the targeted COG-traffic correction and tested default resource
gates pass. Retain selection-local reader reuse, bounded parallel inspection
and the two-worker default. The broader performance-leadership gate remains
**closed**: several-large native completion is still 73% slower than tuned GDAL
on unshaped replay and 28% slower on the shaped link, despite almost identical
source bytes. The remaining gap is not attributable to excess COG traffic
alone. Ordered backpressure, available data parallelism, startup, checksum work,
GDAL versions and other reader/runtime costs remain possible contributors;
this work does not isolate their individual costs.

The measured memory profile has headroom, with no allocator-level RSS guarantee.
Rust links GDAL 3.12.4 and rasterio links GDAL 3.10.3; the two statistics
consumers also use different checksum implementations. One workstation, one
native tile/grid, warm replay and three repetitions per cell limit generalization. Full ranges and the preserved failures constrain the
claims. No public release, deployment, merge, tag, paid resource, new format,
ChronoZarr adapter, adaptive controller or generalized persistent cache was
created. Changes stop at a validated local commit and this decision gate.
