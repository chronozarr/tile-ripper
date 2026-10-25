# Product qualification milestones

[Wave 3 qualification](wave3/qualification.md) adds GOES-19 CMIPC C13/C01, SMAP SPL3SMP_E v6 global AM/PM, and one real NISAR provisional frequency-A HHHH diagonal. These are native HDF5 array profiles with explicit coordinate models and independent access/output gates; other mission products and grids remain outside their qualification.

[Wave 2 qualification](wave2/qualification.md) adds separate historical Landsat TM/ETM+ profiles, current OPERA DSWx-S1, and a bounded DIST-ALERT profile. The Wave 1 evidence below remains scoped to its original source specimens and routes. Provider routing and the AlphaEarth prototype retain their separate qualification documents.

# Wave 1 qualification and decision gate

Scope: local `codex/acquisition-planner`, starting at `629538c`, executed on 2026-10-23. No global Cargo default change, push, merge, tag, release, deployment, requester-pays request, or ChronoZarr change is part of this work.

## Admission is per product and access route

| Family | Real scientific qualification | Access boundary |
| --- | --- | --- |
| Landsat 8/9 Collection 2 Level-2 SR | Independent stored-code, grid, calibration, AOI, source-validity and raw/derived-QA comparisons for separate L8/L9 acquisitions, SR_B4/SR_B5/QA_PIXEL/QA_RADSAT/SR_QA_AEROSOL | Planetary Computer STAC and runtime-only signed Azure HTTPS, with conditional source versions. No USGS requester-pays S3. |
| HLS L30 v2.0 | Public GSFC B04/Fmask reference plus current production L30 B04/Fmask authenticated stream and Zarr fetch; mature-reader exact values, masks, coordinates and provenance | Explicit HLS-specific LP DAAC Earthdata COG route; credentials stay at URS. |
| HLS S30 v2.0 | Separate public GSFC B04/Fmask reference plus current production S30 B04/Fmask authenticated stream and Zarr fetch | Same separately qualified HLS transport contract; L30/S30 remain distinct observations. |
| OPERA DSWx-HLS v1.0 and v1.1 | Official public v1.0 WTR/DIAG reference plus current v1.1 WTR/DIAG/SHAD/CLOUD native acquisition and independent value, fill and policy comparisons | Public JPL sample bucket and separately qualified explicit PO.DAAC v1.1 Earthdata route. Catalog version remains 1.0; each source retains its exact processing release. |
| OPERA RTC-S1 v1.0 | Official public reference VV/VH Float32 linear gamma0 power and UInt8 layover/shadow mask; exact raw bits, NaNs, AOI/source-validity and independent finite/unaffected masks | Public JPL sample bucket plus a current Sentinel-1C burst through the separately qualified exact ASF Cumulus Earthdata route. Older datapool URLs are not admitted by that authentication policy. |

These are five profiles and four mission/product families; L30/S30 are counted independently because their source/band semantics differ. Descriptor tables include additional specified variables, but real-source equality claims are limited to the bands, dates, grids and routes named in the receipts. No other mission in the roadmap is admitted by name alone. Public reference specimens are real distributed products, not synthetic stand-ins for production authentication.

## Evidence and reproducibility

- [NASA product/object/metadata inspection](nasa-inspection.json) pins official source URLs, source sizes, strong ETags, byte-range receipts, original acquisition/sensing/processing metadata and documentation.
- [Landsat 8 acceptance and equivalent output measurements](landsat-validation.json) and [Landsat 9 acceptance](landsat9-validation.json).
- [HLS L30 reference acceptance](hls-l30-validation.json), [S30 reference acceptance](hls-s30-validation.json), and [current authenticated production acceptance](hls-auth-validation.json).
- [DSWx-HLS reference acceptance](opera-dswx-hls-validation.json), [current PO.DAAC acceptance](dswx-auth-validation.json), [current DSWx inspection](dswx-current-inspection.json), [RTC-S1 reference acceptance](opera-rtc-s1-validation.json), [current ASF acceptance](rtc-auth-validation.json), and [current RTC inspection](rtc-current-inspection.json).
- [Final build and acceptance checks](acceptance.json).
- [Live bounded discovery](discovery-validation.json) passes separate L30, S30, current DSWx v1.1 and current ASF RTC queries.
- [Scientific definitions and authoritative specifications](specifications.md), [runnable commands and examples](workflow.md), and [prioritized 25-row roadmap](roadmap.md).

