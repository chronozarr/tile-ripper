# Tile-Ripper — Reusable Native Grid Inspection and Faster Geographic Planning

Continue from committed `afe41c2` on `codex/acquisition-planner`.

The existing Tile-Ripper workflow now supports:

- Bounded CMR discovery of SWOT Raster D products.
- Authenticated, version-pinned HDF5 acquisition without system libhdf5.
- WGS84 AOI-to-native-pixel selection using actual source coordinates.
- Independently verified geographic masks and native-grid geometry.
- Quality-aware five-layer SWOT acquisition.
- Resumable source-byte transfers.
- Atomic scientific Zarr output for multiple observations.
- Existing native COG and Zarr v2/v3 readers.

The latest milestone passed 108 geometry oracles, 145 Rust tests, 16 feature combinations and scientific qualification of three real SWOT products.

**Objective: Make source-grid inspection reusable so geographic selection, job planning and acquisition do not repeatedly inspect the same immutable HDF5 metadata unnecessarily. Establish a small shared grid-description interface only where it genuinely serves multiple formats.**

## 1. Profile the redundant inspection work

Preserve the existing benchmark and scientific baseline.

For the three-granule live AOI job, the previous report measured:

- 89.33 seconds for geographic job generation.
- 55.66 seconds for two shape/full-axis inspection passes.
- 32.23 seconds for selected five-variable admission.
- 1.392 seconds for geographic computation.
- 244 wire HTTP requests during job generation.
- An additional 32.68 seconds for a separate normal job plan.

Trace exactly which operations repeat:

- Authentication and redirect resolution.
- Canonical source identity validation.
- HDF5 superblock/group inspection.
- Dataset metadata discovery.
- Chunk-index traversal.
- Full x/y coordinate retrieval.
- CRS parsing.
- Five-variable profile inspection.
- Scientific admission and plan construction.

Separate necessary fresh version checks from redundant data retrieval, parsing and computation.

Record timings, HTTP requests, source bytes, allocations and cache behavior for each phase.

Do not assume all repeated HEADs or authentication requests can safely be removed.

## 2. Design an immutable, version-bound inspection result

Introduce the smallest useful source-grid inspection representation.

It should contain, where supported:

- Canonical source identity.
- Strong ETag and object size.
- Actual CRS and coordinate representation.
- Grid dimensions.
- Axis ordering and direction.
- Validated x/y coordinate information or affine transform.
- Dataset shape/type/profile information.
- Relevant physical storage metadata.
- Versioned scientific interpretation metadata.
- Explicit capability and unsupported-profile status.

Distinguish:

A. Information required for geographic selection.

B. Information required for scientific-variable admission.

C. Information required for execution.

Avoid retaining huge decoded arrays unnecessarily.

Ensure all retained metadata and buffers are explicitly accounted for within existing admission limits.

An inspection result must never implicitly authorize reading a changed source object.

## 3. Eliminate redundant SWOT preparation

The current geographic resolver performs multiple authenticated preparations for shape, full axes and selected science.

Investigate whether one prepared source inspection can support:

1. Determining grid shape and CRS.
2. Reading actual bounded coordinate axes.
3. Resolving the geographic AOI.
4. Inspecting the selected five-variable scientific profile.
5. Constructing the final executable physical plan.

Reuse already-validated immutable metadata, coordinates and parsed index information.

Do not force reopening the same source merely because a later stage needs a different logical selection.

Preserve method-specific authentication and fresh source identity checks where required.

Do not change the scientific pixel-center inclusion algorithm, boundary tolerance or AOI-mask semantics.

No implicit use of CMR geographic coverage as native-grid evidence.

## 4. Evaluate safe reuse across CLI operations

`discover-job`, `plan` and `fetch` currently run as separate processes.

Determine whether the existing version-bound source checkpoint can safely retain reusable metadata ranges that are already validated.

Prioritize:

- In-process immutable inspection reuse.
- Existing checkpoint-range reuse.
- Rebuilding a scientific plan from verified source metadata.

Do not immediately create a separate persistent metadata cache.

If cross-process reuse is justified, it must:

- Reauthenticate online.
- Confirm the canonical source version before using persisted source bytes.
- Validate local retained bytes.
- Reject changed ETags, sizes, schemas or interpretation policies.
- Preserve per-source confidentiality and checkpoint boundaries.
- Never store signed URLs or credentials.
- Retain fresh final source validation.

