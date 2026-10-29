# OPERA expansion feasibility

Implementation and source/output qualification are now recorded in [OPERA expansion qualification](opera-expansion/qualification.md). The assessment below preserves the research baseline.

Assessment date: **2026-10-27**. This is a research and qualification proposal for the local Tile-Ripper checkout. It adds no product implementation, release claim, or production-access qualification.

**Yes: several missing OPERA products fit Tile-Ripper's acquisition model.** DIST-ALERT-S1 and DIST-ANN-HLS are the best first production-archive additions; DISP-S1 is a worthwhile separate HDF5 milestone; CSLC-S1 requires new numeric and output support. **DSWx-NI COGs already exist as anonymously readable v0.1 engineering samples**, and a pinned prototype profile is feasible without a new physical reader. Its production discovery and final-v1.0 qualification remain held. Other NISAR-derived OPERA products and VLM also remain held. The existing NISAR GCOV profile supplies none of those OPERA products.

## What is already supported here

These are exact local profiles and recorded specimens, not blanket support for an OPERA family. See [profile definitions](../../src/products/profile.rs), [Wave 1 qualification](qualification.md), [DSWx-S1 qualification](wave2/dswx-s1.md), and [DIST-ALERT qualification](wave2/dist-alert.md).

| Profile | Qualified scientific scope | Remaining scope within the family |
| --- | --- | --- |
| `opera-dswx-hls` | Product processing releases 1.0/1.1; WTR, DIAG, SHAD, CLOUD on inspected native grids; public reference and separately recorded PO.DAAC production access | Listed but untested layers are not established by these specimens; no NISAR inference |
| `opera-dswx-s1` | Product 1.0, inspected software 1.2; original WTR, BWTR, CONF, DIAG; UInt8; PO.DAAC production route | Other processing releases and sensor/platform variants need their own evidence; its diagnostic/confidence meanings differ from HLS |
| `opera-dist-alert` | DIST-ALERT-HLS v1; nine UInt8/Int16 status, confidence, onset/last-date and DATA-MASK layers; one accumulated observation at a time | Annual HLS summaries, other HLS layers, and all DIST-S1 are separate admissions |
| `opera-rtc-s1` | RTC-S1 product 1.0; inspected VV/VH Float32 linear gamma0 and UInt8 mask; public reference and separately recorded ASF Cumulus access | HH/HV, polar grids, and RTC-STATIC are not covered by the VV/VH qualification |
| `nisar-gcov-p1-fa-hhhh` | Separate provisional NISAR GCOV frequency-A HHHH; inspected P05023/productVersion 1.0.11, Float32 calibrated linear gamma0 and UTM16N 10/20 m grids | **Not OPERA DSWx-NI, DISP-NI, CSLC-NI, or VLM-NI.** Other covariance terms/frequencies remain separate |

The [Wave 4 gate](wave4/qualification.md) supersedes earlier geographic exclusions only for its named specimens. Neither a declarative profile nor shared storage format qualifies a different scientific product. This assessment does not establish that local acquisition work has been merged or released.

## Current catalog evidence

Anonymous NASA CMR collection queries and one-granule queries were made on 2026-10-27; a returned server Date was `Tue, 27 Oct 2026 15:16:03 GMT`. No ambient credentials or protected raster payloads were used. The DSWx-NI follow-up below also inspected bounded public TIFF header ranges. Collection versions below are **catalog versions**, distinct from filename processing releases.

