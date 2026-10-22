# Version D quality-aware SWOT WSE: decision gate

**PASS for scientifically correct, bounded five-layer Version D acquisition and
explicit quality-aware analysis on a provider-identity-pinned mirror. The
authenticated live-native archive gate remains CLOSED.** Validated locally on
2026-10-22, continuing from `dbafecc` on `codex/acquisition-planner`.

## 1. Scientific correctness

All 38 quality-aware science cells agree bit-for-bit with separately executed
h5py and NetCDF-C for every selected raw value, source mask, coordinate and
derived mask. Three further raw multi-variable cells validate the ordinary
stream API. Cases include real D sparse/dense/missing windows, boundary crossings,
noncontiguous strides, both byte orders, independent physical layouts, high
UInt32 bits and unassigned conditions. See [science](science.json),
[raw stream science](raw-science.json) and [policies](policies.md).

The demonstrative `[600,400]`, 128 x 128 window uses the explicit Cookbook good
or suspect policy. It selects **3,076 pixels**, with stored WSE
mean **275.451613110 m**, mean available per-pixel one-sigma uncertainty
**0.379689272 m** (3,076 samples), and
**21,757** contributing PIXC samples across selected
cells. Both independent readers exactly reproduce these counts, sums and means.
The uncertainty mean is descriptive, not uncertainty of the spatial mean.
This is not a river-specific SWOT processing workflow.

## 2. Exact stored types and variables

The native HDF5 adapter now admits UInt8, UInt16, UInt32 and Float32 within its
existing rank-two/v1-index/filter profile. Source byte order is interpreted
exactly; public values retain native integer types and original attributes.
The D workflow streams `/wse` Float32, `/wse_qual` UInt8,
`/wse_qual_bitwise` UInt32, `/wse_uncert` Float32 and `/n_wse_pix` UInt32.
WSE/uncertainty CF fill is 9.969209968386869e36; summary fill is 255;
UInt32 fill is 4294967295. The actual storage and CF fills agree. Implicit
initialization remains separate from CF missingness; ambiguous states fail.

Stored Float64 `/x` and `/y` remain exact rank-one coordinates. Source units,
packing, WKT/CF CRS, processing identifiers, original attribute bytes and
source identities are preserved. The source count-units string `l` is retained
literally. Rank-two Float64 `/illumination_time` is inspected and explicitly
unsupported. Granule/coverage times suffice for this single-observation summary;
per-pixel timing is needed before time-sensitive inter-observation analysis.

## 3. Validated quality semantics

Raw source missingness; explicitly accepted good/suspect/degraded/bad categories;
and named bitwise required/excluded conditions are separate policies. The
Cookbook example accepts good and suspect. Bitwise requires every requested
condition, excludes every named veto and rejects unassigned observed bits.
Source definitions must match before applying interpreted policies; unknown,
duplicate or contradictory requested names fail. Raw auxiliary arrays and
independent masks remain available under every policy. No uncertainty/count
threshold, vector-product bit meaning or universal good-pixel definition is
inferred. The exact 19 D masks, definitions and ambiguities are in
[policies.md](policies.md) and recorded in each stream's provenance.

## 4. Exact logical alignment and bounded delivery

All five arrays share the requested logical source indices, positive strides
and stored coordinate centers. The actual D summary array has one 1580 x 1580
physical chunk; the four other science arrays use 790 x 790 chunks. Each uses
its own index and encoded ranges. Synthetic files also vary all five layouts.
Output order is deterministic by WSE chunk row/column and admitted logical
subtile, with PATHS order within a tile. Values and calculations are provisional
until `finish()` validates the final source identity. Cancellation/drop joins
the worker. An execution-local bounded encoded-range LRU and shared coordinate
preparation avoid redundant reads; decoded chunks may be recomputed across
logical intersections. Full index traversal remains the default. Experimental
selective traversal does not validate skipped subtrees.

## 5. Physical bytes and requests

The real object is **89,242,093 bytes**, strong opaque ETag
`"3b93eefb90f0167506435fdca199aa69-1"`. Its exact filename and all inspected layouts, filters,
allocated extents, fills and attributes are in [product-profile.json](product-profile.json)
and [native-profile.json](native-profile.json). All selected arrays are aligned
1580 x 1580 `(y,x)`, shuffle + DEFLATE 4; axes have one 1580-sample compressed
chunk each. The source projection is WGS 84 / UTM 19N (EPSG:32619).

The default exact five-layer 128 x 128 acquisition retrieves
**2,168,859 bytes (2.430% of the product)**
through **121 GET + 5 HEAD** requests, with
**588 duplicate response bytes**. Per-phase payloads:

| Variable / traffic | Bytes | GET |
|---|---:|---:|
| hdf5_wse_payload | 668939 | 1 |
| hdf5_summary_quality_payload | 334210 | 1 |
| hdf5_bitwise_quality_payload | 268327 | 1 |
| hdf5_uncertainty_payload | 727811 | 1 |
| hdf5_pixel_count_payload | 133023 | 1 |
| hdf5_coordinate_chunks | 1728 | 2 |
| hdf5_coordinate_index_discovery | 208 | 4 |
| hdf5_chunk_index_discovery | 1080 | 10 |
| metadata_index_discovery | 33533 | 100 |

The two coordinate payloads total 1,728 bytes across all five variables.
WSE alone retrieves 693,244 bytes through 73 GET + 5 HEAD; adding all four
auxiliary layers increases physical traffic by 1,475,615 bytes and 48 GETs.
Shared metadata/coordinates prevent five independent preparation passes.
WSE-only versus raw multi-variable versus aligned quality-aware totals,
metadata/page overlap and per-phase requests are in [acquisition.json](acquisition.json)
and [performance.md](performance.md). There is no full-product acquisition.
The separately labelled reference inspection fetched 61,905 bytes;
reference capture fetched 9,173,485 bytes in 46 GETs
under a 24 MiB cap. Captured-extent hashes are not a whole-file checksum.

