# Historical Landsat Collection 2 Level-2 SR

`landsat-tm-c2-l2-sr` admits Landsat 4/5 TM and `landsat-etm-c2-l2-sr` admits Landsat 7 ETM+. These profiles use the [current Planetary Computer Collection 2 Level-2 STAC collection](https://planetarycomputer.microsoft.com/api/stac/v1/collections/landsat-c2-l2), with bounded queries restricted to the applicable platform. They preserve source processing level/date, acquisition timestamp, exact object ETag, original sensor band identifiers, and native COG grids. Availability is demonstrated for the receipt specimens and queried intervals only.

Both profiles expose SR_B1 (blue), SR_B2 (green), SR_B3 (red), SR_B4 (near infrared), SR_B5 (shortwave infrared 1), and SR_B7 (shortwave infrared 2). Their PC asset keys are blue, green, red, nir08, swir16, and swir22. The common names describe approximate spectral roles; TM, ETM+, and OLI have different spectral responses. The sensor-specific `eo:bands` metadata is retained in each receipt. SR_B6 is not a surface-reflectance band. Thermal and surface-temperature processing are outside these profiles.

| Layer | Stored type | Source fill | Scale | Offset |
| --- | --- | --- | --- | --- |
| SR_B1/2/3/4/5/7 | uint16 | 0 | 0.0000275 | -0.2 |
| QA_PIXEL | uint16 | 1; fill flag bit 0 | 1 | 0 |
| QA_RADSAT | uint16 | none | 1 | 0 |
| SR_CLOUD_QA | uint8 | product fill 0; no physical TIFF marker | 1 | 0 |
| SR_ATMOS_OPACITY | int16 | -9999 | 0.001 | 0 |

The receipts distinguish physical TIFF nodata from product-semantic fill. In the inspected historical QA_PIXEL and SR_CLOUD_QA TIFFs, no physical TIFF nodata marker is set; QA_PIXEL still defines code 1 and bit 0 as fill, and SR_CLOUD_QA defines code 0 as fill. These storage conventions were checked against actual TIFF/catalog metadata and [USGS Collection 2 calibration guidance](https://www.usgs.gov/faqs/how-do-i-use-a-scale-factor-landsat-level-2-science-products). Calibration is recorded separately; original integer samples remain unchanged. QA_RADSAT=0 is valid even where SR is missing. SR_CLOUD_QA=0 is retained as its product fill code, independently of SR missingness. A quality layer does not override the SR source-validity mask. Atmospheric-opacity fill can occur independently of reflectance fill; [USGS describes LEDAPS aerosol-retrieval failures and default AOT behavior](https://www.usgs.gov/landsat-missions/landsat-collection-2-known-issues).

The [USGS QA definitions](https://www.usgs.gov/landsat-missions/landsat-4-7-surface-reflectance-quality-assessment) distinguish LEDAPS cloud QA from CFMask QA_PIXEL. The [USGS Science Product Guide, Table 5-4](https://d9-wret.s3.us-west-2.amazonaws.com/assets/palladium/production/s3fs-public/atoms/files/LSDS-1618_Landsat-4-7_C2-L2-ScienceProductGuide-v3.pdf) identifies SR_CLOUD_QA=0 as fill. `landsat_tm_etm_cloud_clear` excludes fill, cloud, cloud shadow, adjacent cloud, and snow (bits 1–4); DDV and water remain documented independent flags. `landsat_tm_etm_pixel_clear` excludes fill, dilated cloud, cloud, shadow, and snow. It does not require bit 6: [USGS documents an incorrect clear flag for some historical observations](https://www.usgs.gov/landsat-missions/landsat-collection-2-known-issues). Reserved cloud-shadow and snow-confidence state 2 is rejected. Cirrus flags from the OLI profile are not applied.

Sensor-specific saturation policies require zero QA_RADSAT. They recognize the historical dropped-pixel flag at bit 9 and ETM+ thermal high-gain saturation at bit 8; TM does not define bit 8. This differs from OLI terrain-occlusion semantics. Raw QA remains available separately from each derived policy. No aerosol QA layer is inferred from LEDAPS cloud QA.

The Landsat 7 receipt deliberately crosses missing and measured pixels in an observation acquired after the [31 May 2003 scan-line-corrector failure](https://www.usgs.gov/landsat-missions/landsat-7?page=1). Zero reflectance samples and QA fill codes are preserved; no interpolation or gap filling occurs.

Run the bounded reproducible gate with the independent validation environment:

```sh
prototype-output/zarr-env/bin/python bench/wave2_landsat.py capture
prototype-output/zarr-env/bin/python bench/wave2_landsat.py fetch-live
prototype-output/zarr-env/bin/python bench/wave2_landsat.py offline
prototype-output/zarr-env/bin/python bench/wave2_landsat.py discover
prototype-output/zarr-env/bin/python bench/wave2_landsat.py direct-live
prototype-output/zarr-env/bin/python bench/wave2_landsat.py faults lt05
```

Capture reuses the Wave 1 sparse original-byte mirror, retains logical offsets and exact response hashes, and enforces a 64 MiB budget per specimen. Frozen replay fails on uncaptured ranges. Its Python fixture obtains a container SAS once in memory and confines reads to the listed canonical objects; no SAS values are written. Normal CLI direct discovery uses the application's existing exact-object signing route. Native output is read independently through Zarr-Python/xarray and compared to rasterio/GDAL original source windows. The gate checks every raw value, dtype, source validity, AOI inclusion, derived quality mask, pixel-center coordinate, CRS and calibration attribute. Timing receipts are local observations; no speed claim follows from these samples.


The three `landsat-*-validation.json` receipts qualify ten layers per specimen: all six surface-reflectance bands, three QA layers, and atmospheric opacity. Their original-provider captures total 6,003,528 bytes (LT04), 5,242,880 bytes (LT05), and 7,981,505 bytes (LE07). Each full frozen gate includes ordinary `plan`, `fetch`, and value-bearing `stream` commands, plus an equivalent independently written rasterio→Zarr result whose every array matches exactly. Explicit completeness checks require all ten requested sources, unique observation groups, exact per-band array sets, and ten unique provisional stream chunks whose windows completely cover the selected outputs without overlap. Frozen validation and fault receipts record the tested executable SHA-256. Process wall time, CPU, peak RSS, network work and output object/byte counts are retained for both writers. The Python reference imports no Tile-Ripper scientific implementation; its historical product table and quality calculations are declared in this harness. The native and reference provenance documents differ in size and content, which contributes to total output bytes.

The direct receipts exercise five original-provider layers (red, near infrared, pixel QA, saturation QA and LEDAPS cloud QA), and explicit CLI discovery receipts confirm each selected historical sensor. Planetary Computer returned HTTP 429 during initial ten-band signing attempts; acquisition failed explicitly without switching provider. The bounded direct selections passed after cooldown. This service-rate observation is separate from scientific validation.

The LE07 original source window is 125 × 151 pixels. Its 5,748 zero red-band samples coincide exactly with QA_PIXEL fill. QA_RADSAT is zero throughout this window, including the gaps: a valid zero saturation code does not imply measured reflectance. The `gap_evidence` field records this comparison directly.

`landsat-lt05-faults.json` records six frozen fault injections: source identity mutation, cancellation, and HTTP 503 during fetch; and the same conditions after an actual provisional stream chunk. Every operation failed without publishing a complete manifest or complete stream event. These tests operate only against captured original bytes on loopback and send no fault traffic to the provider.