`bench/wave1_nasa_probe.py` captures bounded source ranges and independent rasterio references, then verifies original bytes offline. `bench/wave1_products.py` drives the actual CLI through version-pinned HTTP mirrors, reopens output in a separate rasterio/Zarr-Python/xarray process, and compares a complete independently implemented `bench/wave1_reference.py` rasterio→Zarr workflow. Reference masks and calibration are independently transcribed from the specifications. The same approved source windows and native output work are compared; discovery/authorization and source-capture costs are reported separately. No decoder output is used as the reference scientific values.

The guarded sparse mirrors preserve original offsets/layout and reject uncaptured ranges. Live capture is byte/request capped. Raw captures and output stores stay under ignored `prototype-output/wave1`; receipts and harnesses are versioned. Receipt hashes identify captured ranges and the exact raw native scientific values. Fresh output paths are required; completed stores are independently reopened with CF decoding disabled to inspect original dtype/bits.

## Reader and scientific-output changes

The existing upstream `tiff-reader` owns TIFF parsing and codecs. Native support now includes tiled single-band UInt8, UInt16, Int16 and Float32, with uncompressed/LZW/Deflate and integer predictors 1/2 or Float32 predictors 1/3. UInt8 palette storage preserves the class indices and requires exactly 256 color entries. No color conversion, new TIFF decoder, GDAL dependency, reprojection, resampling or derived science-value conversion is introduced. Alpha, explicit internal masks/SubIFDs, unsupported sample layouts/codecs, nonrectilinear product grids and unsupported CRS representations remain failures.

Real products required narrowly tested metadata extensions: Landsat PixelIsPoint is normalized to GDAL-compatible corner affines; HLS's inert band DESCRIPTION role is retained; the exact OPERA DSWx 1.0 metadata/GeoKey representation permits a specification-backed UTM normalization. The latter requires matching OPERA product identity, exact projection code, WGS84 ellipsoid and units; it is not a generic datum inference from an ellipsoid. Original GeoKeys, tiepoints, pixel model, metadata XML, nodata text and the normalization explanation remain in output provenance, with a 64 KiB per-source bound and aggregate prepared-memory admission.

Raw values remain unchanged on included source pixels. `aoi_mask` records inclusion and `valid` combines inclusion with physical source missingness and documented product fills. Landsat QA fill bit 0 remains missing even when an asset omits its TIFF nodata tag; the original tag absence and raw QA word are preserved. Named `quality_POLICY` arrays retain their exact policy definition and belong only to their required raw QA/class/radar observation. No cross-band QA join is implicit. The independent complete-output check caught and prevents omission of all-zero valid QA chunks when no source nodata is defined.

For Float32, every source NaN is invalid while its payload bits remain unchanged. A textual `GDAL_NODATA=nan` is retained in physical provenance and normalized out of the finite `Layout.nodata` field so JSON and equality remain well-defined. Float32 Zarr uses a NaN storage fill. With no finite source sentinel, original outside-AOI samples remain stored and the AOI mask excludes them; finite-sentinel sources fill excluded envelope centers with that source sentinel.

## Authentication and source versions

`AuthenticatedHttp::earthdata_hls`, `earthdata_dswx` and `earthdata_rtc` are separate from the existing SWOT constructor. They accept only their own unsigned exact-object LP DAAC HLSL30.020/HLSS30.020, PO.DAAC DSWx-HLS v1.1, or ASF Cumulus RTC-S1 v1.0 COG locators and the separately observed directed archive→URS→archive→exact CloudFront resource paths. `products::prepare_earthdata_authenticated` selects the constructor by profile. No policy inherits another archive’s hosts. Basic credentials go only to URS; cookies are isolated by exact origin; resource requests receive neither credentials nor login cookies. Every COG asset has an exact session-map entry. New hosts, path layouts, collection releases, unmatched/missing/extra sessions and source-version conflicts fail closed.

Authentication reservations are aggregate per request and charged alongside prepared metadata and active native reader reservations. Source reads remain conditional on strong ETags; preparation, pre-execution and post-execution checks retain object size/version guarantees and terminal failure invalidation. Stream chunks are provisional until completion, and scientific output is published only after final validation. This extends COG transport only because actual HLS and OPERA products required it; SWOT authorization remains separate. The three archive policies are deployment-specific and deliberately fail on unqualified host/path changes.

## Performance interpretation and remaining limits

The JSON receipts report discovery/inspection, source HTTP requests and encoded bytes, unique source/block work, first chunk, completion, output-writing time, CPU, RSS, conservative reader/transport/writer reservations, output object counts/size, and exact scientific comparisons. Loopback replay measures execution/output costs; initial capture reports actual remote source bytes. The independent rasterio process includes its Python/library startup. These small AOIs are correctness and resource gates, not representative throughput benchmarks or evidence of general superiority to GDAL/rasterio.

