# Tile-Ripper — Bounded SWOT Raster Acquisition

Continue on `codex/acquisition-planner` from commit `92a6f6b`.

The existing native HDF5 adapter supports restricted rank-two datasets, conditional remote byte-range acquisition, bounded source-chunk extraction, and scientifically validated native streaming.

A separate experimental selective-v1-index implementation now reduces metadata requests substantially on real GOES-19 files. Full-index traversal remains the default because selective traversal cannot validate skipped subtrees.

Actual preliminary product inspection found legacy v1 HDF5 indexes in SWOT L2 HR Raster and a NISAR GCOV example.

**Objective: Extend the existing native HDF5 adapter just enough to retrieve scientifically meaningful SWOT L2 HR Raster water-surface elevation subsets, without downloading the entire product or installing libhdf5.**

This is a product-driven interoperability milestone, not a general HDF5 expansion.

## Phase 1 — Inspect the exact SWOT product

Use the existing inspection evidence in `docs/hdf5-selective/products.md` and the official SWOT L2 HR Raster documentation.

Identify the actual target product and version, dataset paths, axes, dimensions, compression, chunk shapes, chunk indexes, coordinate conventions, missingness, quality variables, and CRS metadata.

Prioritize WSE and its associated projected x/y coordinates.

Inspect the actual file rather than assuming the product's published schema covers every physical detail.

Record precisely which characteristics already fit Tile-Ripper's HDF5 admission contract.

Determine the minimum additional capability necessary for a useful WSE subset.

## Phase 2 — Bounded chunked-coordinate support

Extend the current HDF5 axis reader to accept compatible rank-one chunked coordinate arrays.

Requirements:

- Reuse `hdf5-reader` for indexing, chunk decoding, filters and numeric interpretation.
- Fetch only coordinate chunks intersecting the requested coordinate selection, where the storage layout supports this.
- Support the exact numeric dtypes and filters verified in the chosen real SWOT product.
- Preserve coordinate values and units.
- Preserve axis ordering and requested strides.
- Handle partial, absent or invalid coordinate chunks explicitly.
- Retain original source metadata and source-version identities.
- Charge coordinate metadata, encoded buffers and decoded chunks to existing resource admission.
- Use bounded reads rather than materializing entire coordinate arrays unnecessarily.

Keep contiguous and compact coordinate paths working exactly as before.

Do not add a second HDF5 parser or introduce an unbounded coordinate cache.

## Phase 3 — Establish legitimate remote access

Investigate the actual SWOT archive access path.

The prior inspection required Earthdata authentication and encountered redirects unsuitable for the existing anonymous conditional-range transport.

Determine whether a supported, authenticated byte-range route can provide:

- Stable source identity.
- Exact object length.
- Conditional GET or equivalent defensible version validation.
- Bounded, verifiable range responses.
- Appropriate redirect handling.
- Credentials supplied by the caller without embedding them in source code or logs.

Reuse the existing HTTP transport where possible.

Do not weaken source-version validation merely to follow redirects.

Never commit credentials, cookies, access tokens or authenticated URLs.

Do not circumvent access controls.

If the archive does not expose a suitable range interface, continue scientific validation with a legally obtained local file and bounded local mirror, clearly distinguishing that result from live remote acquisition.

Do not describe a local mirror as proof that the real provider supports selective HTTP retrieval.

## Phase 4 — Implement a restricted SWOT WSE workflow

Using the actual inspected product, demonstrate selecting a spatial subset of WSE and the matching coordinate arrays.

Preserve:

- Native stored scientific values and dtype.
- Coordinate values and units.
- Original CRS/projection metadata.
- Source missingness and fill-value semantics.
- Scale/offset packing, where present.
- Relevant acquisition identifiers and timestamps.
- Dataset path and source provenance.
- Successful final validation before committing a result.

Do not assume every HDF5 dimension is geographic.

Do not resample, reproject, mosaic or reinterpret projected coordinates as longitude/latitude.

