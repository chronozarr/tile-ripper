# Native grid inspection reuse: decision and measurements

Geographic SWOT job generation now uses **one immutable source inspection per
granule**. Across the three qualified products, the matched HTTPS trial reduces
metadata/body traffic from **244 to 104 wire requests** and from **2,408,825 to
1,021,395 source bytes**. With 25 ms added to every wire request, mean generation
time falls from **8.791 to 4.664 seconds**
(47.0% less). The complete separate-process generation → plan →
fetch workflow falls from **16.589 to 12.412 seconds** under that same delay.

This is a local, unmerged, unreleased change on `codex/acquisition-planner`.
Native HDF5 and authenticated Earthdata remain opt-in and qualified only for the
existing documented SWOT profiles. GDAL remains the default.

## What repeated and what remains fresh

The old discovery bridge opened a one-pixel WSE preparation, dropped it, opened
a full-scene WSE preparation to retrieve the axes, dropped it, then ran a normal
selected five-variable job plan. Every preparation built a fresh exact-object
authentication session, canonical source binding, HDF5 parser, root/attribute
interpretation, coordinate buffers and byte cache. The full-scene preparation
also constructed WSE jobs that were never executed. Within each old plan, a full
legacy index traversal repeated for each requested chunk.

| Three-source generation operation | `afe41c2` | Reuse |
|---|---:|---:|
| Source inspections / base metadata parser opens | 9 | 3 |
| Full scientific-index traversals | 34 | 15 |
| Coordinate index traversals / compressed coordinate reads | 18 | 6 |
| Coordinate-only decoder parser opens | 18 | 6 |
| Science payload execution / science decoder opens | 0 | 0 |
| Login requests | 9 | 3 |
| Canonical HEADs | 27 | 15 |
| Wire HEADs, including redirect destinations | 54 | 30 |
| All wire requests | 244 | 104 |
| Source response bodies | 2,408,825 B | 1,021,395 B |

The original traversal/decoder counts are derived from its unchanged planner
loops and the inspected shapes/chunks: one probe plus four full-scene WSE chunks
per source, then 5/5/9 selected science chunks; two one-chunk axes per preparation.
The optimized counters are recorded directly. Base parser opens are distinct
from the closed parser views required to decode a coordinate or science chunk.

The optimized inspector retains actual source axes, parsed dataset definitions,
root/time/mapping interpretation and full parsed science indexes. Root/time and
mapping interpretation are shared within the source. The selected plan hits all
five scientific profiles, all needed indexes and all ten per-layer coordinate
lookups; it does not reopen a source or retrieve metadata for another logical
window. Parsed metadata and raw byte ranges remain private to that exact source
binding. Source version, access policy and parser-policy boundaries are checked.

Fresh canonical checks remain at object binding, completed inspection, reuse
admission, completed preparation, stream completion and the job's final sweep.
Conditional payload reads still enforce ETag, size and exact range semantics.
A checkpoint still requires a fresh online GET proof before retained bytes can
be used. Head-resource and GET-resource routes remain separate. In particular,
this implementation deliberately retains more HEADs during geographic generation
than a single ordinary selected plan (15 versus 9 across three sources).

## Before/after and process boundaries

[Full matched receipts](benchmarks.json) and [compact metrics](summary.json) retain
each phase's HTTP counts, redirect/login activity, source bytes, parser/cache
counters, timings, Rust allocations, RSS, working reservations and binary hashes.
The [receipt index](receipt-index.json) retains full diagnostic hashes/local
paths; repeated attributes and large value lists are compacted in versioned
evidence. The before/after generation adapter uses native operations over private HTTPS
fixtures because production CMR provenance must keep its authoritative NASA
locator. Production locator validation was not relaxed for fixtures.

Two repetitions per implementation run at 0 and 25 ms added delay. The final
matched timing run starts after other verification processes finish. The
allocator wrapper is identical in both generation adapters. The baseline CLI
is the original committed implementation; its generation adapter exposes only
the old resolver's visibility and leaves its body unchanged.

