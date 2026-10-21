# Selective HDF5 v1 traversal: decision gate

**Pruning is possible for valid ordered v1 chunk indexes and materially reduces
remote metadata I/O. Keep it as an explicit experiment. Full traversal remains
the default because pruning cannot retain failure detection inside unread
subtrees.** The additive parser extension, tests and unpublished upstream patch
are committed locally on `codex/acquisition-planner`.

## Real-file result

The [NOAA GOES-19 request](goes19-request.json) selects native UInt16 cloud-top
temperature codes from a **28,545,639-byte** NetCDF-4 object, 22.5 times the
original rainfall control. `/TEMP` is 5424×5424 with 226×226 chunks,
shuffle/DEFLATE and a legacy v1 index. There are 548 allocated chunks in a
24×24 logical grid. Independent inspection identifies 16 index
nodes: one level-one root and 15 leaves. See [large-file.json](large-file.json)
and [summary.json](summary.json).

For the one-chunk selection, exact index traversal changes from **32 GETs /
23,800 bytes to 4 GETs / 3,040 bytes**, visiting 2 rather than 16 nodes. Payload
retrieval is the identical one GET / 87,452 bytes. All preparation, including
attributes and selected coordinates, falls from 76 GETs / 34,908 bytes to
48 GETs / 14,148 bytes. Only 101,600 response bytes are needed for the native
selective acquisition, about 0.36% of the object. Direct conditional NOAA reads
and h5py/NetCDF-C comparisons confirm the result; this is not a synthetic file.

| Selection | Policy | Nodes | Index GETs / bytes | All prep GETs / bytes | Payload GETs / bytes |
|---|---|---:|---:|---:|---:|
| sparse | full | 16 | 32 / 23,800 | 76 / 34,908 | 1 / 87,452 |
| sparse | selective | 2 | 4 / 3,040 | 48 / 14,148 | 1 / 87,452 |
| dense | full | 16 | 32 / 23,800 | 76 / 38,268 | 13 / 1,981,709 |
| dense | selective | 4 | 8 / 7,760 | 52 / 22,228 | 13 / 1,981,709 |
| noncontiguous | full | 16 | 32 / 23,800 | 85 / 34,674 | 28 / 1,433,989 |
| noncontiguous | selective | 7 | 14 / 11,000 | 67 / 21,874 | 28 / 1,433,989 |

Dense means the 904×904 window beginning at (2000,2000); noncontiguous means
5×6 samples at steps (901,677), beginning at (226,452). The latter includes
unallocated chunks. All values, validity bits, stored coordinates and selected
chunk sets match full traversal and independent references. The original
`fe58a2c` executable has the same full-index counts. Whole-grid geometry tests
visit the same complete index under both policies; pruning is not always an
I/O saving.

The primary mirror downloaded **6,159,747 bytes** and
the separate broader-page tuning mirror totals **12,358,247
bytes**, including the reused primary extents. The latter required only
0 additional provider bytes. Both remain
under the explicit 16 MiB cap. Extent hashes and version identities are retained;
HTTP replay refuses uncaptured holes. No massive object was staged, and native
acquisition has no whole-file fallback.

## Performance and resources

The completed matrix has **324 cells**, three processes per combination,
including 0, 20 and 80 ms added response delay. It compares full and selective
exact reads, bounded 4/16 KiB native metadata pages, and h5py exact/16/64/256 KiB
range strategies. Local-file timings and the small-object whole-file control
are separate. At 80 ms, median completion seconds are:

| Selection | Full / exact | Selective / exact | Full / 16 KiB | Selective / 16 KiB | h5py / 64 KiB | h5py / 256 KiB |
|---|---:|---:|---:|---:|---:|---:|
| sparse | 6.893 | 4.721 | 2.417 | 1.129 | 0.815 | 0.661 |
| dense | 7.734 | 5.598 | 2.928 | 1.868 | 3.199 | 2.828 |
| noncontiguous | 9.250 | 7.704 | 3.612 | 2.819 | 3.935 | 3.262 |