Avoid serializing fragile internal parser objects if retained source bytes can be safely reparsed.

## 5. Develop a narrow cross-format grid contract

Evaluate how the inspected grid representation applies to:

- SWOT/HDF5 with explicit stored x/y coordinates.
- COG with an admitted affine native grid.
- CF-Zarr with explicit one-dimensional spatial coordinates.

These representations have meaningful differences.

Do not pretend every array has a regular affine grid.

Do not assume every CRS can be interpreted by the current pure-Rust projection support.

Represent supported coordinate models and missing capabilities explicitly.

Start by implementing reuse in SWOT. Extract shared types or traits only where they remove actual duplicate code or enable a concrete common operation.

Avoid a generalized GIS framework, arbitrary-rank coordinate model or new scheduling layer.

Existing COG and Zarr semantics must remain unchanged unless a separately verified improvement is warranted.

## 6. Preserve geographic scientific correctness

Repeat the full source-global center-selection oracles.

Verify:

- AOI bounds and Polygons.
- Polygon holes and concave boundaries.
- Boundary ties and tolerances.
- Reversed native axes.
- UTM-zone differences.
- Empty geographic selections.
- Geographic inclusion versus source validity versus WSE quality.
- Exact native window bounds.
- Original AOI and mask fingerprints.
- Independence from destination tile/chunk sizes.

Do not replace the validated full-center method with approximate projected-bound pruning just to improve speed.

Keep all five raw science values and their masks bit-exact.

The AOI transformation should remain independently checked against PROJ/GEOS.

## 7. Benchmark before and after

Use the three currently qualified SWOT products.

Compare:

A. Original committed implementation.

B. Optimized inspection reuse.

C. Normal independent `discover-job`, `plan` and `fetch` invocations.

D. A checkpoint-assisted reconstruction where appropriate.

Run repeatable benchmarks using frozen authenticated HTTPS fixtures, including simulated high-latency conditions.

Measure:

- Source inspections and parser opens.
- HDF5 metadata/index reads.
- Coordinate chunk reads.
- Canonical HEAD/GET operations and redirect hops.
- Total response-body bytes.
- Geographic resolution time.
- Job-generation time.
- Plan time.
- Fetch/output completion.
- Working reservations.
- Peak RSS.
- Scientific and version-validation outcomes.

Use at least one bounded live NASA run if necessary to establish that the optimized workflow remains compatible with actual archive authentication.

Do not interpret changing provider latency as an implementation speedup.

The key success measure is reducing redundant source inspection, not merely shifting it into another named phase.

## 8. Regression and failure gates

Preserve:

- Authentication and credential confinement.
- Source-version pinning.
- Metadata-range integrity and checkpoint validation.
- Late source mutation rejection.
- Scientific stream completion.
- Multi-granule aggregate admission.
- AOI/quality/source-mask separation.
- All-or-nothing Zarr publication.
- Cancellation, checkpoint recovery and interrupted output behavior.

Add cases where:

- Source changes between grid inspection and scientific execution.
- Coordinates change with the source ETag.
- Two distinct source versions share a canonical locator.
- Reused metadata cannot satisfy a later selection.
- A later source fails after earlier observations are staged.
- Metadata retention exceeds the requested memory budget.
- Different requests or concurrent preparations access the same cache/inspection context.

Repeat relevant native COG, Zarr v2/v3 and HDF5 scientific/fault suites, plus clean GDAL-free feature builds.

## 9. Decision gate

Deliver:

1. Exact identified causes of redundant inspection.
2. The implemented version-bound reuse mechanism.
3. Before/after authenticated metadata requests and bytes.
4. End-to-end AOI job-generation and plan-time improvements.
5. Scientific, geometry and source-version validation.
6. Working-memory and RSS tradeoffs.
7. A precise shared grid-description contract, if justified.
8. Whether existing COG and Zarr paths would benefit from it.
9. Remaining authentication/provider latency costs.
10. The next milestone most likely to improve the general-purpose acquisition engine.

Commit validated changes locally.

Do not merge, push, publish, tag, deploy or modify ChronoZarr.

Do not add NISAR, general reprojection, new file formats, distributed scheduling, arbitrary CRS handling or a new persistent caching subsystem.

**Success means a geographic SWOT acquisition job no longer repeatedly fetches or parses the same version-bound source-grid metadata unnecessarily, while preserving the established scientific and security guarantees.**

Implement the measured improvement rather than stopping at another architectural proposal.