| Phase | Baseline | Optimized | Requests / source bytes |
|---|---:|---:|---|
| Generation, no added delay, two-run mean | 1.544 s | 1.461 s | 244 / 2,408,825 B → 104 / 1,021,395 B |
| Generation, 25 ms/wire request, two-run mean | 8.791 s | 4.664 s | same counts |
| Independent normal plan, 25 ms | 2.889 s | 2.843 s | 92 / 1,021,395 B for either |
| Independent normal fetch, 25 ms | 4.909 s | 4.905 s | 156 / 7,426,555 B for either |
| Sum: generation + independent plan + fetch | 16.589 s | 12.412 s | no work shifted into plan/fetch |

With no added network delay the geographic CPU work dominates: approximately
1.321 s before and 1.332 s after. That algorithm is unchanged. The small local
elapsed difference is not a broad throughput claim. The controlled delayed
trial and the exact request/byte reductions establish the metadata improvement;
provider latency changes are not treated as implementation gains.

Cross-process reuse uses the **existing** version-bound checkpoint, without a
new persistent cache or serialized parser objects:

| Optimized checkpoint phase, no added delay | Wire requests | Source bytes |
|---|---:|---:|
| Plan warms verified metadata/coordinate ranges | 95 | 1,021,419 |
| Separate fetch uses those ranges, reads science, writes output | 97 | 6,405,184 |
| Separate complete reconstruction | 78 | 48 |

The last row still performs online authorization and fresh final validation;
its output is reconstructed in full. The source version/size, interpretation,
request/policy identity and local range digests are validated before reuse.
Changed selections, quality policies, coordinate declarations, access policies,
ETags, sizes, truncated/corrupt ranges and interruption failures remain covered
by the checkpoint suite. Separate plan/fetch processes still reparse retained
source bytes. That is intentional and avoids persisting internal parser state.

## Memory and allocations

The retained metadata/index/coordinate information increases the per-source
prepared reservation from 2,969,284–3,067,848 B to
3,492,896–3,628,972 B, about 0.50–0.54 MiB extra. It remains charged inside the
64 MiB source request. The job admission remains 101,257,216 B under its
134,217,728 B aggregate limit, including the existing writer and bookkeeping.

In the zero-delay generation measurements, peak Rust allocations are
4,931,693 → 6,055,965 B, cumulative allocations average
81,109,538 → 75,452,783 B, and sampled process peak RSS is
25,722,880 → 27,361,280 B. These are different measures: Rust allocator accounting
excludes native/TLS allocations and allocator overhead; RSS includes the process
runtime; working reservations conservatively budget admitted buffers. Complete
axes are shared immutable `Arc<[f64]>` buffers, not duplicated at callbacks.
No full science array or AOI mask is retained by the inspector.

## Scientific, security and platform gates

The three qualified native windows, grid/mapping/coordinate fingerprints and
AOI masks exactly match `afe41c2`. All five stored science values and independent
source masks remain bit-exact against separate guarded h5py and NetCDF-C readers.
Source-global inverse PROJ/GEOS comparisons cover all 108 geometry cases,
including holes, concavity, boundary ties/tolerance, reversed axes, UTM zones,
partial/empty selections and chunk/tile independence. The boundary discrepancies
against GDAL's rasterizer remain documented separately; no pruning approximation
was introduced.

Real output retains the same AOI-accepted WSE counts: 1,094, 2,533 and 33 for the
held-out, original and additional products. The positive-center/zero-accepted-WSE
case still completes; a zero-center selection emits no executable manifest.
Empty geographic selections inspect grid/scientific metadata but construct no
selected science plan and read no science payloads. Source missingness, WSE
quality and geographic inclusion remain independent.

New runtime tests cover four concurrent preparations of differing selections,
re-entrant grid callbacks, unavailable coordinate coverage, out-of-source
windows, incompatible/insufficient budgets, changed local source versions,
mutation between inspection/preparation/execution, actual changed coordinates
under a new HTTPS ETag at the same locator, and sticky old-version conflicts.
The final source sweep and atomic writer continue rejecting later-source and
previous-source failures after earlier observations are staged.

