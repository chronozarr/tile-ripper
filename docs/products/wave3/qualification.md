# Wave 3 scientific product decision gate

Required GOES/SMAP success and conditional NISAR diagonal success are achieved on local `codex/acquisition-planner`, starting at `6500522`. Exact evidence is specimen-, processing- and access-route-specific. No merge, push, tag, release, publication, deployment, upstream submission or ChronoZarr change belongs to this milestone.

| Qualified family/profile | Exact real specimen | Native representation and access |
| --- | --- | --- |
| GOES-19 ABI CMIPC C13 | `OR_ABI-L2-CMIPC-M6C13_G19_s20262971601182_e20262971603566_c20262971604045.nc` | Int16 CMI brightness-temperature codes, Int8 DQF, original packed Int16 scan axes; 1500 × 2500; anonymous NOAA conditional ranges. |
| GOES-19 ABI CMIPC C01 | `OR_ABI-L2-CMIPC-M6C01_G19_s20262971601182_e20262971603555_c20262971603593.nc` | Independently inspected reflectance-factor codes; 3000 × 5000; different stripe chunks and packing; same separately verified anonymous route. |
| SMAP SPL3SMP_E v006/R19240 global AM and PM | `SMAP_L3_SM_P_E_20261023_R19240_001.h5` (`C2938664763-NSIDC_CPRD`, `G4387462882-NSIDC_CPRD`) | Float32 soil moisture/freeze-thaw plus UInt16 retrieval/surface flags; original global EASE2 M09 grid, EPSG:6933; exact NSIDC Earthdata authentication. AM/PM stay separate. |
| Additional SMAP daily specimen | `SMAP_L3_SM_P_E_20261022_R19240_001.h5` | Soil moisture/QA, source coordinate and mask comparisons through another sealed bounded capture. This is mirror/profile generalization evidence, not another live-native qualification. |
| NISAR provisional GCOV frequency-A HHHH | `NISAR_L2_PR_GCOV_033_148_A_020_4005_DHDH_A_20261022T105546_20261022T105620_P05023_N_F_J_001` (`C2854338529-ASF`, `G4386922318-ASF`) | Float32 RTC gamma0 linear power; original Float64 UTM16N centers, 10 m; exact NISAR ASF Earthdata route. ProductVersion 1.0.11, specification 1.5.0, CRID P05023. Native-index selection only. |

The [GOES](goes.md), [SMAP](smap.md) and [NISAR](nisar.md) reports contain runnable CLI/Rust commands, full source metadata and admission limits. `tile-ripper products` exposes the five array descriptors alongside existing COG profiles. The normal discover/plan/stream/fetch commands route these profiles before physical preparation. The public Rust adapter is `products::arrays`; the older COG profile's UTM assumptions are not imposed on array coordinates.

## Reuse and physical gates

The existing engine already supplied bounded conditional HTTP, source-version binding, rank-two numeric HDF5, legacy v1 index traversal, rank-one coordinates, unsigned quality words, Float32, streaming and atomic Zarr publication. Wave 3 adds original Int8/Int16 science delivery, concrete coordinate-model distinctions, ordinary array-product workflows and new exact archive policies. The existing durable Zarr store, array encoders, deferred device flush and atomic no-replace transaction are reused.

The pinned hdf5-reader 0.9.3 remains the sole HDF5 parser. Its source contains legacy/v2 B-trees and modern fixed/extensible indexes, plus DEFLATE, shuffle, Fletcher32, N-bit, scale-offset and optional LZ4 mechanisms. Tile-Ripper deliberately continues admitting only legacy v1 chunk indexes with at most three DEFLATE/shuffle/Fletcher32 filters. No modern-index/filter widening or system libhdf5 requirement was introduced.

Real sources use v1 B-trees. GOES uses row stripes: C13 CMI 52 × 2500/DQF 104 × 2500, C01 CMI 26 × 5000/DQF 52 × 5000, with shuffle/DEFLATE. SMAP uses 1 × 3856 rows and DEFLATE level 2. NISAR uses 512 × 512 and shuffle/DEFLATE level 1. Native science types now include Int8, Int16, UInt8/16/32 and Float32; Float64 coordinates/scalars do not imply Float64 science-array admission. Native filters, raw signed-zero/NaN bits and integer flags remain typed.

## Scientific and output contract

GOES navigation uses actual packed scan angles, satellite perspective height, original ellipsoid/origin and sweep-x convention. Inverse navigation is independently checked with PROJ, including space and different source grids/bands. Scan angles are never relabeled metres. SMAP uses exact NSIDC EASE2 M09 formulas and original point registration, independently checked against source coordinates and PROJ; the guide's rounded spacing/registration discrepancy is documented. NISAR preserves original UTM centers and projection scalar; it provides verified inverse navigation for selected native centers without admitting large-grid geographic AOI translation.

Raw native science/quality values and types, acquisition/processing identity, calibration, units and original metadata are retained. Source missingness, AOI membership, Earth visibility and named quality masks are distinct. Native selection includes every requested index in its AOI mask, including GOES space; geographic selection excludes invisible centers. Destination coordinate references describe actual output arrays, while original source attributes remain in provenance. No implicit reprojection, resampling, averaging, mosaicking, radiometric conversion, unsigned reinterpretation or calibration is performed.

`goes_dqf_good` accepts raw DQF exactly 0. `smap_retrieval_recommended` accepts original retrieval words exactly 0 or 8, preserving freeze/thaw bit identity and raw surface conditions. Neither policy silently changes science values or joins validity across observations. NISAR applies no extra radar-quality filter and performs no dB conversion or second calibration.

