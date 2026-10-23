# Tile-Ripper — Bounded SWOT Geographic and Temporal Discovery

Continue on `codex/acquisition-planner` from commit `68dc295`.

Tile-Ripper now supports authenticated, resumable, multi-granule SWOT Raster D PGD0 acquisition through its normal Rust API and CLI. It writes scientifically correct Zarr v2 datasets containing five raw science layers, masks, exact coordinates, quality policies, provenance and native-grid metadata.

The two-observation job passes independent h5py, NetCDF-C, Zarr-Python and xarray comparisons. Its aggregate resource admission, source checkpoints, failure recovery and atomic output-publication contracts are validated.

**Objective: Let scientists discover appropriate SWOT granules by geographic area and date, inspect them, and construct valid bounded acquisition jobs without manually finding NASA granule URLs.**

Do not expand this into a general Earth-observation catalog framework.

## 1. Reuse NASA CMR

Study the official CMR Search API and its granule metadata responses.

Implement a narrow CMR client for the qualified SWOT Raster D collection.

Support:

- WGS84 bounding-box searches.
- Date/time intervals.
- Explicit collection identity.
- Deterministic result sorting.
- Bounded pagination.
- Deduplication by stable granule identity.
- Version and processing-release identification.
- Granule footprint and acquisition time.
- Authoritative data-access links where available.

Resolve the exact collection concept ID from authoritative metadata rather than inventing or hardcoding an unverified identifier.

Prefer an existing suitable HTTP/JSON dependency.

Do not build a generalized STAC/CMR abstraction unless actual shared requirements justify it.

## 2. Enforce bounded discovery

Require explicit geographic and temporal constraints.

Define limits for:

- Maximum requested geographic extent.
- Maximum date span.
- Number of pages and results inspected.
- Response bytes and parsed metadata.
- Network concurrency and retries.
- Maximum candidate granules.
- Query execution time.

If a search exceeds limits, report that it was truncated or refused.

Never silently present the first four granules as a complete result set.

The existing acquisition job supports at most four observations. Discovery may return more candidates, but generating an executable job must require an explicit choice of no more than four.

Do not download science pixels during CMR discovery.

## 3. Carefully distinguish discovery and scientific compatibility

For every candidate, distinguish:

A. Reported by CMR as geographically and temporally relevant.

B. Has a supported, legitimate data-access locator.

C. Appears to be the correct Raster D PGD0 processing family.

D. Passes Tile-Ripper's actual native HDF5 scientific-profile inspection.

E. Is ready for a particular explicit native-index acquisition.

These are separate states.

CMR spatial metadata may describe an approximate granule footprint. Catalog intersection does not establish that a particular native pixel contains useful river WSE.

Do not infer full format compatibility from filenames or collection membership.

Inspect each chosen source through the existing authenticated preparation path before claiming that its science layers, fills, quality definitions, CRS and indexing are supported.

Reject ambiguous, unsupported or conflicting product identities explicitly.

## 4. Produce a useful discovery result

Add a documented user-facing discovery command or equivalent Rust API.

A conceptual experience is:

    tile-ripper discover \
      --collection SWOT_L2_HR_Raster_D \
      --bbox WEST,SOUTH,EAST,NORTH \
      --start START --end END \
      --limit 20 \
      --output candidates.json

This syntax is illustrative; fit the existing CLI conventions.

Report candidate granules with:

- Granule ID and canonical product identity.
- Acquisition start/end time.
- Version and processing identity.
- Spatial coverage information.
- Source size, if authoritatively available.
- Candidate source URL.
- Compatibility-inspection status.
- Explicit reasons for exclusions or uncertainties.

Do not put Earthdata credentials, cookies or signed URLs into discovery results.

Use deterministic ordering so the same frozen catalog results produce the same candidate list.

## 5. Generate an explicit SWOT job manifest

Implement a narrow bridge from selected candidates to the existing
`swot_job` manifest format.

Reuse its request, quality-policy, authentication, resource and checkpoint contracts.

Preserve unique source identities, expected-version assertions where available, explicit source selection and native-grid distinctions.

