# Tile-Ripper — Major Remote-Sensing Product Expansion, Wave 1

Continue on `codex/acquisition-planner` from committed `629538c`.

Tile-Ripper already has a scientifically validated, bounded, GDAL-free acquisition engine supporting:

- Native COG retrieval and geographic selection.
- Remote Zarr v2/v3 and sharded arrays.
- Restricted NetCDF-4/HDF5.
- Authenticated NASA Earthdata acquisition.
- STAC and CMR discovery.
- Quality-aware SWOT Raster D workflows.
- Native scientific chunk streaming, checkpointing and portable Zarr output.
- Explicit source-version validation and provenance.

We have spent considerable effort validating the underlying engine.

**The next objective is broad remote-sensing product coverage. Stop optimizing existing SWOT and transport internals unless an actual product exposes a necessary defect.**

## Mission

Develop reusable product-profile support for major Earth-observation datasets, starting with:

1. Landsat 8/9 Collection 2 Level-2 Surface Reflectance.
2. NASA HLS v2.0 L30 and S30.
3. NASA OPERA DSWx-HLS.
4. NASA OPERA RTC-S1, as the first Float32 radar acquisition profile.

The goal is to reuse existing readers, transport, discovery and output—not create separate downloaders for each mission.

Work through the families sequentially. Complete and commit validated families even if a later product exposes a blocker.

## 1. Inspect actual products and access paths

Before implementing adapters, inspect real current-version assets and authoritative specifications for every target.

Determine:

- Available catalog and collection identifiers.
- STAC/CMR discovery behavior.
- Public HTTPS, authenticated HTTP and S3 access methods.
- Cloud-optimized TIFF layout and compression.
- TIFF sample types and band organization.
- Native CRS, grid and resolution.
- Scale/offset, fill values and valid ranges.
- Scientific quality bands and bit definitions.
- Granule and acquisition-time identity.
- Source-version metadata and conditional-range behavior.

Do not infer supported formats from product names.

Do not assume NASA Earthdata, USGS STAC and ASF use identical authentication or URL-signing mechanisms.

USGS Landsat cloud S3 access may involve requester-pays behavior. Do not incur paid requests or silently configure requester-pays access. Find an approved, bounded, no-cost route for testing or report the access limitation.

Pin exact reference products, object identities, versions and documentation.

Use bounded source-range acquisition, frozen manifests and local mirrors for repeated tests.

## 2. Design a small reusable product-profile layer

Add a maintainable mechanism that describes the scientific identity and acquisition requirements of a supported product.

A product profile should specify or resolve:

- Mission, instrument, product family and processing version.
- Discovery provider and collection identity.
- Expected scientific assets and their roles.
- Allowed source layouts/codecs/types.
- Native grid and coordinate conventions.
- Band aliases and descriptions.
- Scale, offset and nodata behavior.
- Quality-band definitions and available policies.
- Expected provenance and temporal metadata.
- Access requirements.
- Supported output and streaming capabilities.

Separate descriptive metadata from executable capability.

A product profile must never bypass physical source inspection or declare an unsupported decoder/codec safe.

Reuse the existing planner, readers, scheduler, streaming contract and scientific writer.

Avoid a large plugin framework, arbitrary configuration language or duplicated format-specific execution engine.

Use declarative profiles where practical, with small typed extensions only where scientific behavior requires them.

## 3. Implement Landsat Collection 2 Level-2 SR

Begin with Landsat 8/9.

Support a bounded workflow taking:

- Geographic AOI.
- Acquisition date range.
- Surface-reflectance bands.
- Optional quality-band selection.
- Destination scientific Zarr store or native stream.

Inspect and qualify real Collection 2 COGs.

Preserve the native stored UInt16 reflectance codes.

For Collection 2 Level-2 SR, authoritative USGS documentation specifies the reflectance conversion:

    reflectance = stored_value * 0.0000275 - 0.2

Preserve the exact original codes and calibration metadata. Do not automatically convert stored values or confuse valid numeric range with fill values.

Support the actual applicable `QA_PIXEL`, `QA_RADSAT` and relevant aerosol-quality semantics after inspecting the product's specifications.

Keep raw bit patterns intact and allow an explicit derived quality mask.

Validate Landsat-specific band mapping and processing identifiers. Do not silently assume Sentinel-2 and Landsat bands with similar descriptions have identical spectral response functions.

