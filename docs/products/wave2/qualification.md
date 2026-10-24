# Wave 2 scientific qualification and decision gate

Executed locally on `codex/acquisition-planner` from `d5ea791`, retaining provider routing `47a708d`, on 2026-10-24. Four new named profiles cover three product families. Product interpretation and exact-object access use the existing native COG planner, typed stream, source-version checks and portable scientific Zarr writer. No acquisition-engine refactor, Cargo dependency/default change, new format, provider substitution, merge, push, release or ChronoZarr change is included.

## Admitted profiles and source scope

| Canonical profile | Independently qualified real source | Variables and access |
| --- | --- | --- |
| `landsat-tm-c2-l2-sr` | `LT04_L2SP_016035_19881109_02_T1` and `LT05_L2SP_016035_20100810_02_T1` | Six UInt16 SR bands B1/2/3/4/5/7, UInt16 QA_PIXEL/QA_RADSAT, UInt8 SR_CLOUD_QA, Int16 atmospheric opacity; current Planetary Computer discovery and signing |
| `landsat-etm-c2-l2-sr` | Post-SLC-off `LE07_L2SP_016035_20040716_02_T1` | Same native layer identities, with a separate ETM+ saturation policy and actual gap-crossing selection; current PC route |
| `opera-dswx-s1` (`opera-dswx-s1-v1`) | `OPERA_L3_DSWx-S1_T17SQV_20261021T230515Z_20261023T070839Z_S1D_30_v1.0`, product 1.0 / SAS 1.2 | Four UInt8 WTR/BWTR/CONF/DIAG assets; exact current PO.DAAC collection and separately admitted Earthdata route |
| `opera-dist-alert` (`opera-dist-alert-v1`, `opera-dist-alert-hls`) | `OPERA_L3_DIST-ALERT-HLS_T17SPU_20261021T160701Z_20261023T063220Z_S2A_30_v1` | Nine VEG/GEN status, confidence, onset/last-date and DATA-MASK layers; exact LP DAAC collection V1 route |

The [historical Landsat evidence](landsat.md), [DSWx-S1 evidence](dswx-s1.md) and [DIST-ALERT evidence](dist-alert.md) contain authoritative definitions, original asset identities, calibration, fill and class domains, source processing/sensing/lineage metadata, independent numerical comparisons, access boundaries and reproduction commands. Full native typed values, grids, axes, source provenance, AOI masks and per-layer validity/derived policy masks are checked. Every final frozen receipt names the exact tested binary hash; [acceptance](acceptance.json) pins the verification artifacts.

All three historical specimens pass ten-layer frozen capture→native fetch/stream→independent rasterio/GDAL/xarray/Zarr checks. Each also passes a five-band direct PC acquisition. The L7 selection contains 5,748 SR_B3 zeros matching QA_PIXEL fill, alongside valid measurements. No gap filling occurs. DSWx-S1 passes four-layer authenticated production fetch and stream; its final frozen gate checks all 100 provisional chunks and complete nonoverlapping coverage. DIST passes nine-layer production fetch and central/edge frozen output, including four DATA-MASK zero codes that remain source-valid and are rejected only by current-observation policies.

Qualification is specimen- and variable-scoped. It does not prove archive-wide scientific equivalence or accuracy of the originating classifier, nor admit every future software release, geographic grid, spectral band or provider mirror. Native output keeps each asset/observation in its own grid group; no forced cube alignment occurs.

## Science and identity boundaries

TM/ETM+ red is SR_B3 and NIR is SR_B4; OLI keeps SR_B4/SR_B5. Source EO band identities/wavelength metadata are retained. Raw SR codes use scale 0.0000275 and offset −0.2 without application; LEDAPS opacity uses 0.001. Historical cloud QA has product fill 0 even where TIFF nodata is absent. QA_PIXEL source fill remains bit 0; saturation zero remains valid independently of SR gaps. The historical pixel policy excludes fill/cloud/shadow/snow flags without relying on the documented problematic clear bit 6. Sensor-specific saturation/drop semantics and the documented LEDAPS code domain are separate from OLI aerosol/terrain policies. Thermal processing, harmonization and indices are absent.