The regression receipts include 38 authentication cases, 32 checkpoint cases,
19 multi-observation fault/recovery cases, 64 independent HDF5 science cells,
33 HDF5 fault cases, 61 native COG profile/fault cases, native COG exact-value
and consumer gates, CF-Zarr/Zarr v2 regressions, 40 Zarr v3 scientific cases and
27 Zarr v3 faults. All 16 feature combinations and clean Linux ARM64 native
builds pass; Linux links no GDAL, HDF5, NetCDF or PROJ shared library. The final
Rust/test/lint counts and log hashes are in [verification](verification.json).

An initial parallel CMR fixture run had two transient failures. Its full serial
reruns passed; the original failure log is retained. Some inherited adversarial
HTTP fixtures log expected connection resets; the Python 3.10 HDF5 fixture also
has a pre-existing `sys.exception()` logging incompatibility. All 33 asserted
HDF5 failure outcomes passed. No production behavior was changed for those logs.

## Bounded live archive acceptance

The ordinary production `discover-job` operation passed for the original three
catalog-selected, strongly pinned sources. Its native inspection took
44.566 s, 104 wire requests and 1,021,395 source bytes. The original
historical 89.33 s result came from a different provider run and is not a matched
speed ratio. Fresh authenticated reconstruction from a copied existing checkpoint
completed in 36.862 s using 78 wire requests and 48 source bytes, with all
three observations independently verified and their final versions validated.

The total, **1,021,443 source bytes and 182 wire requests**, is below the declared
2 MiB / 240-request bounds. There were no new CMR searches or science captures.
A receipt-summary nesting error occurred after successful generation; the saved
manifest/review were reused without repeating source inspection. Therefore the
live generation's outer CPU/RSS measurements are unavailable; its native wall
time and HTTP counters are retained. Producer binary hashes and the final frozen
binary results are explicit in the receipts. The final callback/early-refusal
checks do not change the authenticated archive protocol or scientific decoding.

## Cross-format decision and next milestone

No new cross-format trait or generalized GIS model was added. The useful grid
contract is intentionally descriptive: source-version authority, actual CRS,
coordinate model, dimension ordering/shape/direction, scientific interpretation
and explicit capabilities. A description cannot authorize execution.

| Format | Existing representation and version authority | Reuse decision |
|---|---|---|
| SWOT/HDF5 | Explicit stored 1-D centers; actual CF/WKT authority; one ETag/size (or local stat identity); dataset/chunk-index metadata | Implemented immutable `swot::GridInspection`, with separate per-request plans |
| Native COG | Admitted affine grid in `Layout`; PixelIsArea/GeoTIFF interpretation; PixelIsPoint explicitly unsupported; existing bounded native snapshots and single-file identity | Already reuses a native snapshot; duplicating it in a new grid container would not remove inspection |
| CF-Zarr | Explicit spatial coordinates, admitted affine-center consistency, dimension/time conventions; per-object versions across metadata/axes/data keys | Existing bounded prepared views remain the authority; one file ETag cannot represent a group snapshot |

The current CF-Zarr profile requires its explicit coordinates to agree with an
admitted affine grid. This does not imply that arbitrary explicit coordinates
are affine. Nonuniform coordinates and unsupported CRSs remain capabilities
that a future descriptive contract must distinguish. SWOT boundary semantics
also remain distinct from the existing COG rasterizer. Shared extraction becomes
useful when a concrete second-format consumer needs the same operation; it is
not justified merely by common shape/CRS field names.

Remaining costs are source-global geometry CPU, archive signing/login/redirect
latency, fresh canonical validation and independent-process plan reconstruction.
The next general-engine milestone should audit the version-validation lifecycle
in shared object transport, starting with redundant canonical size/HEAD checks
inside object binding. Any reduction must retain exact-method authorization,
conditional reads, checkpoint proof and final publication validation, and be
measured across single-file native readers and multi-object Zarr bindings.

The [workflow/API](workflow.md), [brief](brief.md), [summary](summary.json),
[matched receipts](benchmarks.json), [live receipts](live.json) and
[verification manifest](verification.json) provide the reproducible boundary.
No merge, push, publication, tag, deployment or ChronoZarr change was performed.