If Landsat 4–7 have materially different semantics, document them for a later profile rather than assuming the Landsat 8/9 adapter covers them.

## 4. Implement HLS v2.0 L30 and S30

Support both Harmonized Landsat Sentinel-2 product families.

Inspect their actual COG assets, Fmask quality layer, calibration attributes and collection metadata.

Implement geographic/date/band discovery and native-grid acquisition through supported archive access.

Preserve:

- Original scientific stored values and dtype.
- Distinct L30/S30 source identity.
- Harmonized band roles.
- Per-product spectral differences.
- QA bitfields and valid-data definitions.
- Source calibration.
- 30 m native grid information.
- Acquisition timestamps and processing provenance.

Do not assume all HLS layers are UInt16 or have the same nodata representation until verified against real files.

Do not resample or composite L30/S30 observations implicitly.

The fact that HLS is harmonized does not license Tile-Ripper to treat every spectral band as scientifically interchangeable.

Produce independent L30 and S30 observation groups.

If Earthdata-authenticated COG access needs a narrowly different transport policy from the already qualified SWOT HDF5 policy, investigate and implement it explicitly with the same credential-confinement and source-version requirements.

Do not broaden trusted redirect hosts or leak credentials to make the test pass.

## 5. Implement OPERA DSWx-HLS

This is an important hydrology use case.

Inspect real OPERA DSWx-HLS products and their official product specification.

Prioritize the classified surface-water layer, associated quality information and scientifically meaningful diagnostic layers.

Determine actual stored integer types, compression, class codes, fills and masks.

Extend the native COG scientific-value representation narrowly where necessary, particularly for UInt8 classification rasters.

Preserve exact stored classes.

Do not turn classified labels into generic binary valid/invalid masks.

Provide explicit, named water-class and exclusion policies only where the source product documents the semantics.

Preserve cloud, shadow, land-cover and other diagnostic layers when requested.

Do not automatically declare every water-class pixel equivalent across OPERA optical and radar products.

Use independent source-value and class-mask references.

## 6. Add a scoped Float32 radar capability with OPERA RTC-S1

Inspect real OPERA RTC-S1 COGs and the official product specification.

Determine the actual backscatter layer type, TIFF compression, nodata representation, raster grid, polarization names and radiometric units.

Extend the native COG reader to support the observed common Float32 profile if the existing underlying TIFF library provides a safe implementation.

Preserve exact Float32 values and NaN bit patterns where practical.

Preserve polarization identity and relevant radiometric calibration/normalization metadata.

Do not silently convert linear backscatter to decibels, or interchange sigma0, gamma0 and beta0.

Keep quality/mask data separate from source values.

Validate against independent GDAL/rasterio reads.

If the required TIFF codec or layout cannot be supported safely within the current native reader, stop this product's implementation at a documented physical blocker. Retain successful earlier products.

Do not rewrite a TIFF decoder or make GDAL mandatory for all product profiles to force radar support.

## 7. Build a reusable scientific-output experience

Use the existing portable Zarr writer.

For every supported product, preserve:

- Original native values and dtype.
- Original native grid, coordinates and CRS.
- Source missingness.
- Geographic AOI inclusion.
- Derived science/quality acceptance.
- Calibration and physical units.
- Product/version/band identity.
- Acquisition timestamps.
- Provenance and source version.
- Exact applied quality-policy definition.

Do not conflate AOI inclusion, nodata, cloud quality, radar validity or classified water detection.

Store heterogeneous native-grid observations separately.

No implicit reprojection, resampling, mosaicking, compositing, band harmonization or index calculation.

Continue using bounded incremental writing, final source validation and safe publication.

## 8. Validate real scientific workflows

For each admitted product family, require at least one actual source acquisition, not only synthetic fixtures.

Where feasible, include multiple bands/variables and dates.

Create independent GDAL/rasterio or other mature-reader scientific references.

Compare:

- Exact native stored bits.
- Dtype, shape and grid.
- Spatial coordinates and AOI mask.
- Nodata and validity masks.
- Quality/classification values and interpreted policies.
- Source scale/offset and metadata.
- Time, product/version identity and provenance.

Reopen completed Zarr output independently with Zarr-Python/xarray.

Exercise boundary windows, partial source coverage, missing assets, invalid QA definitions, source mutation, corrupt data, cancellation, resource limits and interrupted output.

Retain the existing COG, Zarr, HDF5, SWOT, authentication, streaming and durable-output regressions.

