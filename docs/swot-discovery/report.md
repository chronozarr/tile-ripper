# Bounded SWOT geographic and temporal discovery decision gate

**Pass for the requested explicit-native-selection milestone.** Continuing
`68dc295` on `codex/acquisition-planner`, the normal Rust CLI can discover Raster D
candidates from geographic/time constraints, inspect explicitly selected sources,
generate a pinned SWOT job, plan it, and acquire independent native observations
into scientifically verified Zarr v2. Implementation and evidence remain local.

Start with [the runnable scientist workflow](workflow.md),
[two-source selection](../../examples/swot-discovery-selection.json) and
[generated existing-format manifest](generated-job.json).

| Required decision | Result |
| --- | --- |
| Runnable discovery and typed API | `discover`, `swot_discovery::{CmrClient, Query, Report, Candidate}`; existing reqwest, serde_json and chrono dependencies |
| Authoritative collection | Runtime exact short-name/version/provider resolution; verified `C3233944993-POCLOUD`, D, POCLOUD, collection revision 24 |
| Geographic/time search | Explicit WGS84 bbox and RFC3339 interval, positive span <=31 days, bounded geographic extent |
| Pagination and identity | CMR Search-After, same query on every page, deterministic start/UR/concept ordering, stable-concept deduplication, explicit conflicting records |
| Candidate statuses | Catalog relevance, legitimate supported access, apparent family, uninspected scientific profile and unresolved native selection are separate fields |
| Manifest bridge | One to four explicit choices/windows/strides/policies, actual authenticated native job inspection, aggregate admission and observed strong ETag assertions |
| End-to-end science | Both original qualified products pass guarded h5py and NetCDF-C comparisons, Zarr-Python and xarray after acquisition and full checkpoint reconstruction |
| Additional candidates | Three selected by the predeclared rule pass bounded structural/profile inspection; no new scientific qualification is inferred |
| Bounds and failures | Explicit truncated/failed reports and nonzero CLI status; extent, time, page, record, candidate, body, parsed metadata, retry and execution-time caps |
| Geographic-to-native selection | Explicit caller indices remain required; catalog intersection never produces a fabricated index window or useful-WSE claim |

## Catalog/source identity evidence