Metadata batching can outperform pruning alone. Mature h5py remains faster for
the sparse selection; the measured selected/batched native path is competitive
for denser work. This is a delay-only loopback experiment, not a universal
performance ranking. First-native-chunk timing is compared with whole-slice
return for h5py, and native pre/post checks use more HEADs. See the complete
[phase, CPU and RSS tables](performance.md), [local results](local.json) and
[mature metadata/payload byte partition](mature-io.json) for these differences.
libhdf5 internal node visits are not exposed by its file-object API; its exact
requested byte ranges are retained instead of inventing visit counts.

For the 1.27 MB rainfall control at 80 ms, whole-file h5py takes
0.265 s versus
1.182 s for selected native acquisition
with 16 KiB metadata pages. Whole retrieval really is cheaper in this small,
delay-dominated control; it is explicitly labelled reference tooling.

All native benchmark reservations fit the declared 32 MiB budget. The largest
observed benchmark reservation is 7.16 MiB and peak native
RSS is 20.31 MiB. Separate resource gates acquire 0.5 and 32 MiB of
output under 4 and 8 MiB working budgets; the selected-mode RSS ranges from
11.25 to
16.50 MiB. Working reservations
are not an RSS cap; runtime, HTTP, receipt/inspection structures and caller
retention have their documented scope. [Resource receipts](selective-resources.json)
also retain declared reservations. Timing, scientific and fault receipts name
the exact final native binary SHA256 in [final-binary.json](final-binary.json).
The original mirror-capture records retain their earlier capture-stage hash;
final large-file comparisons use the current benchmark receipts.

## Scientific, fault and structural decision

Passed: the original **64 scientific and 33 fault cases**, **56 added scientific
comparison cells**, **33 added fault/policy cells**, and three independently
checked large-file selections. Added coverage includes boundary/first/last and
intermediate keys, several chunk shapes, full and strided selections, holes and
fully unallocated storage, reverse payload allocation, bad pointers/counts,
truncation, parent/key/level inconsistencies, mutation, page-cache framing,
cancellation and small budgets. Some scope tests deliberately expect selected
acceptance of an unrelated damaged subtree; they document the limit, not a
claim that every corrupt file is rejected. A further [123-case NetCDF-C axis
audit](netcdf-coordinates.json) independently confirms the stored coordinates,
with automatic scientific packing disabled.

All **269 upstream library unit tests** and all-target Clippy pass. The root
default/no-default/native-cog/native-hdf5/all-features test and Clippy matrix
passes, with final HDF5/all-features checks refreshed. Inherited COG, Zarr v2,
CF, Zarr v3, streaming, durable-output and fault gates pass. Receipts are under
[inherited/](inherited/) and [verification.json](verification.json).

The [audit](audit.md) explains lexicographic Cartesian pruning, the byte
dimension, missing records and validation. Visible-node validation is stronger
in several respects, but an unread descendant cannot be checked. Full traversal
is retained by default. Recommend **against default adoption** under the
inherited whole-index failure contract. `TILE_RIPPER_HDF5_INDEX_POLICY=selective`
is an explicitly selected experiment; metadata pages are independently opt-in.

## Upstream and next milestone

The additive, opt-in [upstream proposal](upstream-proposal.md) and its
[apply-checked patch](upstream.patch) are ready for review. Existing collector/read-slice signatures
and defaults remain compatible, and selected entry caches cannot bypass later
full traversal. Only two upstream source files change. No issue or PR was
published, and no default behavior was silently weakened.

Actual bounded inspection found **v1 indexes in both SWOT Raster and the NISAR
simulated GCOV sample**. SWOT's compressed rank-one coordinates exceed the
current axis-storage profile. NISAR's CF NaN marker accompanies implicit zero
storage initialization, a deliberately rejected ambiguity in the current
reader. Official method-specific redirects are also outside native transport;
the SWOT inspection used existing Earthdata authentication. These are observed
restrictions, not guesses that those products require modern index families.
Stored signed Int16 excludes the larger inspected GOES CMI/radiance files.
See [actual layouts and access evidence](products.md).

Recommend the next narrow milestone: bounded chunked-coordinate acquisition,
followed by actual SWOT Raster qualification through a deliberate archive-access
contract. Resolve upstream implicit-fill semantics before admitting the NISAR
candidate. Keep modern index families and additional pixel types separate until
their own physical/scientific cases are demonstrated. No product adapter,
general HDF5 implementation, merge, tag, push, release, deployment or ChronoZarr
change is included. [Reproduction commands](workflow.md) retain the complete
method and source-validation limits.