DSWx-S1 WTR water policies accept 1 (open water) and 3 (inundated vegetation); HLS class 2 is not substituted. BWTR is evaluated independently. Actual SAS 1.2 DIAG is a UInt8 fuzzy score with fill 255, differing from an older specification's codes and from HLS UInt16 diagnostics. HAND, layover/shadow and ocean codes remain raw classification information, independent of source fill. Current source layers have 22 WTR/BWTR precedence differences; no rewrite conceals them. Physical admission requires product 1.0 and software 1.2, layer identity and original RTC lineage/timestamps.

DIST status, algorithm confidence, observation quality and missingness have distinct roles. Confirmed masks accept status 3/6/7/8; ongoing masks accept 3/6. Confidence remains an Int16 score, not a probability. Native dates retain `days since 2020-12-31`, original -1 fill and the distinct meanings of onset versus last assessed observation. DATA-MASK 0 has no physical fill tag and does not erase accumulated alerts. Exact TIFF descriptions and physical units bind every requested variable: all wrong status/confidence/date layer pairings are rejected in a regression using independently inspected source metadata. A single alert is not a reconstructed history; annual products, recovery and compositing remain unqualified.

New OPERA source admission binds 3660² shape, 30 m north-up UTM, MGRS zone/hemisphere, 100 km tile corner with source grid snapping, and the northing repetition cycle selected using the existing UTM projection. Wrong zone, shape, tile corner and ±2,000 km shifts fail tests. Nominal filename acquisitions, tile-local sensing intervals, filename generation and actual processor times remain separately recorded. DIST uses the actual sensing timestamp; DSWx-S1 uses the tile-local RTC start, which differs from the collection's earliest burst token.

## Ordinary CLI and Rust use

```sh
cargo build --locked --no-default-features --features native-cog,native-hdf5,authenticated-http
target/debug/tile-ripper products
target/debug/tile-ripper discover --product landsat-etm-c2-l2-sr \
  --bbox -78.65,35.95,-78.61,35.99 --start 2004-07-16T00:00:00Z \
  --end 2004-07-16T23:59:59Z --bands red,nir,QA_PIXEL,QA_RADSAT,SR_CLOUD_QA --limit 8
target/debug/tile-ripper plan examples/product-landsat-etm.json --human --gdal-free
target/debug/tile-ripper fetch examples/product-landsat-tm.json --output NEW_TM.zarr --gdal-free
target/debug/tile-ripper fetch docs/products/wave2/dswx-s1-request.json \
  --auth earthdata --netrc /private/earthdata.netrc --output NEW_WATER.zarr --gdal-free
target/debug/tile-ripper stream docs/products/wave2/dist-alert-request.json \
  --auth earthdata --netrc /private/earthdata.netrc --values --gdal-free
cargo run --locked --no-default-features --features native-cog,authenticated-http \
  --example product_stream -- docs/products/wave2/dist-alert-request.json /private/earthdata.netrc
```

The new discovery examples under `examples/product-landsat-{tm,etm}.json` and `examples/product-opera-{dswx-s1,dist-alert}.json` contain AOI/date/product/bands without TIFF layouts or provider storage paths. Pinned OPERA requests reproduce an exact qualified granule. Fresh discovery can return overlapping tiles or different generation stamps as separate observations; it never silently replaces one. Narrow queries or raise the explicit bounded observation limit when the catalog reports overflow. The Rust example uses ordinary product preparation and typed chunks and calls `finish` before claiming validated completion; it was executed against original captured DIST sources locally.

Landsat retains the qualified credential-free PC rule. New OPERA profiles have one qualified discovered production route each, requiring explicitly supplied Earthdata credentials. No new alternate provider was added. Existing HLS coverage comparisons and object/sample-scoped attestations remain unchanged. Authentication uses separate exact collection/layer constructors; credentials go only to URS and temporary signed locators stay in memory. Source size/strong ETag, pre/post validation and atomic output publication retain the existing contract. Streaming chunks are provisional until successful completion.

## Measurements and repeatability

Create an isolated reference environment when reproducing the Python gates:

