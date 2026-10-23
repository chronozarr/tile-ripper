# Tile-Ripper — Geographic AOI to Native SWOT Raster Selection

Continue on `codex/acquisition-planner` from commit `cb69adb`.

The existing workflow supports bounded CMR discovery, authenticated native HDF5 inspection, explicit native-index selection, version-pinned multi-granule jobs, transfer checkpointing, and atomic scientific Zarr v2 publication.

Two real SWOT Raster D PGD0 observations have passed independent h5py, NetCDF-C, Zarr-Python and xarray verification. Three additional granules passed bounded structural inspection, but have not received full scientific qualification.

**Objective: Let a scientist supply a WGS84 geographic AOI and automatically derive scientifically correct native SWOT Raster pixel windows, without reprojection or resampling.**

This should complete the geographic-to-native acquisition workflow.

Do not build a general raster GIS engine or change the core HDF5 reader.

## 1. Audit existing geographic machinery

Inspect the current:

- `swot_discovery` client and `discover-job` bridge.
- Native SWOT x/y coordinate extraction.
- HDF5 preparation and admission.
- Native Sentinel-2 WGS84-to-UTM transformations.
- Existing polygon and rasterization utilities.
- Scientific Zarr writer and mask semantics.

Reuse established `geo`/`proj4rs` components where appropriate.

Identify which CRS, geometry and coordinate utilities are already independently validated and which need new qualification.

Do not assume the Sentinel-2 AOI implementation can simply be copied to SWOT: source grids, coordinate representations and selection semantics differ.

## 2. Define the geographic selection contract

Support explicit WGS84 longitude/latitude input using:

- Bounding boxes.
- Simple GeoJSON Polygons.
- Polygon holes and concave boundaries where supported by the existing geometry library.

For each selected granule:

1. Read its actual native CRS.
2. Read the relevant native x/y coordinate definitions.
3. Transform the geographic AOI into native projected coordinates.
4. Determine which native pixel centers intersect the AOI.
5. Derive an explicit index-space bounding window.
6. Preserve an AOI inclusion mask independently of all scientific quality masks.

Start with the already qualified 100 m Raster D PGD0 grid profile.

Do not infer native pixel indices from CMR granule footprints.

Do not assume x/y coordinates increase in the same direction across every source.

## 3. Handle geometry and pixel boundaries correctly

This is the central scientific gate.

Define and independently verify:

- Longitude/latitude axis order.
- Source CRS and transformation conventions.
- Polygon boundary inclusion.
- Pixel-center selection versus all-touched behavior.
- Exact center-on-boundary cases.
- Holes and concave polygons.
- Partially overlapping AOIs.
- AOIs entirely outside a granule.
- Native axis direction.
- Nonuniform but monotonic stored coordinate arrays, if admitted.
- Floating-point rounding and coordinate tolerance.
- AOIs crossing native grid or chunk boundaries.
- Multiple granules with different native CRS definitions.

Densify geographic edges when necessary so transforming only corner vertices cannot silently exclude native pixels.

Keep transformation tolerance and geometry limits explicit.

For the first implementation, reject ambiguous antimeridian, polar, invalid topology, unsupported CRS and nonlinear grid cases rather than approximating them silently.

No raster reprojection, interpolation or mosaic is permitted.

## 4. Independently validate index translation

Use independent GDAL/PROJ and Python geometry references.

For the existing UTM19T and UTM01W observations, compare:

- Geographic-to-native coordinate transformation.
- Selected row/column bounds.
- Inclusion of individual native pixel centers.
- AOI mask contents.
- Pixel counts.
- Native x/y coordinates.
- Empty and partial intersections.

Construct adversarial fixtures including boundary ties, reversed axes, irregular polygon edges and holes.

Verify that changing destination tile/chunk edge does not change global pixel inclusion.

The reference should rasterize in source-global coordinates or use an independently defined center-in-polygon test, not simply reproduce Tile-Ripper's own window-local rounding.

Preserve every independently observed discrepancy until explained.

## 5. Integrate with discovery-job

Extend the existing selection schema to support either:

A. An explicit native index window, preserving current behavior.

B. A geographic AOI to resolve during authenticated source inspection.

Do not permit ambiguous requests that silently combine conflicting selection methods.

The generated manifest must contain the resolved, explicit native indices, source grid identity and geographic selection provenance.

Preserve the geographic polygon or a stable representation of it for downstream AOI-mask generation.

Planning must show, per granule:

- Native CRS.
- Grid dimensions and direction.
- Selected index window.
- Geographic AOI intersection.
- Count of included native centers.
- AOI coverage information.
- Estimated decoded/source-chunk requirements.
- Estimated working-memory budget.
- Scientific compatibility status.