| Product/variant | Exact collection and catalog version | Archive / evidence | Tile-Ripper decision |
| --- | --- | --- | --- |
| DSWx-HLS | `OPERA_L3_DSWX-HLS_V1`, `1.0`; [C2617126679-POCLOUD](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C2617126679-POCLOUD) | PO.DAAC; catalogued | Existing exact profile |
| DSWx-S1 | `OPERA_L3_DSWX-S1_V1`, `1.0`; [C2949811996-POCLOUD](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C2949811996-POCLOUD) | PO.DAAC; catalogued | Existing exact profile |
| DIST-ALERT-HLS | `OPERA_L3_DIST-ALERT-HLS_V1`, `1`; [C2746980408-LPCLOUD](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C2746980408-LPCLOUD) | LP DAAC; catalogued | Existing nine-layer profile |
| **DIST-ANN-HLS** | `OPERA_L3_DIST-ANN-HLS_V1`, `1`; [C2519119034-LPCLOUD](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C2519119034-LPCLOUD) | LP DAAC; 56,695 granules at snapshot | Missing; good COG candidate |
| **DIST-ALERT-S1** | `OPERA_L3_DIST-ALERT-S1_V1`, `1`; [C4090131664-ASF](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C4090131664-ASF) | **ASF**, 195,724 granules; sampled filename release `v1.0` | Missing; good COG candidate |
| **DISP-S1** | `OPERA_L3_DISP-S1_V1`, `1`; [C3294057315-ASF](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C3294057315-ASF) | ASF; 205,761 granules; `.nc` assets | Missing; dedicated HDF5 profile |
| **CSLC-S1** | `OPERA_L2_CSLC-S1_V1`, `1`; [C2777443834-ASF](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C2777443834-ASF) | ASF; 8,743,102 granules; sampled processing `v1.1`, S1D | Missing; complex science values |
| RTC-S1 | `OPERA_L2_RTC-S1_V1`, `1`; [C2777436413-ASF](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C2777436413-ASF) | ASF; catalogued | Existing exact profile |
| RTC-S1-STATIC | `OPERA_L2_RTC-S1-STATIC_V1`, `1`; [C2795135174-ASF](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C2795135174-ASF) | ASF; 371,083 granules; sampled legacy datapool COG links | Missing companion profile and access route |
| CSLC-S1-STATIC | `OPERA_L2_CSLC-S1-STATIC_V1`, `1`; [C2795135668-ASF](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C2795135668-ASF) | ASF; 44,347 granules; sampled legacy datapool HDF5 link | Missing companion profile and access route |
| DISP-S1-STATIC | `OPERA_L3_DISP-S1-STATIC_V1`, `1`; [C3959290248-ASF](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C3959290248-ASF) | ASF; 1,445 granules; DEM, mask and LOS COG assets | Missing; LOS asset has multiple bands |
| **DSWx-NI** | No matching public production collection found | Public engineering COGs: product v0.1/software 0.3.0; final algorithm v1.0 released Oct 7 | Pinned prototype candidate now; production discovery/final-v1.0 data qualification held |
| DISP-NI / CSLC-NI | No matching OPERA NI/NISAR production collection found in these public CMR searches | Announced/developing; see release conflicts below | Hold exact identifiers, versions and acquisition claims |
| VLM-S1 / VLM-NI | No OPERA VLM collection found | Future products | Hold |

Counts are transient catalog observations, not coverage/completeness guarantees. For example, the newest start-date result sampled for DIST-S1 was `OPERA_L3_DIST-ALERT-S1_T15UWR_20260228T235832Z_20260714T134753Z_S1C_30_v1.0` (`G4246933966-ASF`); discoverability does not demonstrate current global forward processing.