[Official CMR search documentation](https://cmr.earthdata.nasa.gov/search/site/docs/search/api.html)
defines bbox/temporal searches and Search-After pagination.
[NASA's Raster D product page](https://podaac.jpl.nasa.gov/dataset/SWOT_L2_HR_Raster_D)
and [the authoritative collection metadata](https://cmr.earthdata.nasa.gov/search/concepts/C3233944993-POCLOUD.umm_json)
identify the collection. The implementation resolves it afresh rather than
embedding this concept ID. [Frozen exact response hashes and query URLs](catalog-captures.json)
and [the checked-in CMR captures](../../tests/data/cmr/) retain the metadata used
for deterministic tests.

The [predeclared protocol](protocol.json) fixed both known-control searches and
the additional selection rule before acquisition. The original-control bbox
`[-73,41,-70,44]` and held-out bbox `[-179,67,-174,70]` each returned six candidates.
Both live searches used page size two and traversed three populated pages plus
the terminal empty page. Each search made five CMR requests including collection
resolution. [Original results](known-original.json), [held-out results](known-held.json)
and [live workflow evidence](live-workflow.json) retain collection/granule revisions,
exact catalog times, PGE identities, footprints, sizes and unsigned archive links.

| Selected observation | CMR concept | Native descriptor | Strong source ETag | Object bytes |
| --- | --- | --- | --- | --- |
| `held_out_20240217` | `G3927559976-POCLOUD` | `100m_UTM01W_N_x_x_x`, cycle 011 / pass 082 / scene 016F | `"8c583d1085e24a2c438260a206d38131-1"` | 6,527,265 |
| `original_20240216` | `G3927359490-POCLOUD` | `100m_UTM19T_N_x_x_x`, cycle 011 / pass 035 / scene 115F | `"3b93eefb90f0167506435fdca199aa69-1"` | 89,242,093 |

Both selected files report PGD0, product counter 01 and PGE 5.4.0. Canonical
filenames/URLs and full intervals are in the generated manifest. Source asset
IDs are the CMR granule concepts, with distinct caller observation IDs. Catalog
provenance is retained in the job, plan, receipt and Zarr attributes. Every
normal plan/fetch with that provenance validates it against actual source
metadata; saved discovery results never substitute for source version checks.

Catalog granule times and native coverage times differ slightly. For example,
the original catalog start is `2024-02-16T02:51:53.691Z`, while native coverage
starts at `2024-02-16T02:51:54.231971Z`. Both remain separately identified.
Native granule endpoints are checked against filename endpoints within the
filename's one-second precision. The source `crid`, product counter, descriptor,
cycle/pass/scene, PGE and size assertions are checked independently of those
times. No CRS, flag definition, fill value or science value comes from CMR.

## Scientist workflow and scientific comparisons

The [workflow receipt](live-workflow.json) records live search, explicit selection,
authenticated job generation, normal checkpointed `plan`, `fetch`, and complete
scientific reconstruction. A later build repeats generation and reconstruction
and checks the catalog attributes and their time-origin annotation.

The held-out window is rows `[0,128)`, columns `[656,784)`; the original window
is rows `[600,728)`, columns `[400,528)`. Both use unit strides and the unchanged
good/suspect policy. Manifest execution order is held-out then original,
deliberately different from acquisition-time order. Their native grids remain
separate and are never aligned or mosaicked.

Each output was opened through Zarr-Python and `xarray.open_zarr` with raw
mask/scale/time decoding disabled. The existing independent reference worker
compared all stored bits in all five arrays, five independent source masks,
derived quality masks, exact x/y coordinates, scalar time, source identity,
interpreted attributes, and HDF5 attribute storage bytes/shapes. Separate h5py
(HDF5 1.12.2) and NetCDF-C (4.9.3 / HDF5 1.14.6) processes used the previously
qualified captures under the capture-coverage guard, with zero capture misses.
The native source reader/decoder was not modified.

The known windows retain 1,310 and 3,076 accepted cells, respectively. A third
acquisition of the held-out source selects `[600:728,400:528]` and retains zero
accepted cells. Both reference readers agree exactly on that output too. The
same CMR-intersecting granule can therefore pass native metadata/profile
inspection while an explicit native selection provides no accepted WSE.
None of these checks identifies river pixels.

## Additional compatibility inspections

The rule chose the first three previously unqualified 100 m PGD0 candidates in
the original region over February 16–18, 2024, excluding the two controls and
sorting by catalog start/UR/concept. Each was inspected once with a `[0:16,0:16]`
unit-stride window, without streaming science arrays or substituting a later
candidate after a failure. [Exact results](additional-inspections.json):

| CMR concept | Descriptor / pass / scene | Native shape | Inspection seconds | Source-body bytes |
| --- | --- | --- | --- | --- |
| `G3927661761-POCLOUD` | UTM19T / 035 / 114F | 1,578 × 1,577 | 12.019 | 345,790 |
| `G3927300707-POCLOUD` | UTM19T / 035 / 116F | 1,582 × 1,581 | 14.420 | 378,446 |
| `G3925708721-POCLOUD` | UTM18T / 063 / 115F | 1,540 × 1,540 | 12.685 | 296,661 |

All pass the existing structural/profile and explicit-selection inspection.
This records compatibility at their inspected versions; it does not establish
independent numerical qualification of additional data or enlarge the supported
processing releases. Uncertainty/count/quality semantics remain unchanged.

## Resource and performance observations

These are single bounded runs, with provider/network latency included; no broad
acquisition speedup is claimed. Response counts/bytes and source-inspection
metrics are retained in the JSON evidence.

| Operation | Observed cost |
| --- | --- |
| Original CMR query | 5 requests; 74,072 response-body bytes; 1.380 s |
| Held-out CMR query | 5 requests; 73,103 response-body bytes; 1.222 s |
| Additional two-day query | 2 requests; 133,614 response-body bytes; 0.690 s; 12 candidates |
| Two-source draft construction + aggregate admission | 0.000623 s |
| Two-source native profile/selection inspection | 23.948 s; 47 source HTTP requests; 675,605 source-body bytes |
| Normal first checkpointed fetch | 30.885 s; 2,256,418 source-body bytes including final source sweeps |
| Complete two-source checkpoint reconstruction | 23.672 s; 32 source-body bytes including final sweeps |
| Alternate empty held-out selection | 19.593 s; 453,897 source-body bytes |
| Aggregate admitted working cap | 101,257,216 bytes against a 134,217,728-byte limit |

The fetch/reconstruction/empty-selection total is 2,710,347 bytes against the
predeclared 16 MiB acceptance cap; this total excludes the separately reported
generation/plan/additional-inspection transfers, redirect bodies and wire framing.
The later two-source reconstruction adds 32 source-body bytes and separately
records process RSS and timings. Working reservations are not an RSS ceiling.
The first parent summary hit an asset-ID accounting error after successful
science comparisons; receipts were recovered and rechecked locally. No provider
data were refetched to repair that summary, and missing parent RSS/CPU samples
are explicitly labeled unavailable. Later complete runs retain those measurements.

Discovery's complete hard-limit table is in the [workflow](workflow.md). All
catalog requests are serial, with at most 33 attempts in one query, bounded
response and parsed metadata, no redirects/credentials and a shared deadline.
The discovery JSON has a 4 MiB cap; selection input and executable job input have
64 KiB caps, and the job review has a 3 MiB cap. Native acquisition retains its
existing source budgets, job reservations, checkpoint quotas, conditional version
checks, serial source ownership and checked no-replace scientific publication.

## Adversarial and regression evidence

[Verification receipt](verification.json) records command results and hashes.
The final Rust run passed 140 tests, with zero failures and eight existing
environment-dependent/helper skips. Separate fixture matrices passed 38
authentication scenarios, 32 checkpoint scenarios and 19 job fault/restart cases.
The Rust suite exercises collection identity resolution, exact query encoding,
Search-After behavior, changed catalog order, duplicate/conflicting records,
multiple dates with overlapping footprints, missing data URLs, unsupported and
ambiguous processing identities, malformed dates/geometry/JSON, empty results,
candidate/page/record/body/string bounds, server timeouts, redirects, 429/503,
missing/repeated cursors, explicit index/policy preservation, aggregate admission,
native identity conflicts and CLI output/input contracts.

The existing scientific job matrix is repeated against frozen native captures
and independent guarded reference inputs, including restart, source mutation,
cancellation, failed later sources, checkpoint quotas, heterogeneous grids/strides,
all-invalid selections and scientific publication races. Authentication and
checkpoint fixture matrices are repeated separately. Active COG, Zarr and HDF5
regressions run through the all-feature/all-target Rust suite; environment-bound
ignored tests are identified in the verification receipt rather than counted as
executed tests. Clippy, formatting and feature-boundary builds are checked.

## Remaining gap and next milestone

Geographic discovery is now usable without manually locating archive URLs.
The deliberate remaining step is the scientist's native pixel selection.
The next milestone should independently validate a narrow WGS84-AOI-to-native
selection capability using inspected CRS and actual coordinate arrays, with
pixel-center/boundary, axis-direction, antimeridian, multiple-grid and empty-AOI
controls. It should return explicit windows and any separately defined AOI mask
for review. Useful quality-accepted WSE and river membership must remain separate
claims. General reprojection, resampling, mosaicking and automatic time-series
alignment remain outside this capability.

Nothing was merged, pushed, published, tagged or deployed; ChronoZarr and upstream
HDF5 sources were not modified.

[Final local two-observation dataset](../../prototype-output/swot-job/discovered-final-acceptance-xx8v_cum/observations.zarr)
is the checked acceptance-binary reconstruction. It is an ignored local artifact;
its source identities, independent comparison results and binary hash are retained
in the versioned workflow/verification receipts.