## 6. Working reservations, CPU, RSS and completion

For the default exact 128 x 128 aligned operation, the three-run median is
**65.78 ms** completion and
**47.98 ms** native process CPU, maximum peak RSS
**26.69 MiB**, reservation
**26.29 MiB** within a 32 MiB working budget.
At 20 ms added per-response delay, exact metadata takes
3.637 s versus 0.911 s
with the verified optional 16 KiB metadata policy. All 114 benchmark cells and
mature h5py/NetCDF-C controls are retained in [benchmark.json](benchmark.json)
and [performance-summary.json](performance-summary.json). These controlled
loopback results are not provider-throughput or universal performance claims.

The tradeoffs are visible in the controls: at 20 ms response delay, h5py exact
ranges complete the window in 1.892 s versus native exact 3.637 s, reflecting
fewer metadata requests. The 16 KiB policies are close (native 0.911 s, h5py
0.942 s). On the local dense slice, native aligned completion is 49.7 ms versus
h5py 30.1 ms; native repeats decoder intersections to bound assembled output.
Decode counters include first visits as well as repeats. These are observed
profile-specific results, with the differing validation scopes described above.

| Raw output | Working budget | Peak reservation | Process peak RSS | Completion |
|---:|---:|---:|---:|---:|
| 0.265625 MiB | 4 MiB | 1.52 MiB | 11.48 MiB | 3.3 ms |
| 0.265625 MiB | 8 MiB | 2.02 MiB | 11.50 MiB | 3.2 ms |
| 17 MiB | 4 MiB | 2.00 MiB | 18.14 MiB | 55.8 ms |
| 17 MiB | 8 MiB | 2.50 MiB | 18.22 MiB | 57.5 ms |
| 68 MiB | 4 MiB | 2.98 MiB | 36.53 MiB | 320.5 ms |
| 68 MiB | 8 MiB | 3.59 MiB | 35.84 MiB | 226.6 ms |

[Resources](resources.json) show bounded tiled assembly for up to 68 MiB of raw
output. [Real D admission](real-resources.json) rejects insufficient 4/8/16/24 MiB
budgets and succeeds at 32 MiB. Reservations are working admission, not RSS caps:
runtime/allocator overhead, inspection clones, plan representations and caller
retention differ. RSS grows with selected plan/chunk count; it is not presented
as independent of total selection size.

## 7. Authenticated live-native archive access

**CLOSED.** Fresh Version D official authenticated eight-byte conditional GET
and wrong-ETag rejection succeed in independent reference tooling. HEAD and GET
use method-specific signed routes; cross-method reuse returns 403. Native
canonical inspection still rejects redirects. No native authentication or HTTP
validation was weakened. The caller-configured origin, credential/cookie,
redirect and final-object invariants remain a separate gate in
[access-design.md](access-design.md), with fresh evidence in [access.json](access.json).
No secret, cookie value or signed URL is committed.

## 8. Version C versus actually inspected Version D

The target is the actual **PGD0 Version D**, PGE 5.4.0, created 2025-12-22,
for the 2024-02-16 cycle-11/pass-35/scene-115 observation. The older PIC0 Version C
object is 92,416,806 bytes and PGE 5.1.1. Both inspected grids and chunk shapes
match, but raw science values differ across reprocessing and must not be mixed.
D declares bit 9 (512) and bit 22 (4194304), absent from the inspected PIC0 flag
definitions. The newer PDD is document Revision C, which is not the data release.
The original C WSE/coordinate suite still passes all 18 cells. The interpreted
quality-aware wrapper requires D identity/schema; C auxiliary quality acquisition
and other D scenes/PID0 processing are not blanket-qualified by this result.

## 9. Remaining workflow barriers and regression gate

Live native archive access is the main usability barrier. Geographic AOI mapping,
per-pixel Float64 timing, time-series alignment, broad CF interpretation,
river masks/corrections and wider HDF5 types/index families remain outside scope.
Large physical chunks impose a decode/reservation floor. Aligned intersections
can repeat decompression despite encoded-range cache hits; metadata batching
helps latency but does not remove this CPU tradeoff.

The [verification receipt](verification.json) records 31 new rejected fault cases,
six synthetic and five real budget cases, the Rust test/Clippy feature matrix,
270 vendored-reader tests, original C science, 146 compressed-axis
science cases, HDF5 full/selective science/fault/resources and coordinate checks,
and COG/Zarr v2/v3/streaming/cancellation/transactional-publication gates.
The native build requires no GDAL or libhdf5. Existing default GDAL behavior is
retained. A small local vendored-parser guard rejects nonzero integer bit offsets,
partial precision and unrepresentable widths instead of misdecoding them.
Three independently written noncanonical fixtures are rejected; the
[admission receipt](integer-admission.json) includes the confirmed pre-guard
16-versus-1 counterexample and final explicit rejection. The existing
selective-index patch and dependency lockfiles are unchanged; no ChronoZarr or
upstream repository was modified.

## 10. Best next general-purpose milestone

Implement and adversarially qualify explicit caller-configured authentication
and method-specific redirects in the shared conditional-range transport. This
would make the verified D science workflow usable against the official archive
and help other protected scientific products. Preserve version validation and
secret confinement as entry gates. Reducing repeated decompression is a
subsequent measured optimization, not a reason to weaken the access contract.

Implementation and validated reports are committed locally. No merge, push,
tag, publication, deployment or upstream HDF5 submission is part of this work.