Reproduce bounded evidence with the [OPERA collection search](https://cmr.earthdata.nasa.gov/search/collections.json?keyword=OPERA&page_size=100), [DIST-S1 one-granule query](https://cmr.earthdata.nasa.gov/search/granules.json?collection_concept_id=C4090131664-ASF&page_size=1&sort_key=-start_date), or the same query with another concept ID. Pattern queries used `short_name=OPERA*NI*`, `OPERA*VLM*`, and `OPERA*DIST*`, with `options[short_name][pattern]=true`. The NI query returned only unrelated TROPO-ZENITH. Negative results establish what these public catalog searches found, not the absence of internal/test objects anywhere.

There are material conflicts in the overview material:

- The [JPL schedule table](https://www.jpl.nasa.gov/go/opera/products/) lists June/September 2026 for DSWx-NI/DISP-NI, while the [DSWx suite page](https://www.jpl.nasa.gov/go/opera/products/dswx-product-suite/) still labels NI under development and the [DISP suite page](https://www.jpl.nasa.gov/go/opera/products/disp-product-suite/) says coming soon. An [ASF subject-matter-expert reply dated 2026-08-24](https://forum.earthdata.nasa.gov/viewtopic.php?t=8134) gives February 2027 as the earliest expected DISP-NI production. A schedule date is insufficient admission evidence.
- The [JPL DIST suite page](https://www.jpl.nasa.gov/go/opera/products/dist-product-suite/) still describes all DIST access through LP DAAC and foregrounds prototype S1 samples. The current CMR DIST-S1 record identifies ASF and validated v1 assets. Use the owning collection, not the family paragraph, for routing.
- The schedule gives April 2028 for both VLM variants and an older 100 m/TBD description. The [dedicated VLM page](https://www.jpl.nasa.gov/go/opera/products/vlm-product-suite/) now describes nominal **120 m MGRS tiles**, ASF distribution, CAL/geodetic referencing, and both variants as coming soon. No frozen storage schema should be inferred from either overview.

## Scientific and physical integration requirements

### DSWx-NI: public prototype COGs are available now

The ARSET comparison slide's file-format claim is supported by current primary sources. [DSWX-SAR's final DSWx-NI v1.0 software release](https://github.com/opera-adt/DSWX-SAR/releases/tag/DSWx-NI-v1.0) was published **2026-10-07**, commit `32bf7e1`. NASA's [final acceptance-test instructions](https://github.com/nasa/opera-sds-pge/wiki/DSWx%E2%80%90NI-Final-Acceptance-Test-Instructions), edited October 6, list actual expected WTR/BWTR/CONF/DIAG TIFF outputs with `LSAR_30_v1.0` filenames, derived from August 20 NISAR GCOV inputs. The linked sample ZIP requires JPL VPN/Artifactory credentials; the release has no downloadable sample assets. This is stronger evidence than an anticipated production date, but does not establish a public final-v1.0 archive.

A bounded listing of OPERA's public reference bucket found **actual anonymous v0.1 TIFFs under `transfer/rileykk/dswx_ni/`**, outside its `products/` prefix. The following direct URLs were successfully inspected on 2026-10-27:

| Layer / direct TIFF | Object bytes | ETag |
| --- | ---: | --- |
| [B01_WTR](https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/transfer/rileykk/dswx_ni/job-WF-SCIFLO_L3_DSWx_NI-MS_41_15-10-20260416T152808.731058Z/pge_output_dir/OPERA_L3_DSWx-NI_T42RVP_20260112T004524Z_20260417T094901Z_LSAR_30_v0.1_B01_WTR.tif) | 732,280 | `"4c24ac7234e0d63d71ade7f8269b455d"` |
| [B02_BWTR](https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/transfer/rileykk/dswx_ni/job-WF-SCIFLO_L3_DSWx_NI-MS_41_15-10-20260416T152808.731058Z/pge_output_dir/OPERA_L3_DSWx-NI_T42RVP_20260112T004524Z_20260417T094901Z_LSAR_30_v0.1_B02_BWTR.tif) | 559,556 | `"63c17a56666857346c31dd92df615ffd"` |
| [B03_CONF](https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/transfer/rileykk/dswx_ni/job-WF-SCIFLO_L3_DSWx_NI-MS_41_15-10-20260416T152808.731058Z/pge_output_dir/OPERA_L3_DSWx-NI_T42RVP_20260112T004524Z_20260417T094901Z_LSAR_30_v0.1_B03_CONF.tif) | 3,242,895 | `"5bcf61a8f64146e91a7d50984b4162d7"` |
| [B04_DIAG](https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/transfer/rileykk/dswx_ni/job-WF-SCIFLO_L3_DSWx_NI-MS_41_15-10-20260416T152808.731058Z/pge_output_dir/OPERA_L3_DSWx-NI_T42RVP_20260112T004524Z_20260417T094901Z_LSAR_30_v0.1_B04_DIAG.tif) | 5,731,855 | `"f67e4e123f22f440267381d6bd2ae57d"` |

These are **engineering/transfer outputs**, not a curated validated production collection. Their [job exit marker](https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/transfer/rileykk/dswx_ni/job-WF-SCIFLO_L3_DSWx_NI-MS_41_15-10-20260416T152808.731058Z/_exit_code) is `0`; a different sibling job reports `255` and should not be substituted as the qualifying specimen merely because it also contains TIFFs. The successful marker alone does not validate the science.

All four inspected files are classic TIFF, single-band **UInt8, rank 2, 3660×3660**, with **512×512 tiles, Adobe DEFLATE, horizontal predictor 2, nodata 255**. GeoKeys give **EPSG:32642, WGS84/UTM42N, pixel-is-area**. The actual rasterio affine coefficients are `(30,0,399960,0,-30,2900040)`, equivalent to the GDAL/Tile-Ripper transform `[399960,30,0,2900040,0,-30]`. Retain this source transform rather than reconstructing the grid from a tile label. Four overview levels have shapes 915², 229², 58² and 29², with IFDs preceding imagery. WTR/BWTR/CONF use a 256-entry palette; DIAG is MinIsBlack. Palette entries include white class 0, blue 1 and green 3. Preserve the class indices, not RGB expansion.

A successful metadata-directed pass transferred **57,344 bytes in 14 conditional 206 range GETs**, excluding HEAD and listing/control traffic. Reads were capped at 2 MiB/128 requests per object. The 4 KiB metadata pages include small adjacent compressed-overview bytes; **no full image was acquired and no raster pixels were decoded**. Thus these are actual source-layout findings, not a completed Tile-Ripper acquisition or a claim that particular pixel classes were encountered.

The TIFFs declare `DSWX_PRODUCT_VERSION=0.1`, `SOFTWARE_VERSION=0.3.0`, `PRODUCT_SOURCE=NISAR_GCOV`, `SENSOR=LSAR`, and `SPACECRAFT_NAME=NISAR`. Their separate `PRODUCT_VERSION={'1.0.2'}` annotation must not be mistaken for the DSWx release. Nominal filename acquisition is 2026-01-12 00:45:24Z, while mosaic metadata spans **00:44:50Z–00:45:58Z**. Filename processing is 2026-04-17 09:49:01Z; the TIFF processing field is 09:49:12Z. Preserve each field and its origin; do not force them to match.

The [matching software-0.3 class dictionary](https://github.com/opera-adt/DSWX-SAR/blob/DSWx-NI-v0.3/src/dswx_sar/common/_dswx_sar_util.py) and [NI writer](https://github.com/opera-adt/DSWX-SAR/blob/DSWx-NI-v0.3/src/dswx_sar/nisar/save_mgrs_tiles_ni.py) establish WTR 0=not-water, 1=open-water, 3=inundated vegetation, with masks 250=HAND, 251=layover/shadow, 254=ocean and 255=missing. **This prototype also enables partial-water processing** in its [actual algorithm parameters](https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/transfer/rileykk/dswx_ni/job-WF-SCIFLO_L3_DSWx_NI-MS_41_15-10-20260416T152808.731058Z/AlgoParams.yaml); the matching writer can emit **class 8=partial water**. CONF retains categorical algorithm/wetland codes rather than a common probability scale; DIAG stores a rounded 0–100 fuzzy score plus masking codes. Neither a palette entry nor a class definition proves the inspected windows contain that class.

#### WTR slide reconciliation and policy contract

The user-provided workshop slide **“Dynamic Surface Water Extent (DSWx) WTR Layer”** is presentation evidence, with no public slide URL established here. Its NI column calls class 8 **“water edge pixels”**; the algorithm calls it **“partial water.”** These are compatible descriptions of the classified boundary/mixture category, not instructions to infer edges geometrically or estimate a fractional inundation value. Likewise, the slide's “100% inundated” description of class 1 must remain a classification label, rather than a new pixel-fraction measurement produced by Tile-Ripper.

The [released NI v1.0 class definitions and partial-water routine](https://github.com/opera-adt/DSWX-SAR/blob/DSWx-NI-v1.0/src/dswx_sar/common/_dswx_sar_util.py) still define code 8; the routine assigns it from aggregated open-water samples when the source algorithm's partial-water conditions hold. The [released NI writer](https://github.com/opera-adt/DSWX-SAR/blob/DSWx-NI-v1.0/src/dswx_sar/nisar/save_mgrs_tiles_ni.py) applies this routine conditionally. Activation is **configuration-specific**: the [v1.0 repository defaults](https://github.com/opera-adt/DSWX-SAR/blob/DSWx-NI-v1.0/src/dswx_sar/defaults/algorithm_parameter_ni.yaml) disable it, whereas the [final acceptance algorithm parameters](https://github.com/nasa/opera-sds-pge/blob/main/.ci/scripts/dswx_ni/opera_pge_dswx_ni_r5_final_algorithm_parameters.yaml) enable it, as of this assessment. This verifies a final-code definition and a test configuration, not that every final-v1.0 delivered image contains class 8. Tile-Ripper should preserve the delivered codes without recomputing the source algorithm's thresholds.

| WTR code | Current qualified S1 profile | Inspected NI v0.1 / released NI v1.0 code | Recommended handling |
| --- | --- | --- | --- |
| 0 | Not water | Not water | Source-valid classification; water-selection masks are false |
| 1 | Open water | Open water | Source-valid; explicitly named open-water policy accepts it |
| 3 | Inundated vegetation | Inundated vegetation | Source-valid; keep distinct from open water |
| 8 | Outside the current S1 policy's class domain | Partial water / slide's water-edge category; conditional algorithm stage | Admit only for the separately qualified NI release; retain separately in output |
| 250 | HAND mask | HAND mask | Contextual source class; not fill |
| 251 | Layover/shadow mask | Layover/shadow mask | Contextual source class; not fill |
| 254 | Ocean mask | Defined in source code; omitted from the slide | Contextual source class; not fill |
| 255 | No data | No data | Source fill; source-validity mask is false |

The slide's S1/NI class tables are selective: both omit ocean code 254. Its NI input-source line naming **“Sentinel-1A and C”** conflicts with the inspected TIFF's `PRODUCT_SOURCE=NISAR_GCOV`, `SENSOR=LSAR`, and the final PGE's NISAR GCOV inputs. Treat that line as an apparent presentation-label error (an inference), rather than changing the source identity. The slide does not verify real pixel occurrences or a finalized production layout.

Existing [S1 policy code](../../src/products/profile.rs) already validates WTR against `{0,1,3,250,251,254,255}` and selects water only for `{1,3}`; code 8 is an undefined-class error **when that S1 policy is evaluated**. The S1 BWTR policy selects class 1. Existing [product source validity](../../src/products.rs) excludes the band's nodata 255 and otherwise retains these contextual classes. Raw arrays and the named derived masks are written separately, with AOI membership applied explicitly. Therefore, adding NI must not widen the existing S1 domain or mark 0/250/251/254 as missing.

For a new NI profile, preserve raw WTR and independent source-validity/AOI masks. Define explicit, optional policy names for **open water `{1}`**, **water including inundated vegetation `{1,3}`**, and **water including edge/partial pixels `{1,3,8}`** only for NI releases that define 8. Inundated vegetation and partial-water masks can also be exposed separately. Contextual codes 250/251/254 are false for these water policies but remain stored, source-valid codes. No water policy should be silently applied or substituted for source validity. A later version's new code requires explicit admission rather than inheriting one of these domains automatically.

**Implementation implication:** these TIFF dtypes, palettes, compression, predictor and UTM georeferencing match existing native COG capabilities. Add a separate, explicitly named **DSWx-NI v0.1 prototype** profile restricted initially to pinned observations and this inspected software/source identity. Change the identity parser to admit `DSWx-NI`/`LSAR` and the exact release; bind each selected layer to its object; retain nominal time, mosaic interval, processing times and lineage separately. Keep masks 250/251/254 distinct from source fill 255. Account explicitly for partial-water code 8 when defining any water policy, rather than aliasing the S1 policy.

The next real-source gate can therefore proceed **without waiting for production discovery**: select tiny interior, fill-edge and tile-crossing windows from these pinned objects; compare bits, source validity, class semantics, actual affine/centres, stream values and durable Zarr reopening against rasterio. Confirm the prototype's grid alignment independently before making any HLS/S1 cross-sensor alignment claim. Retain prototype status in output provenance. A public v1.0 object, current product specification and separate qualification are still needed before promoting the final release or adding PO.DAAC production discovery/authentication. The [final PGE configuration](https://github.com/nasa/opera-sds-pge/blob/main/.ci/scripts/dswx_ni/opera_pge_dswx_ni_r5_final_runconfig.yaml) explicitly requests COG/DEFLATE/30 m, but configuration cannot replace inspection of those final output bytes.

### DIST-ALERT-S1: feasible with the COG engine

The [current developer product documentation](https://opera-adt.github.io/dist-s1/stable/pages/product_documentation/product_documentation/) defines ten separate COG layers: UInt8 status/status-acquisition/count/percentage, Float32 metric/maximum-metric/confidence, and Int16 onset/duration/last-date. Its missing markers are respectively 255, NaN and -1; onset dates use days since 2020-12-31. Preserve accumulated status separately from current-acquisition status, and retain confidence as an algorithm score. These meanings differ from HLS layers even where suffixes resemble one another.

The [official product description](https://d2pn8kiwq2w21t.cloudfront.net/documents/OPERA_Product_Document_oIArfpC.pdf) describes 30 m UTM/MGRS tiles and ASF distribution; it specifies an alert product without an S1 annual counterpart. The [final v1 specification](https://cumulus.asf.earthdatacloud.nasa.gov/PUBLIC/DATA/OPERA/OPERA_DIST-S1_ProductSpecv1.0.0_D-108778_Rev_A_2026-05-09_CL26-1594.pdf) is dated May 9, 2026. Production TIFF tags, tiles, compression, dtype, metadata and complete masks still need inspection; the older public test-bucket v0.1 examples cannot qualify v1.0.

Required work: a distinct product ID, collection/suffix/filename checks, ASF exact-object authentication, native grid admission, per-layer NaN/source-validity checks and published class policies. Do not reuse HLS confidence arithmetic or introduce a new metric threshold. No new numeric payload type appears necessary for the documented layers.

### DIST-ANN-HLS: feasible, but annual time needs its own model

The [HLS disturbance specification, sections 3–4](https://lpdaac.usgs.gov/documents/1766/OPERA_DIST_HLS_Product_Specification_V1.pdf) defines 21 single-layer COGs on 3660×3660, 30 m HLS/MGRS grids. Layers use UInt8 or Int16; examples include status, annual maxima, confidence, onset, duration, counts and previous-year indicators. Missing values are layer-specific, commonly 255/-1; date codes retain the 2020-12-31 epoch. Annual status adds previous-year event classes 9/10. Its filename has a summary year rather than an individual acquisition timestamp.

Implement a separate annual identity and temporal extent, retaining the CMR observation interval, summary year and processing timestamp. Do not invent a January 1 instantaneous observation, apply the alert parser, or treat all event dates as occurring in the summary year. Start with a bounded status/date subset, then qualify other annual layers independently. An LP DAAC alert login does not automatically authorize the annual collection's exact-object policy.

### DISP-S1: useful next HDF5 milestone

The [DISP-S1 specification](https://d2pn8kiwq2w21t.cloudfront.net/documents/OPERA_DISP_S1_Final_Product_Spec.pdf), especially sections 2.2–2.4 and 4.2–4.3, specifies CF-1.8/netCDF4-compatible HDF5, 30 m UTM frame grids, rank-two Float32 displacement/quality/correction fields, UInt16 connected-component labels, UInt8 masks and Int16 SHP counts. Float64 `/x`, `/y`, `/time`, and `/reference_time` carry coordinates/time. LOS displacement is in metres, positive towards the satellite. Corrections are separately stored; reference epochs can change. `recommended_mask` is a delivered 0/1 mask, with quality thresholds recorded in its attributes.

Prefer the archived `.nc` for initial admission. CMR also links `.zarr.json.gz` files: ASF explains these are [Kerchunk references to archived netCDF](https://storymaps.arcgis.com/stories/680e40e7b755413ba9321c5c1ca3e8c4), not proof of compatibility with a physical Zarr reader. Supporting them would add a separate reference-manifest and transport contract.

Current rank-two primitive payloads are promising, but production HDF5 chunk/index/filter layout and fill must be inspected before claiming reader compatibility. Add frame identity, general inspected UTM navigation, and both reference/secondary times. CMR `time_start` is the **reference** acquisition in the sampled record, while `time_end` is secondary; copying the existing discovery timestamp would mislabel it. Specify secondary-time filtering while preserving the full interval and source time units/calendar. Initial output should keep each pair independent; automatic reference stitching, velocity estimation and correction application are separate science operations. [ASF's FAQ](https://docs.asf.alaska.edu/datasets/disp_faq/) also distinguishes the portal's short-wavelength layer from unfiltered displacement.

### CSLC-S1: confirmed numeric gaps

The [CSLC specification](https://d2pn8kiwq2w21t.cloudfront.net/documents/OPERA_CSLC-S1_ProductSpec_v1.0.0_D-108278_Initial_2023-09-11_URS321269.pdf) describes complex radar imagery, Float64 carrier/flattening phases, separate calibration/noise metadata and 5 m easting ×10 m northing posting. The [suite page](https://www.jpl.nasa.gov/go/opera/products/cslc-product-suite/) includes UTM/polar stereographic grids. These are geocoded complex observations, not RTC intensity.

A bounded **actual public v1.1 reference-object inspection** used h5py/HDF5-C with anonymous conditional HTTP ranges. Source: [S1C VV sample](https://opera-pst-rs-pop1.s3.us-west-2.amazonaws.com/products/CSLC_S1/OPERA_L2_CSLC-S1_T034-071081-IW1_20250419T002708Z_20250716T223633Z_S1C_VV_v1.1/OPERA_L2_CSLC-S1_T034-071081-IW1_20250419T002708Z_20250716T223633Z_S1C_VV_v1.1.h5). Object size: 260,243,443 bytes; ETag: `"958ed2e52b45c652f4475d75f7d10feb-32"`. Inspection transferred **14,640 bytes in 32 range GETs**, excluding HEAD/control traffic, under a 2 MiB/128-request cap; no imagery chunks were decoded.

| Actual dataset | Dtype / rank / shape | Actual storage |
| --- | --- | --- |
| `/data/VV` | complex64; rank 2; 4899×19884 | 128×128 chunks; shuffle + DEFLATE level 4; default HDF5 fill `0j` |
| `/data/azimuth_carrier_phase`, `/data/flattening_phase` | Float64; same rank/shape | Same chunks/filters; default storage fill 0 |
| `/data/x_coordinates`, `/data/y_coordinates` | Float64; rank 1; lengths 19884/4899 | Contiguous; metre coordinate attributes |
| `/data/projection` | Int32 scalar | Contiguous; `epsg_code=32614` |

This proves a concrete layout, not acquisition correctness or production authentication. Storage initialization is not a scientific missingness rule: do not mask every complex zero or assume allocated pixels equal the storage fill.

Full CSLC support requires an explicit complex payload representation through parsing, reservations, streaming and durable output; Float64 science-plane support for the phase layers; complex missingness; burst/polarization/processing identity; and qualified navigation. Preserve original complex samples and metadata. Do not silently return intensity, discard imaginary components, apply calibration LUTs, remove phase, or substitute compressed CSLC intermediates. Those are different operations/products.

### Static companions

Separate IDs should bind static geometry to the matching burst/frame, grid and release. Their nominal epoch is not a fresh measurement every time a dynamic observation is read. Sampled legacy ASF URLs are rejected by today's routing policy and require explicit new qualification or a current Cumulus record.

The [DISP-S1-STATIC specification](https://cumulus.asf.earthdatacloud.nasa.gov/PUBLIC/DATA/OPERA/OPERA_DISP_S1_STATIC_ProductSpec_v1.0.0_D-108762_Rev_A_2025-03-20.pdf) describes DEM, UInt8 layover/shadow mask, and a **three-band** Float32 LOS unit-vector COG in east/north/up order, pointing ground-to-satellite. Files use DEFLATE; vector precision was intentionally rounded. The single-band COG reader cannot ingest that whole vector asset unchanged. DEM/mask could precede a deliberate band-selection extension. RTC-STATIC/CSLC-STATIC need their own per-layer dtype/layout/CRS inspections; shared labels do not justify automatic joins or reprojection.

## Owning code and bounded qualification plan

The relevant boundaries are concrete:

- [products.rs](../../src/products.rs): closed collection/version map, exact asset suffix matching and discovery that currently stores CMR `time_start` as one datetime; add product-specific temporal interpretation.
- [profile.rs](../../src/products/profile.rs): closed families, identity parsers, four COG profile dtypes, roles and per-layer quality policies; new displacement/phase/vector semantics should be explicit.
- [routing.rs](../../src/products/routing.rs) and [authenticated_http.rs](../../src/authenticated_http.rs): product-specific canonical hosts, object paths and redirect confinement. Existing RTC/LP DAAC/PO.DAAC constructors are deliberately narrow; another ASF collection must not inherit them by hostname alone.
- [native_cog.rs](../../src/native_cog.rs): tiled single-band admission; vector COGs require an explicit extension.
- [hdf5.rs](../../src/hdf5.rs): rank-two science data, legacy layout-v1–v3/v1-B-tree chunks, at most three deflate/shuffle/Fletcher32 filters; primitive science types Int8/Int16/UInt8/UInt16/UInt32/Float32. Float64 coordinate support does not mean Float64 science support. Other HDF5 index/filter layouts remain unqualified.
- [pixels.rs](../../src/pixels.rs), [array_grid.rs](../../src/products/array_grid.rs), and [scientific output](../../src/scientific_output/arrays.rs): no complex/Float64 science payload; array navigation currently names concrete qualified models, including NISAR UTM16N rather than arbitrary UTM. Raw samples, source validity, AOI and quality masks remain distinct; final source validation and atomic publication remain required.

Recommended order and initial gates: the bounded **pinned DSWx-NI v0.1 prototype gate described above can start immediately**, independently of the following production-archive milestones. It must remain visibly separate from final-v1.0 support.

1. **DIST-ALERT-S1 v1.0.** Pin one actual ASF granule and four representative layers: accumulated/current status, Float32 confidence, Int16 onset. Inspect all TIFF metadata first. Select small interior, source-missing and tile-crossing windows; compare exact values, raw class codes, NaNs, masks, coordinates and date epoch with independent rasterio. Expand to ten layers only after their own checks.
2. **DIST-ANN-HLS v1.** Pin one annual tile, two statuses and onset/last-date layers; retain summary year plus coverage interval. Exercise confirmed, previous-year and no-data codes using real bounded windows where available; do not claim unobserved classes were encountered. Add remaining annual layers later.
3. **DISP-S1 v1.0.** Inspect one original `.nc` with displacement, recommended mask, connected components and Float64 axes/time metadata. Begin with explicit native-index windows before geographic admission. Compare against h5py and independently opened NetCDF-C; test frame-edge/chunk-crossing selections, reference versus secondary time, and a second pair with a different reference epoch. Preserve corrections without applying them.
4. **Useful static subsets**, then full multi-band LOS support. Require exact dynamic/static frame or burst compatibility; refuse mismatched grids/versions.
5. **CSLC-S1 v1.1.** Decide the exact complex output representation before implementation. Qualify real/imaginary bits, NaN components, zero values, phase-plane Float64, uneven x/y spacing and output reopening with independent readers. UTM first; polar navigation is a separate gate.
6. **Final DSWx-NI, other NI variants and VLM after distribution evidence.** Final DSWx-NI software and expected COG outputs exist, but require accessible final output bytes and exact version/specification qualification before promotion beyond the pinned v0.1 prototype. Require a real collection before adding automatic discovery; do not assume S1 masks, calibration or temporal rules transfer. Other NI/VLM products retain their separate release gates.

For each milestone, start with at most one observation, a small AOI/native window and an explicit source-read cap (for example 8 MiB, revised only if measured metadata or a necessary source block exceeds it). Save original identity/ETag, selected byte ranges, source metadata and independent reference results. Protected captures must use an explicitly supplied private netrc; this assessment did not probe it. Avoid full-object fallbacks. Run normal `discover`, `plan`, `stream` and `fetch`; independently reopen the durable Zarr and compare raw values and all masks/coordinates. Verify changed-source, failed-read and cancellation paths leave no published incomplete output. Validate the relevant feature builds and existing regression suite after implementation. Report source bytes, requests, memory and overfetch; a tiny-window benchmark does not establish general throughput superiority.

**Decision:** a pinned DSWx-NI v0.1 prototype is feasible now using verified public COGs and the existing physical reader. For production-archive expansion, start with DIST-ALERT-S1 and DIST-ANN-HLS, then take DISP-S1 through a separate scientific-array gate. Keep final DSWx-NI, full CSLC, other NI variants and VLM outside the supported-product list until their distinct requirements pass.