The difficult issue is geographic-to-native pixel selection.

Do not silently invent an index window from the CMR footprint.

For the first version, allow users to supply explicit native pixel windows and strides for selected granules. Where helpful, inspect actual source coordinates and report dimensions, CRS, coordinate ranges and candidate selection information.

An automated longitude/latitude AOI-to-native-index translator should be a separate scientifically validated capability unless the current milestone can implement and independently qualify it without broadening scope.

A generated manifest must not be described as ready to fetch until all required pixel selections are resolved and admission succeeds.

## 6. Validate with known and held-out products

Use the two already qualified PGD0 granules as known positive controls.

Build deterministic CMR fixtures containing:

- Multiple dates.
- Overlapping footprints.
- Duplicate catalog records.
- Missing data URLs.
- Unsupported processing releases.
- Invalid and ambiguous metadata.
- Empty results.
- Pagination limits.
- Changed catalog ordering.
- Granules whose footprints intersect an AOI but whose inspected native grids do not provide useful selected data.

Confirm that source identity and selection semantics are preserved from discovery through the existing job preparation.

Choose a few additional real candidate granules using a predeclared selection rule. Perform bounded compatibility inspection without automatically expanding scientific support.

Use CMR and archive queries respectfully. Run the larger test matrix against frozen fixtures.

## 7. Keep acquisition and authentication boundaries intact

CMR discovery is not authorization to download a protected scientific object.

Use the established explicit Earthdata session and method-specific redirect policy for archive inspection and acquisition.

Do not broaden credential forwarding or make discovery implicitly read a user's netrc file.

Do not replace source-version checks with catalog metadata.

Retain:

- Conditional source validation.
- Version-bound checkpoints.
- Scientific stream replay semantics.
- Aggregate working-memory and disk admission.
- All-or-nothing scientific Zarr publication.
- Original native stored values and masks.

Do not modify the scientifically qualified five-layer SWOT decoder merely to accommodate additional discovered granules.

## 8. Test the actual scientist workflow

Demonstrate:

1. Search by bounding box and date.
2. Inspect a bounded candidate list.
3. Select two qualified granules.
4. Resolve explicit native-index windows.
5. Produce the existing SWOT acquisition manifest.
6. Run the existing plan command.
7. Fetch or reconstruct the Zarr output.
8. Open both native-grid observation groups with xarray.

Verify that the discovery result and acquisition manifest preserve all expected canonical source identities, acquisition times, processing versions and scientific-policy choices.

Use the existing reference readers for scientific comparisons; the CMR client must not become a source of invented science metadata.

## 9. Performance and robustness

Measure:

- Discovery HTTP requests and bytes.
- Pagination and duplicate handling.
- Time to candidate list.
- Source-inspection overhead.
- Plan-construction time.
- Resource-admission overhead.
- Catalog failure and timeout behavior.

No broad acquisition speedup is required. This milestone is primarily about usability and reliable discovery.

Repeat relevant authenticated transport, checkpoint, scientific-output, COG, Zarr and HDF5 regressions.

## Decision gate

Deliver:

1. A runnable discovery command and typed Rust API.
2. Verified CMR query and pagination behavior.
3. Explicit candidate and compatibility statuses.
4. A bridge to the existing bounded SWOT job manifest.
5. A working end-to-end demonstration.
6. Exact catalog/source identity and provenance evidence.
7. Bounded network, memory and output behavior.
8. Scientific and adversarial test results.
9. The remaining gap between geographic discovery and automatic native-pixel selection.
10. A recommendation for the next engineering milestone.

Commit validated implementation and evidence locally.

Do not merge, push, publish, tag, deploy, modify ChronoZarr, or submit upstream HDF5 patches.

Do not add general reprojection, mosaicking, distributed execution, new formats, automatic time-series alignment or unlimited catalog crawling.

**Success means a scientist can find appropriate SWOT granules from geographic and temporal criteria, review their actual compatibility, and construct a scientifically explicit acquisition job without manually locating NASA product files.**

Implement the working discovery path; do not stop at a design document.