Use narrow explicit compatibility gates instead of relaxing existing source format contracts.

## 9. Benchmark complete acquisitions

Benchmark each added family against appropriate established workflows.

Measure:

- Discovery and source inspection.
- HTTP requests and encoded source bytes.
- Source-block overfetch and duplication.
- Time to first chunk.
- Total completion.
- Output-writing time.
- CPU and RSS.
- Declared working reservations.
- Output object counts and storage.
- Exact scientific correctness.

Compare equivalent source selections and scientific output work.

Do not promise superior performance to GDAL, rasterio or established archive clients.

Do not spend this milestone optimizing small percentage differences unless a new product cannot operate efficiently without a narrow fix.

## 10. Improve user-facing product selection

A scientist should be able to request a supported product by its recognizable family name, rather than knowing which internal COG/HDF5 reader or catalog collection it uses.

Develop an intuitive documented interface, such as:

    tile-ripper products
    tile-ripper discover --product landsat-c2-l2-sr ...
    tile-ripper discover --product hls-l30 ...
    tile-ripper fetch request.json ...

These are illustrative. Fit them to existing CLI conventions and avoid breaking the current command structure.

Expose exact supported variables, product limits, backend requirements, quality policies and output types.

When a profile is unsupported, explain which specific source feature or operation failed.

Make native Rust the intended path for admitted product profiles, with GDAL remaining optional compatibility rather than an implicit fallback.

Do not change the global Cargo default in this milestone without a separate compatibility decision.

## 11. Build the broader mission-coverage roadmap

After implementing the first wave, inspect the larger remote-sensing ecosystem and create a prioritized coverage matrix.

Include, at minimum:

- Landsat 4–9 and historical products.
- Sentinel-2 and Sentinel-1 GRD/RTC.
- OPERA DSWx-HLS, DSWx-S1, RTC-S1 and DIST products.
- GOES ABI.
- SMAP.
- MODIS and VIIRS.
- Sentinel-3.
- ICESat-2 ATL03/ATL06/ATL08.
- GEDI L2A/L2B.
- SWOT Raster and other SWOT families.
- NISAR GCOV.
- Relevant ERA5/GFS cloud Zarr products.

For each, classify:

A. Supported and scientifically validated.

B. Likely supportable by an existing physical reader with a product profile.

C. Requires a contained reader/type/coordinate extension.

D. Requires a fundamentally different observation model or physical format.

E. Blocked by access, dependency, metadata or scientific ambiguity.

Distinguish gridded rasters, swaths, point/along-track data and multidimensional arrays.

Prioritize by scientific usefulness, public accessibility, expected users, implementation effort, portability and reuse of the existing engine.

Do not inflate the supported-product count by listing names that have not been tested.

## Execution strategy

Use small staged commits:

1. Real-product inspection and minimal profile foundation.
2. Landsat Collection 2.
3. HLS L30/S30.
4. OPERA DSWx-HLS.
5. OPERA RTC-S1 Float32 experiment and integration if qualified.
6. Final cross-product acceptance and roadmap.

You may delegate independent scientific references, product metadata audits, or decoder-profile investigations to separate agents.

The primary agent owns integration and verifies consequential scientific claims.

Continue implementing even if one product is blocked; preserve the blocker and proceed to independent product families where safe.

## Decision gate

Deliver:

1. Exact newly validated product families.
2. A reusable, maintainable product-profile mechanism.
3. Runnable discovery, plan, fetch and stream workflows.
4. Independent real-source scientific validations.
5. Native data type and codec coverage.
6. Scientific metadata and quality-policy correctness.
7. Authentication and archive-access boundaries.
8. End-to-end performance and resource measurements.
9. Updated installation and product-support documentation.
10. A prioritized roadmap to approximately 15–25 meaningful product families.

Commit verified changes locally.

Do not merge, push, tag, publish, deploy or modify ChronoZarr.

Do not add NISAR complex SAR processing, ICESat-2 photon streaming, HDF4, general reprojection, a distributed scheduler or a new geospatial format engine in this milestone.

Do not revisit licensing or release preparation as part of the engineering work.

**Success means Tile-Ripper moves from a handful of carefully engineered demonstrations to a genuinely useful selection of major optical, water-mapping and radar Earth-observation products—without losing its native-Rust portability, selective acquisition, scientific correctness or transparent provenance.**

Implement the product coverage. Do not stop at a survey or registry design.