Do not claim pixels contain usable WSE merely because their centers intersect the polygon.

A candidate that intersects geographically but has zero included native centers should be reported explicitly, not silently substituted with another granule.

## 6. Preserve science and mask semantics

The existing output contains five raw science variables, five source-validity masks and a derived quality mask.

For geographic selections, add a distinct `aoi_mask` identifying native pixel centers included by the geographic request.

Do not modify raw WSE or auxiliary arrays to enforce that AOI mask.

Keep separate:

- Source missingness.
- AOI inclusion.
- SWOT summary/bitwise quality acceptance.

Document the composition needed for downstream analysis, such as:

    analysis_mask = aoi_mask & source_valid_wse & quality_mask

Only use this composite for reported accepted-AOI statistics when requested.

Retain raw integer flag bits, Float32 WSE and uncertainty bits, native projected coordinates, CRS and original provenance.

An AOI can contain zero quality-accepted WSE pixels despite including native raster centers. This must be a valid, documented result.

## 7. Preserve source-version and checkpoint correctness

The resolved native selection and geographic AOI/mask policy are part of the scientific acquisition identity.

A changed polygon or selection must not silently reuse an incompatible scientific job or checkpoint.

Reuse the existing exact-object Earthdata authentication, strong ETag checks, source-byte checkpointing, fresh scientific stream and successful-finish contract.

Do not add arbitrary URL authentication or broaden the existing trusted redirect policy.

Keep multiple native-grid observations separate.

Preserve all-or-nothing scientific Zarr publication.

## 8. Build an end-to-end demonstration

Demonstrate:

1. Discover Raster D granules from geographic and temporal criteria.
2. Select up to four candidates explicitly.
3. Supply a geographic AOI rather than manually calculated row/column indices.
4. Inspect each actual native source and derive its window.
5. Review the resulting index selections, masks and resource estimates.
6. Generate the existing version-pinned manifest.
7. Run authenticated, checkpointed acquisition.
8. Publish a complete scientific Zarr v2 dataset.
9. Open it independently in xarray.
10. Compare the final pixels, masks and coordinates against GDAL/PROJ and the existing h5py/NetCDF-C references.

Use the two already qualified granules as controls, and at least one additional bounded candidate where the available independent reference data permits.

Do not expand the scientific product profile merely to force a third source to pass.

Report separately:

- Granule footprint intersects the AOI.
- Native pixel centers intersect the AOI.
- Native profile passes.
- Scientific acquisition passes.
- Quality-accepted WSE exists.

These are distinct outcomes.

## 9. Profile the geographic-selection cost

Measure:

- CMR discovery time.
- Authenticated metadata and coordinate inspection time.
- Coordinate transformation and AOI-mask construction time.
- Native chunk selection and encoded bytes required.
- Actual HTTP requests and source bytes.
- Time to first native chunk.
- Complete acquisition and durable-output time.
- Working reservations, RSS and checkpoint usage.

The current two-source native inspection takes about 24 seconds in a live observation. Determine whether AOI resolution adds meaningful overhead.

Avoid expensive science-payload downloads during mere candidate discovery.

Do not undertake broad performance optimization unless a new bottleneck is demonstrated.

## 10. Maintain regressions

Repeat relevant:

- Native HDF5 and SWOT science/fault gates.
- Geographic selection oracles.
- Earthdata authentication and secret-confinement tests.
- Checkpoint/replay tests.
- Multi-granule publication tests.
- COG and Zarr v2/v3 regressions.
- Rust feature-matrix and clean GDAL-free builds.

Use frozen metadata and bounded mirrors for repeated testing. Limit live NASA access.

## Decision gate

Deliver:

1. Supported geographic AOI and CRS contract.
2. Correct WGS84-to-native-index implementation.
3. Independently verified pixel-center inclusion and AOI masks.
4. A runnable AOI-based discover-job/fetch workflow.
5. Real multi-granule scientific Zarr output.
6. Source/quality/AOI mask separation.
7. Authentication and checkpoint correctness.
8. Network, CPU and memory measurements.
9. Remaining geographic/product limitations.
10. Recommendation for the next general-purpose Tile-Ripper milestone.

Commit validated work locally.

Do not merge, push, publish, tag, deploy or modify ChronoZarr.

Do not add general reprojection, mosaicking, automatic cross-granule alignment, NISAR support, arbitrary HDF5 formats or new authentication methods.

**Success means a scientist can supply a real geographic area and date range, discover supported SWOT Raster observations, resolve the exact native pixels from inspected source coordinates, and produce a scientifically correct, independently readable dataset without manually calculating array indices.**

Implement and validate the working functionality; do not stop at a design document.