No implicit mosaics, composites, observation alignment, index calculation, bandpass substitution, dB conversion or sigma0/gamma0/beta0 interchange occurs. Current product admission is north-up 30 m WGS84 UTM only. Polar Landsat/RTC grids, HLS thermal/angle held-out specimens, every DSWx diagnostic layer, other RTC polarizations, and long-running authentication refresh on these new COG products need additional evidence. The qualified production OPERA routes are the exact current PO.DAAC v1.1 and ASF Cumulus v1.0 patterns in the receipts; older ASF datapool routes remain outside that auth policy. Existing narrow Zarr/HDF5/SWOT contracts remain independently tested.

## Recorded acceptance checks

The integrated implementation passed `cargo test --locked --all-features --all-targets` (174 tests, 9 explicitly ignored) and the GDAL-free matrix `cargo test --locked --no-default-features --features native-cog,native-hdf5,authenticated-http` (145 tests, 8 explicitly ignored). `cargo clippy --locked --all-features --all-targets -- -D warnings`, `cargo fmt --all -- --check`, and no-default-feature and authenticated-HTTP-only compilation passed. Existing ignored live-oracle tests were not silently counted as executed; the Wave 1 real-source harnesses are recorded separately above. `otool -L target/release/tile-ripper` confirmed the optimized native executable has no GDAL linkage.

The recorded loopback complete-acquisition measurements below include independent output writing in both workflows. Python process startup is included in the reference elapsed time. RSS includes process/library overhead and is distinct from the engine's declared working reservation. These are small, exact-source correctness selections; their large decoded-block/selected-value ratios reflect unavoidable native tile granularity, not a claim of efficient large-area throughput. Duplicate HTTP response-range bytes were zero in all six native replays.

| Case | Native elapsed (s) | Rasterio→Zarr process (s) | First native chunk (s) | Native GETs / response bytes | Native peak RSS (bytes) | Declared peak reservation (bytes) | Output objects / bytes | Decoded block / selected bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Landsat 8 | 0.082 | 0.294 | 0.031 | 30 / 367,202 | 39,092,224 | 1,147,333 | 121 / 1,424,344 | 52.51 |
| Landsat 9 | 0.082 | 0.280 | 0.031 | 30 / 362,832 | 39,239,680 | 1,142,594 | 122 / 1,425,074 | 52.51 |
| HLS L30 public reference | 0.050 | 0.249 | 0.016 | 12 / 376,757 | 21,118,976 | 777,038 | 53 / 216,018 | 202.27 |
| HLS S30 public reference | 0.052 | 0.236 | 0.018 | 11 / 496,513 | 21,594,112 | 1,025,431 | 53 / 219,357 | 226.77 |
| DSWx-HLS public reference | 0.044 | 0.481 | 0.014 | 10 / 145,313 | 20,135,936 | 1,940,660 | 52 / 141,390 | 453.54 |
| RTC-S1 public reference | 0.116 | 0.332 | 0.034 | 14 / 3,429,353 | 29,442,048 | 9,631,833 | 82 / 248,597 | 453.54 |

[Direct original-provider executions](direct-source-validation.json) additionally passed Landsat 8, Landsat 9, public DSWx and public RTC acquisition against the pinned independent frozen selections. [Current HLS authentication](hls-auth-validation.json) records fresh native production acquisition and separate mature-reader HTTP validation. Remote timing includes provider latency and authentication and is kept separate from loopback execution costs. Writer timing, CPU, resource-scope explanations, source capture costs, exact hashes, coordinates and masks are in the per-case JSON receipts rather than inferred from this summary table.

The current DSWx held-out window includes non-water, open water, partial water, cloud-obstructed and aerosol-reassigned pixels: independent masks accept 3/20 water, 16/20 shadow-free and 5/20 cloud-obstruction-free centers. Current RTC crosses the source-validity edge with 99 AOI centers and 53 valid centers in each of VV, VH and mask. The existing GDAL-compatible scanline boundary ownership remains in effect; the RTC receipt records why an inclusive-coordinate test at an exact pixel-center boundary was an incorrect independent oracle.

The [Wave 4 geographic gate](wave4/qualification.md) supersedes the Wave 3 geographic exclusions only for the inspected NISAR UTM16N 10/20 m HHHH grids, SMAP northern EASE2 N09 AM/PM groups, and GOES-19 CMIPM1 C02. Source-version, scientific-mask, authentication and publication contracts remain separate and required.
