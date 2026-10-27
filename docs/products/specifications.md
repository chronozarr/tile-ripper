# Wave 1 scientific product specifications

These profiles are closed, declarative descriptions of named product profiles. A profile identifies expected catalog, grid, band, storage, calibration, and quality semantics. It does not make a remote object readable or scientifically admissible. Every selected object still requires the normal physical inspection and execution gates.

The implementation preserves stored samples. Scale and offset are metadata (`physical = stored * scale + offset`); they are not applied automatically. Raw QA, classification, and radar-mask layers remain ordinary selected layers. A requested policy produces a separate named science mask from its listed raw layer. It never overwrites the raw layer, silently aligns another observation, or applies calibration or cross-sensor harmonization.

## Pinned authorities and catalog identities

| Profile | Release identity and access route | Specification authority |
| --- | --- | --- |
| `landsat-c2-l2-sr` | Landsat 8/9 Collection 2, `L2SP` or SR-only `L2SR`; Planetary Computer STAC collection `landsat-c2-l2`, using runtime-signed Azure HTTPS assets from the item | [USGS Landsat 8-9 Collection 2 Level-2 Science Product Guide v6.0 (May 2024)](https://d9-wret.s3.us-west-2.amazonaws.com/assets/palladium/production/s3fs-public/media/files/LSDS-1619_Landsat8-9-Collection2-Level2-Science-Product-Guide-v6.pdf), [USGS Collection 2 QA definitions](https://www.usgs.gov/landsat-missions/landsat-collection-2-quality-assessment-bands), [USGS Landsat STAC collections](https://www.usgs.gov/landsat-missions/spatiotemporal-asset-catalog-stac), [Planetary Computer `landsat-c2-l2` access example](https://planetarycomputer.microsoft.com/docs/quickstarts/reading-stac-r/) |
| `hls-l30` | NASA LP DAAC CMR/STAC collection `HLSL30_2.0`; Earthdata-authenticated HTTPS or temporary in-region S3 credentials | [HLS User Guide V2.0, April 2026](https://lpdaac.usgs.gov/documents/1698/HLS_User_Guide_V2.pdf) |
| `hls-s30` | NASA LP DAAC CMR/STAC collection `HLSS30_2.0`; same access boundary as L30 | [HLS User Guide V2.0, April 2026](https://lpdaac.usgs.gov/documents/1698/HLS_User_Guide_V2.pdf) |
| `opera-dswx-hls` | NASA PO.DAAC collection `OPERA_L3_DSWX-HLS_V1`, CMR collection version 1.0; admitted granule processing releases 1.0 and 1.1; Earthdata-authenticated HTTPS or temporary in-region S3 credentials | [OPERA DSWx-HLS Product Specification v1.0.1, Rev B, July 10 2024](https://archive.podaac.earthdata.nasa.gov/podaac-ops-cumulus-docs/web-misc/opera/OPERA_DSWx-HLS_ProductSpec_v1.0.0_D-107395_RevB.pdf), [PO.DAAC collection landing page](https://podaac.jpl.nasa.gov/dataset/OPERA_L3_DSWX-HLS_V1) |
| `opera-rtc-s1` | NASA ASF DAAC collection `OPERA_L2_RTC-S1_V1`, product version 1.0; Earthdata-authenticated HTTPS or temporary in-region S3 credentials | [OPERA RTC-S1 Product Specification v1.0.3, Rev B, March 21 2024](https://d2pn8kiwq2w21t.cloudfront.net/documents/ProductSpec_RTC-S1.pdf), [ASF OPERA RTC-S1 product guide](https://hyp3-docs.asf.alaska.edu/guides/opera_rtc_product_guide/) |
| `opera-dswx-ni-prototype` | One anonymous, public engineering job: exact v0.1/software-0.3.0 objects, names and ETags only; no CMR collection or automatic route | [prototype source and qualification receipt](opera-expansion/dswx-ni.md) |
| `opera-dist-alert-s1` | ASF collection `OPERA_L3_DIST-ALERT-S1_V1`, product v1.0, exact Earthdata Cumulus object route | [qualified ten-layer source/output scope](opera-expansion/dist-s1.md) |
| `opera-dist-ann-hls` | LP DAAC collection `OPERA_L3_DIST-ANN-HLS_V1`, annual v1 COGs with exact Earthdata route and provider-recorded coverage extent | [qualified 21-layer source/output scope](opera-expansion/dist-ann.md) |

Catalog labels and profile metadata are expectations. They do not establish that tile-ripper has performed a live catalog, authentication, range-read, decoder, or scientific equality qualification for these products.

## Band and grid declarations

All imagery has 30 m posting. Landsat uses its scene-native map grid, normally UTM (with the product-specified polar stereographic exception). HLS L30/S30 and DSWx-HLS use the UTM, MGRS-aligned HLS grid. RTC-S1 uses a predefined burst grid in UTM, with EPSG:3413 north of 75 degrees as specified by OPERA.

### Landsat 8/9 Collection 2 Level-2 SR

| Native band | Catalog asset | Meaning | Stored type | Nodata | Scale | Offset |
| --- | --- | --- | --- | ---: | ---: | ---: |
| `SR_B1` | `coastal` | coastal/aerosol reflectance | UInt16 | 0 | 0.0000275 | -0.2 |
| `SR_B2` | `blue` | blue reflectance | UInt16 | 0 | 0.0000275 | -0.2 |
| `SR_B3` | `green` | green reflectance | UInt16 | 0 | 0.0000275 | -0.2 |
| `SR_B4` | `red` | red reflectance | UInt16 | 0 | 0.0000275 | -0.2 |
| `SR_B5` | `nir08` | near-infrared reflectance | UInt16 | 0 | 0.0000275 | -0.2 |
| `SR_B6` | `swir16` | shortwave-infrared 1 reflectance | UInt16 | 0 | 0.0000275 | -0.2 |
| `SR_B7` | `swir22` | shortwave-infrared 2 reflectance | UInt16 | 0 | 0.0000275 | -0.2 |
| `QA_PIXEL` | `qa_pixel` | pixel condition bit field | UInt16 | semantic fill bit | 1 | 0 |
| `QA_RADSAT` | `qa_radsat` | saturation/terrain bit field | UInt16 | none assigned | 1 | 0 |
| `SR_QA_AEROSOL` | `qa_aerosol` | aerosol QA bit field | UInt8 | 1 | 1 | 0 |

USGS specifies stored SR codes 1–65455, with fill 0. This numeric range is descriptive calibration metadata and is never substituted for source nodata or cloud/quality acceptance; original codes are retained even when scaled reflectance is negative.

Identity admission accepts only `LC08` or `LC09`, Collection `02`, and processing `L2SP` or `L2SR`. It recognizes both the seven-field USGS product identifier (which includes the processing date) and the exact six-field Planetary Computer STAC item identifier (which omits that date); an independently supplied platform or processing field must agree with the identifier.

### HLS V2.0

L30 contains `B01`-`B07` surface reflectance, `B09` cirrus TOA reflectance, and `B10`/`B11` TOA brightness temperature. Reflectance and TOA-reflectance layers are Int16 with nodata -9999 and scale 0.0001. Temperature layers are Int16 with nodata -9999 and scale 0.01 degrees Celsius.

S30 contains `B01`-`B08`, `B8A`, and `B09`-`B12`. `B01`-`B08`, `B8A`, `B11`, and `B12` are surface reflectance; `B09` water-vapor and `B10` cirrus are TOA reflectance. All are Int16 with nodata -9999 and scale 0.0001. `B08` is broad NIR and resolves from `nir_broad`; `B8A` is narrow NIR and resolves from `nir` or `nir_narrow`. They are never treated as aliases of each other.

Both products contain `Fmask` (UInt8, fill 255) and `SZA`, `SAA`, `VZA`, `VAA` (UInt16, fill 40000, scale 0.01 degrees). HLS is harmonized by the product producer; this profile performs no further bandpass, BRDF, or cross-sensor harmonization.

### OPERA DSWx-HLS V1

The product has exactly ten 3660 by 3660 layers on the HLS grid:

| Layer | Stored type | Nodata | Defined domain |
| --- | --- | ---: | --- |
| `WTR` | UInt8 | 255 | 0 not water; 1 open water; 2 partial surface water; 252 snow/ice; 253 cloud/cloud shadow/adjacency; 254 ocean masked; 255 fill |
| `BWTR` | UInt8 | 255 | 0 not water; 1 water; 252 snow/ice; 253 cloud/cloud shadow/adjacency; 254 ocean masked; 255 fill |
| `CONF` | UInt8 | 255 | 0-4 unmasked; 10-14 cloud/cloud-shadow/adjacency; 20-24 snow/ice; ones digit 0 not water, 1 high-confidence open water, 2 moderate-confidence open water, 3 conservative partial water, 4 aggressive partial water; 254 ocean; 255 fill |
| `DIAG` | UInt16 | 65535 | a five-digit decimal test code from 00000 through 11111; this descriptor does not invent bit semantics for it |
| `WTR-1` | UInt8 | 255 | 0 not water; 1 open water; 2 partial water; 254 ocean class specified but not used by the standard v1.0 product; 255 fill |
| `WTR-2` | UInt8 | 255 | 0 not water; 1 open water; 2 partial water; 254 ocean; 255 fill |
| `LAND` | UInt8 | 255 | 0-99 low-intensity developed; 100-199 high-intensity developed; 200 water/wetland/mangrove; 201 selected forest classes; 255 fill |
| `SHAD` | UInt8 | none specified | 0 shadow; 1 not shadow |
| `CLOUD` | UInt8 | 255 | 0-15 combinations of cloud-shadow/adjacency (bit value 1), snow/ice (2), cloud (4), and processor-defined bit 3 (8); 254 ocean masked; 255 fill |
| `DEM` | Float32 | none specified | elevation in meters above the WGS84 ellipsoid |

The water outputs are separate scientific products. A membership policy for `WTR` is not applied to `BWTR`, `CONF`, `WTR-1`, or `WTR-2`. Undefined class values are errors.

### OPERA RTC-S1 V1

Available polarizations are individual Float32 linear gamma0-power COGs: usually `VV` and `VH`, with `HH` and `HV` in horizontally polarized acquisition regions. Each polarization is optional in the descriptor because a granule contains only the acquired channels. The `mask` is UInt8: 0 valid/unaffected, 1 valid/shadow, 2 valid/layover, 3 valid/layover and shadow, and 255 invalid/fill. Static incidence-angle, number-of-looks, and normalization-factor layers belong to the separate RTC-S1-STATIC product and are outside this profile.

The finite-sample policy tests only IEEE finiteness. It does not convert linear gamma0 to decibels and does not infer geometric validity; use the separate RTC mask policy for that.

### OPERA DSWx-NI v0.1 engineering prototype

This is not final DSWx-NI v1.0 or a production archive profile. It accepts only the public, ETag-pinned NISAR GCOV/LSAR specimen described in the [prototype receipt](opera-expansion/dswx-ni.md). All four 3660 by 3660, 30 m UTM 42N layers are UInt8 with fill 255: `WTR`, `BWTR`, `CONF`, and `DIAG`. Raw class indices are stored unchanged. `250` (HAND), `251` (layover/shadow), and `254` (ocean) remain valid contextual raw values; `255` alone is missing.

The optional WTR policies are distinct: open water `{1}`, water `{1,3}`, and edge/partial water `{1,3,8}`. BWTR water is `{1}`. No S1 policy is widened or aliased for NI. `CONF` is categorical and `DIAG` is a 0–100 rounded fuzzy score; neither is a probability.

### OPERA DIST-ALERT-S1 v1.0

The integration declares ten single-band, 30 m UTM COGs from one accumulated Sentinel-1 observation: `GEN-DIST-STATUS`, `GEN-METRIC`, `GEN-DIST-STATUS-ACQ`, `GEN-METRIC-MAX`, `GEN-DIST-CONF`, `GEN-DIST-DATE`, `GEN-DIST-COUNT`, `GEN-DIST-PERC`, `GEN-DIST-DUR`, and `GEN-DIST-LAST-DATE`. Status/count/percent are UInt8 with fill 255; metric/maximum/confidence are Float32 with NaN fill; date/duration/last-date are Int16 with fill -1. `GEN-DIST-DATE` and `GEN-DIST-LAST-DATE` are **days since 2020-12-31**. `GEN-DIST-DUR` is a **duration in days**, not an epoch date. Confidence is an uncalibrated source metric, not a probability or threshold.

The only declared status policies select confirmed classes `{3,6,7,8}` or the ongoing subset `{3,6}` from `GEN-DIST-STATUS`. The [source/output receipt](opera-expansion/dist-s1-cmr-receipt.json) records the exact qualified specimen and observed class scope.

### OPERA DIST-ANN-HLS v1 annual summary

The [qualification receipt](opera-expansion/dist-ann-qualification-receipt.json) covers all 21 annual layers, including UInt8 `VEG-DIST-STATUS` and `GEN-DIST-STATUS` (fill 255), plus Int16 `VEG-DIST-DATE`, `GEN-DIST-DATE`, `VEG-LAST-DATE`, and `GEN-LAST-DATE` (fill -1). Each named `*-DATE`/`*-LAST-DATE` is **days since 2020-12-31**. `VEG-DIST-DUR` and `GEN-DIST-DUR` are instead **durations in days**. A summary-year token is not converted to a January 1 observation. Instead, the output carries the exact CMR coverage start/end as `time_bounds`, uses coverage start for its time coordinate, and records the summary year separately. Current-year confirmed classes are `{3,6,7,8}`; retained previous-year confirmed classes are `{9,10}`.

The qualified reader representation is northern T17SNU, not a blanket global admission. A southern T20LRP reference uses EPSG:32620 with negative northing. It is retained as an unqualified CRS/hemisphere representation rather than characterized as corrupt or scientifically invalid.

## Closed quality policies

| Policy | Required raw layer | Acceptance rule |
| --- | --- | --- |
| `landsat_pixel_clear` | `QA_PIXEL` | Reject reserved confidence value 2 in cloud-shadow, snow/ice, or cirrus fields and contradictory flag/confidence encodings; accept when fill, dilated-cloud, cirrus, cloud, cloud-shadow, and snow bits are absent and clear bit 6 is set. Water is not excluded. |
| `landsat_radiometrically_unsaturated` | `QA_RADSAT` | Reject any unused bit; accept only when Band 1-7 and Band 9 saturation and terrain-occlusion bits are all zero. |
| `landsat_aerosol_valid` | `SR_QA_AEROSOL` | Accept documented center-pixel valid-retrieval codes 2, 66, 130, and 194. Reject codes absent from the product guide's enumerated domain. |
| `landsat_aerosol_recommended` | `SR_QA_AEROSOL` | Accept 2, 66, and 130; code 194 is a valid high-aerosol retrieval but is excluded because the guide does not recommend high-aerosol pixels. |
| `hls_fmask_clear` | `Fmask` | Reject reserved bit 0 when set; reject fill 255; accept when cloud (1), adjacency (2), shadow (3), and snow/ice (4) bits are zero. Water bit 5 and aerosol bits 6-7 do not make a pixel cloudy. |
| `hls_fmask_clear_low_aerosol` | `Fmask` | HLS clear plus aerosol code 0 (climatology) or 1 (low). Numeric extraction is `(word >> 6) & 3`; 2 is medium and 3 high. |
| `dswx_wtr_water`, `dswx_bwtr_water` | matching layer | `WTR`: classes 1 or 2; `BWTR`: class 1. |
| `dswx_conf_unmasked_water` | `CONF` | Classes 1-4 only. |
| `dswx_conf_any_water` | `CONF` | Classes 1-4, 11-14, or 21-24, explicitly retaining obscured water classifications. |
| `dswx_wtr1_water`, `dswx_wtr2_water` | matching layer | Classes 1 or 2 in that intermediate layer. |
| `dswx_shadow_free` | `SHAD` | Class 1. The inverse-looking class assignment follows the product specification. |
| `dswx_cloud_obstruction_free` | `CLOUD` | Classes 0 or 8: low three obstruction bits are zero; bit 3 is retained without exclusion. Ocean mask 254 and fill 255 are excluded. This policy does not interpret bit 3 as a water detection. |
| `opera_rtc_finite` | one selected polarization | Float32 sample is finite. |
| `opera_rtc_mask_valid` | `mask` | Classes 0-3. |
| `opera_rtc_mask_unaffected` | `mask` | Class 0 only. |
| `dswx_ni_wtr_openwater`, `dswx_ni_wtr_water`, `dswx_ni_wtr_edgewater` | `WTR` | Respectively `{1}`, `{1,3}`, and `{1,3,8}` for the pinned NI specimen. |
| `dswx_ni_bwtr_water` | `BWTR` | Class 1 only for the pinned NI specimen. |
| `dist_s1_confirmed`, `dist_s1_confirmed_ongoing` | `GEN-DIST-STATUS` | Respectively `{3,6,7,8}` and `{3,6}`; source/output qualification is scoped by the recorded receipt. |
| `dist_ann_*confirmed` | matching annual status layer | Current-year `{3,6,7,8}`, prior-year `{9,10}`, or their explicit union; the raw annual status is retained. |

Policy names are a closed enum. Unknown policy names, mismatched sample types, reserved states, undefined class codes, and contradictory Landsat QA encodings return errors. This prevents a new producer code or malformed word from being silently treated as either good or bad science.

## Bounded authenticated HLS evidence

The canonical, unsigned request examples [`hls-l30-authenticated-request.json`](hls-l30-authenticated-request.json) and [`hls-s30-authenticated-request.json`](hls-s30-authenticated-request.json) pin one production V2 granule each and request only `B04` and `Fmask` over a narrow AOI. The compact receipt [`hls-auth-validation.json`](hls-auth-validation.json) records the strong ETags, exact source windows, transport request and byte counts, memory reservations, and an independent comparison with rasterio/GDAL plus xarray/zarr-python.

The independent comparison checks raw values and stored dtype width, pixel-center coordinates, a separately transformed WGS84 AOI mask, nodata validity, the derived `hls_fmask_clear` mask, acquisition and processing times, and retained physical GeoTIFF metadata. Credentials and temporary signed resource URLs are neither stored nor reported. This is evidence for those two pinned, narrow examples only; it is not an archive-wide qualification claim.

### DSWx catalog and processor distinctions

The PO.DAAC CMR collection `OPERA_L3_DSWX-HLS_V1` retains collection version `1.0` while current distributed granules and their TIFF `PRODUCT_VERSION` identify processing release `1.1`. The profile explicitly admits inspected releases 1.0 and 1.1, records each observation's exact processing release, and requires it to agree with the physical TIFF metadata. The collection version is retained separately. Current v1.1 uses ordinary EPSG UTM keys; the narrow custom-UTM normalization applies only to the observed v1.0 representation.

The July 2024 product specification v1.0.1 labels CLOUD bit 3 as input HLS-Fmask water. The official [PROTEUS v1.0.2 color table](https://github.com/nasa/PROTEUS/blob/v1.0.2/src/proteus/dswx_hls.py#L1547-L1591), [class definitions](https://github.com/nasa/PROTEUS/blob/v1.0.2/src/proteus/dswx_hls.py#L1754-L1777), and [mapping implementation](https://github.com/nasa/PROTEUS/blob/v1.0.2/src/proteus/dswx_hls.py#L1919-L1993) identify that bit as aerosol class reassignment and do not map the HLS water bit into CLOUD. The [tagged v1.0.1 processor](https://github.com/nasa/PROTEUS/blob/v1.0.1/src/proteus/dswx_hls.py#L1912-L1989) also uses aerosol reassignment; this is a guide/processor discrepancy rather than a demonstrated version boundary. The inspected v1.1 TIFF records `SOFTWARE_VERSION=1.0.2` and `AEROSOL_CLASS_REMAPPING_ENABLED=TRUE`. Retain those exact source metadata and raw CLOUD codes. `dswx_cloud_obstruction_free` tests only the low three obstruction bits and excludes ocean/fill; it accepts 0 or 8 without presenting bit 3 as water.

### Output units

Output `units` use `1` for reflectance, linear gamma0 power ratios and categorical/QA codes, `m` for elevation, `degree` for angles, and `degree_Celsius` for brightness temperature. Quantity, class/bit interpretation, radiometric normalization and vertical-reference meaning remain in descriptive/product metadata and original source metadata. These unit strings follow [CF units conventions](https://cfconventions.org/cf-conventions/cf-conventions.html#units); they do not imply conversion of stored values. Calibration remains separate.