Start with explicit dataset paths and index-space slicing. Add geographic AOI selection only if correct mapping can be independently demonstrated without broadening the milestone.

Use the existing public native streaming contract, or make the smallest necessary compatible extension.

## Phase 5 — Independent scientific validation

Compare Tile-Ripper's results with independently executed h5py and netCDF4 references.

Test:

- Exact WSE stored values.
- Projected x/y coordinate arrays.
- Chunk boundaries and noncontiguous slices.
- Coordinate chunk decoding and partial chunks.
- NoData, HDF5 initialization and CF fill-value distinctions.
- Floating-point/NaN handling where applicable.
- Source calibration and units.
- Dataset and observation identity.
- Data and coordinate mutations between preparation and final delivery.
- Corrupt/truncated compressed coordinate chunks.
- Malformed index pointers and unsupported layouts.
- Cancellation and small memory budgets.

Retain the existing full-index traversal as default.

If selective traversal is tested, label its reduced validation scope and compare it independently.

Repeat inherited HDF5, COG, Zarr v2/v3, streaming and publication regressions.

## Phase 6 — Physical acquisition and performance

Measure the complete subset operation:

- Total original file size.
- Requested scientific samples and coordinates.
- HDF5 metadata/index bytes.
- Coordinate chunk bytes.
- WSE payload bytes.
- GET/HEAD counts.
- Duplicate/overfetched ranges.
- Preparation latency.
- Time to first chunk.
- Total completion time.
- CPU and peak RSS.
- Declared working reservations.

Compare full and experimental selective-index strategies, exact ranges and bounded metadata batching.

Include mature h5py/netCDF4 controls, with validation and local-versus-remote access differences stated explicitly.

Use a small number of bounded real-source requests. Perform repeatable optimization benchmarks on a local mirror.

No full-file network download should be silently performed as part of a purported selective acquisition.

## Phase 7 — Evaluate scientific usefulness

Demonstrate one modest but meaningful WSE calculation from the selected data, such as valid-pixel counts, a spatial WSE summary, or a simple elevation profile where the coordinates support it.

Independently reproduce the calculation.

Explain exactly which raw values, calibration and quality criteria were used.

Do not equate basic fill-value masking with all SWOT science-quality filtering.

Report which additional quality flags, metadata or dimensions would be needed for more sophisticated river analyses.

## Phase 8 — Keep NISAR separate

Retain the NISAR GCOV findings for the next planning decision.

The inspected candidate exposed an ambiguity between implicit HDF5 zero initialization and a CF NaN fill marker.

Do not resolve this by silently interpreting missing values as zero or NaN.

Investigate the upstream reader's semantics separately if needed, but do not expand this milestone into a NISAR implementation.

## Upstream boundary

The selective v1 index patch remains unpublished.

Do not submit an upstream issue or pull request without explicit approval.

Keep any changes to hdf5-reader reviewable and preserve full traversal behavior.

## Decision gate

Deliver:

1. The actual SWOT product/profile inspected.
2. Whether bounded chunked-coordinate reads work correctly.
3. Whether native remote range access is feasible with legitimate authentication.
4. Exact agreement with independent h5py/netCDF4 references.
5. Total bytes retrieved versus original product size.
6. Performance and memory against mature controls.
7. Remaining SWOT scientific limitations.
8. Whether the adapter generalizes naturally to other NetCDF-4 products.
9. The next recommended format/product milestone.

Commit verified implementation and supporting reports locally.

Do not merge, publish, release, deploy, or modify ChronoZarr.

Do not implement modern HDF5 index families, arbitrary multidimensional geolocation, general reprojection or a new file-output system.

**Success means Tile-Ripper can retrieve scientifically correct SWOT Raster WSE subsets and their matching chunked coordinates through the existing bounded native stream, with a transparent and verifiable acquisition path.**

If remote archive access remains incompatible, complete a scientifically validated local/mirror demonstration and stop with the access blocker explicitly documented rather than claiming a successful live remote acquisition.