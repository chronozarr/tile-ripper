# Tile-Ripper — Selective HDF5 Index Traversal and Large-File Validation

Continue on `codex/acquisition-planner` from commit `fe58a2c`.

The new opt-in `native-hdf5` implementation supports restricted rank-two scientific array slices through the existing planner, bounded scheduler and native streaming interface. GOES-19 data have been independently validated against h5py and netCDF4.

The main demonstrated limitation is that `hdf5-reader 0.9.3` traverses the complete legacy v1 chunk B-tree before filtering chunk records. In the GOES-19 control, a one-chunk selection and a full-index selection both required 22 index GETs.

**Objective: Determine whether selective HDF5 index traversal can materially reduce remote metadata I/O, implement the narrowest correct optimization, and demonstrate its value on a larger real scientific file.**

## 1. Audit the v1 B-tree traversal

Inspect the exact `hdf5-reader 0.9.3` source.

Understand:

- How chunk coordinates are encoded in v1 B-tree keys.
- Ordering of internal nodes and leaf records.
- How chunk extents and multidimensional selections map to index keys.
- Which index nodes must be visited to identify all intersecting chunks.
- How unallocated chunks are represented.
- Which validations require reading nodes outside the selected traversal.
- Whether filtering can occur during traversal rather than after collecting the complete index.

Identify any ordering or layout cases where pruning is unsafe.

Do not assume multidimensional spatial bounds correspond to one contiguous B-tree key interval.

## 2. Prototype selective traversal

Where justified by the source format, implement a selection-aware v1 traversal through a small isolated upstream-compatible extension.

Prefer contributing a reusable API to `hdf5-reader` over maintaining a separate HDF5 index parser inside Tile-Ripper.

Keep all existing format validation, source identity checks and malformed-index protections.

An unsupported index type must still fail explicitly.

Do not introduce a full-file download fallback.

## 3. Obtain a meaningful large-file test

The first GOES-19 object is only 1.27 MB. Select a substantially larger, publicly accessible NetCDF-4/HDF5 object whose chunk layout fits the supported profile.

Prefer NOAA, NASA or another authoritative scientific archive with anonymous conditional range access.

If a relevant SWOT or NISAR product is accessible through a legitimate supported route, inspect it. Otherwise use another representative gridded scientific product.

Capture a bounded metadata and payload mirror for repeated benchmarks.

Do not download massive files unnecessarily.

Document the file size, chunk count, index structure, filters and scientific variable selected.

## 4. Compare index strategies

Benchmark:

A. Existing complete-index traversal.

B. Selective traversal, if feasible.

C. The best practical metadata batching/cache strategy.

D. A tuned mature-reader workflow, including whole-file retrieval when that is actually cheaper for small objects.

Use sparse and dense selections, including noncontiguous slices.

Measure:

- HDF5 index nodes visited.
- Metadata GET counts and bytes.
- Payload GET counts and bytes.
- Time spent preparing the plan.
- Time to first chunk.
- Full acquisition completion.
- CPU and peak RSS.
- Declared memory reservations.

Test local, moderate-latency and high-latency conditions.

Keep equivalent scientific selection and validation guarantees wherever possible. Disclose differences for local h5py versus remotely range-backed readers.

Do not assume selective traversal will always beat the full scan.

## 5. Scientific and fault validation

Repeat all existing 64 scientific cases, 33 fault cases and inherited COG/Zarr regressions.

Add independent tests for:

- First, last and intermediate B-tree keys.
- Chunk-boundary selections.
- Sparse and dense multidimensional selections.
- Strided slices.
- Missing or unallocated chunks.
- Different supported chunk shapes.
- Out-of-order physical payload addresses.
- Corrupt child pointers and invalid record counts.
- Truncated metadata.
- Inconsistent object identity.
- Source mutation between index and payload reads.
- Cancellation and small working budgets.

Verify that selective traversal returns exactly the same chunk set, values and coordinates as full traversal and independent h5py/netCDF4.

Do not drop structural validation without explicitly documenting and testing the associated risk.

## 6. Evaluate upstream integration

If a safe optimization is demonstrated, prepare an upstream issue or pull-request-ready patch for `hdf5-reader`.

Document API compatibility, tests, performance and maintenance implications.

Do not publish an upstream patch without approval.

If the optimization requires substantial parser duplication or makes malformed-file safety weaker, preserve the experiment and recommend against adoption.

## 7. Revisit SWOT/NISAR feasibility

Use the findings to reassess specific product layouts:

- SWOT L2 HR Raster WSE and projected coordinate arrays.
- NISAR GCOV projected raster variables.
- GOES and other accessible gridded NetCDF-4 products.

Distinguish actual inspected storage layouts from product-documentation expectations.

Report which products fit the current restricted reader and which require modern HDF5 index families, additional numeric types, multidimensional coordinates, filters or authenticated remote access.

Do not implement a product-specific adapter without first confirming its actual physical layout.

## Decision gate

Deliver:

1. Whether selective v1 B-tree traversal is possible and safe.
2. The before/after metadata request and byte counts.
3. Performance for sparse and dense slices.
4. Independent scientific and fault-test results.
5. Large-file validation with meaningful savings, or a demonstrated reason it is infeasible.
6. An upstream contribution recommendation.
7. The next actual product/format coverage limitation.
8. A recommendation for the next Tile-Ripper milestone.

Commit validated changes and research locally.

Do not merge, tag, publish, deploy, modify ChronoZarr or add a general HDF5 implementation.

**Success means Tile-Ripper can retrieve a small selection from a substantially larger HDF5 dataset without scanning unrelated v1 index branches, while preserving scientific correctness, source validation and bounded memory.**

If the underlying v1 index semantics prevent safe pruning for supported selections, demonstrate that precisely and retain the current bounded full-scan behavior instead of introducing a fragile optimization.