The shared fill correction removes the reader's CF `_FillValue` fallback: HDF5 initialization and scientific missingness are independent. Default storage zero remains valid unless a scientific marker makes it missing. Explicit storage fills are retained. The HDF5 fill-write-time code table is corrected to allocation = 0 / never = 1 / if-set = 2. Undefined and never-initialized storage are rejected. Independent h5py/NetCDF-C fixtures verify allocated zero/NaN and unallocated default 0/explicit 7 in both endian orders. The real NISAR specimen has all 4,760 chunks allocated; no unallocated synthetic behavior is attributed to it.

## Independent acceptance

- [GOES native and streaming equality](goes-validation.json), [live NOAA](goes-live-validation.json), [physical C13](goes-c13-inspection.json) and [C01](goes-c01-inspection.json).
- [SMAP native and streaming equality](smap-validation.json), [live native NSIDC](smap-live-validation.json), [second-date evidence](smap-additional-validation.json) and [source inspection](smap-inspection.json).
- [NISAR native and streaming equality](nisar-validation.json), [live native ASF](nisar-live-validation.json), [standalone prototype](nisar-prototype.json), [source inspection](nisar-inspection.json) and [actual calibration/processing definitions](nisar-metadata-inspection.json).
- [Storage-fill reference](fill-reference.json), [Rust example/human-plan executions](rust-examples.json), [21 lifecycle/resource/output controls](faults.json), [reference credential-policy controls](reference-policy-tests.json) and [final acceptance/build receipt](acceptance.json).

Every selected raw numeric/quality value and derived mask is checked independently, then the serialized native stream is compared with those verified arrays. Zarr-Python and xarray reopen complete outputs with CF decoding disabled for original types/bits. GOES additionally matches separately opened NetCDF-C; the NISAR sparse reference does not open with NetCDF-C, so that real-product claim is deliberately absent. The initialization fixtures separately match NetCDF-C.

The source captures are bounded original extents, not whole-file downloads or whole-object hashes. Sparse backing files are incomplete, and guarded replay refuses uncaptured holes. Canonical source, strong ETag, object size, extent SHA256 and capture costs are retained. Live-native access, local-mirror correctness and performance are reported separately. Protected constructors retain URS-only credentials, origin-confined cookies and exact-resource redirects; no route inherits another archive's grants.

## Reproduce and review

Build the native engine and examples:

```sh
cargo build --locked --release --no-default-features --features native-cog,native-hdf5,authenticated-http --bin tile-ripper --example array_product --example array_faults
```

The independently pinned reference environment and capture/replay commands are:

```sh
uv run --no-project --python 3.12 --with-requirements docs/products/wave3/reference.txt python bench/wave3_arrays.py goes --capture --live
uv run --no-project --python 3.12 --with-requirements docs/products/wave3/reference.txt python bench/wave3_capture.py examples/product-smap.json --netrc PRIVATE_FILE --directory prototype-output/wave3/reproduced-smap
uv run --no-project --python 3.12 --with-requirements docs/products/wave3/reference.txt python bench/wave3_capture.py examples/product-nisar.json --netrc PRIVATE_FILE --directory prototype-output/wave3/reproduced-nisar
uv run --no-project --python 3.12 --with-requirements docs/products/wave3/reference.txt python bench/wave3_fill.py
uv run --no-project --python 3.12 --with-requirements docs/products/wave3/reference.txt python bench/wave3_acceptance.py faults
uv run --no-project --python 3.12 --with-requirements docs/products/wave3/reference.txt python bench/wave3_acceptance.py benchmark
```

Protected capture accepts only the caller's explicit private netrc, sends credentials only to URS, keeps signed resources in memory and enforces an 8 MiB original-body cap. It checks raw selected values independently before native comparison and seals only after source revalidation. Its native work uses a guarded mirror; it is not a substitute for the separately recorded live-native constructor tests. `--live` and offline summaries use separate files to prevent later replay from overwriting live evidence.

The full original protected mirrors remain at their documented ignored paths; the capture tool can recreate a fresh bounded selection from any canonical example. Reference programs use h5py/NetCDF-C, pyproj and Zarr-Python rather than native decoder output as expected values. Final portability/build commands and log hashes are in acceptance.json. The dgov plan compiled structurally; dispatch lacked a provider configuration, so execution and checks ran directly without changing dispatch or review policy.

## Decision and remaining boundaries

The [performance receipt](performance.json) and [measurement interpretation](performance.md) report discovery, authentication, inspection, source bytes/requests, decoded amplification, geographic selection, first chunk, completion/durable output, CPU/RSS, reservations and output size/count. Tiny correctness AOIs and process-startup costs do not establish throughput leadership.

Clean macOS ARM64 and Linux ARM64 GDAL-free builds/tests are recorded, with no GDAL/libhdf5 linkage. The Linux permission-sensitive controls run as an ordinary user; a root-only attempt was rejected as invalid evidence. Windows and the declared minimum Rust toolchain were not requalified.

The current gates exclude other ABI L1b/L2 layouts, SMAP polar EPSG:6931/L4 and processing releases, NISAR geographic AOI translation/other grids/channels/releases, modern HDF5 index families, unusual filters, complex SAR and point/track/vector products. Existing optical, OPERA, SWOT, COG, Zarr and AlphaEarth contracts remain separate and pass regressions. The [updated 25-family roadmap](../roadmap.md) recommends bounded GOES/polar-SMAP/NISAR grid extensions for Wave 4 before broader generalization.