```sh
uv venv --python 3.10 prototype-output/wave2-reference-env
uv pip install --python prototype-output/wave2-reference-env/bin/python -r bench/wave1-requirements.txt
```

Use that environment's Python for each dedicated harness. Native acquisition itself needs neither Python nor these reference libraries.

[Performance summary](performance.json) consolidates the per-case complete-output process wall/CPU/RSS, source blocks/encoded bytes/overfetch, first-output and writer phases, memory reservations and final object sizes. Native and independently implemented rasterio→Zarr workflows produce identical scientific arrays. Reference startup is included. Metadata serialization, codecs and provenance object sizes differ, so these are scientifically equivalent small-window correctness/resource comparisons, not interchangeable byte-level encodings or a performance-superiority claim. Frozen loopback timings exclude original-provider latency and authorization.

[Timed normal catalog commands](discovery-timing.json) measure all five source cases. [Accountless signing and source HEAD](access-timing.json) isolate one actual PC control-plane sample; its temporary SAS is never persisted. Direct production receipts retain Earthdata authentication seconds, redirect/request counts and full execution phases; auth timers are cumulative session time and can overlap, not a serial component to add to wall time. Native first-output counters begin in execution after preparation, while external stream first-chunk timers begin at process launch; the receipts name these scopes. Selected tile encoded bytes, decoded tile overfetch, metadata-inclusive HTTP bytes and initial capture page bytes are distinct metrics.

Captured original upstream bytes total 6,003,528 (LT04), 5,242,880 (LT05), 7,981,505 (LE07), and 729,426 for DIST; DSWx's four small compressed original files have per-object sizes/hashes in its inspection receipt and an 8 MiB per-object ceiling. Repeated gates use frozen original offsets and reject uncaptured ranges. No repeated science qualification requires a live bulk download. Raw mirrors, intermediate stores and process logs remain ignored under `prototype-output`; receipts retain immutable identities/range hashes and mature-reader equality.

Initial ten-band live Landsat calls encountered HTTP 429 from the PC signer and failed without substitution. After cooldown, direct five-band workflows passed for all three sensors; full ten-band stored-source workflows passed independently. No retry/cache optimization was added to hide that provider constraint. One initial DIST live attempt had a transient transport failure; subsequent nine-layer authenticated discovery/fetch passed. Neither family remains blocked. DIST-ANN remains a separate future qualification.

## Build, preservation and next milestone

The baseline passed 197 all-feature tests (9 explicitly ignored). Final integration passes 204 all-feature tests (9 ignored) and 175 GDAL-free tests (8 ignored), strict all-feature Clippy, formatting, no-default-feature and authenticated-HTTP-only compilation. Six prior Wave 1 frozen scientific workflows pass; their historical receipts were preserved. Existing Zarr, native HDF5/SWOT, AlphaEarth, source-mutation, cancellation and atomic-output regressions remain in the Rust matrix. Per-family frozen suites additionally exercise failure/cancellation/version changes and reject successful publication after faults.

A fresh empty-target native build and the runnable Rust example pass on macOS arm64 with Rust 1.92.0. `otool -L` confirms no GDAL linkage; the native dependency graph contains no GDAL package. Cargo manifests/lock/defaults are unchanged and no new native dependency was added. Independent Python reference tools are optional benchmark dependencies, including rasterio 1.4.4/GDAL 3.10.3, Zarr 2.18.3, xarray 2025.6.1 and NumPy 2.2.6 for the historical/radar gates. DIST's isolated reference environment is recorded in acceptance. Linux/Windows and the declared minimum Rust toolchain were not rerun in Wave 2; earlier platform evidence is not presented as a fresh result.

The [25-family roadmap](../roadmap.md) records actual A–E status, preserving DIST-ANN and track/point-model boundaries. Recommend GOES ABI, then SMAP soil moisture/QA, then a single real-valued NISAR GCOV diagonal as the next bounded qualifications. ICESat-2 and GEDI still require a deliberate beam/segment/shot model. The [AlphaEarth follow-up assessment](../../alphaearth/workflow.md#wave-2-follow-up-assessment) preserves the existing rank-four `(time, band, y, x)` prototype and outlines CLI/AOI/output/year/accounting work without implementing it.
