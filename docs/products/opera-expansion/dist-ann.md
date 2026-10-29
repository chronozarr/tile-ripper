# OPERA DIST-ANN-HLS V1

`opera-dist-ann-hls` is an annual-summary profile for the LP DAAC collection `OPERA_L3_DIST-ANN-HLS_V1` (CMR `C2519119034-LPCLOUD`). It declares all 21 producer COG layers with their inspected native dtype, nodata, units, and tile contracts. The separate qualification receipt records the live exact-version acquisition and independent checks.

The annual filename token is a **summary year**, not an acquisition timestamp. Every observation carries a typed `properties.temporal_extent` object with `coverage_start`, `coverage_end`, and `summary_year`. `datetime` is exactly `coverage_start`; selection is by interval overlap. The example's live CMR granule illustrates why: its 2025 summary has coverage from `2025-01-02T16:13:44.046Z` through `2025-12-26T16:23:45.583Z`, not a fictional `2025-01-01` instant. See [the compact CMR receipt](dist-ann-cmr-receipt.json).

All 21 layers are native 30 m MGRS COGs. Status layers are UInt8 with nodata 255. Initial- and last-date layers are Int16 with nodata -1 and retain the producer day epoch `2020-12-31`; they are not converted to timestamps. The remaining producer metrics, confidence values, counts, durations, and history/index layers retain their declared raw dtype and nodata. Status masks are explicitly separated:

- `dist_ann_*_confirmed` selects current-year confirmed codes 3, 6, 7, 8 plus retained previous-year confirmed codes 9, 10.
- `dist_ann_*_current_year_confirmed` selects only 3, 6, 7, 8.
- `dist_ann_*_previous_year_confirmed` selects only 9, 10.

Here `*` is `veg` or `gen`. A policy is a raw categorical mask, not a confidence calibration or a substitute for source validity.

## Access and qualification boundary

The CMR identity, canonical protected href shapes, and annual interval are recorded. Native TIFF inspection found 3660 × 3660, 30 m north-up UTM, DEFLATE COGs; most layers use 256 × 256 blocks while `*-LAST-DATE` uses 512 × 512. The [compact source receipt](dist-ann-source-receipt.json) pins headers, object versions, tags, and hashes of ignored 8 × 8 samples for all 21 layers.

The [qualification receipt](dist-ann-qualification-receipt.json) records one fresh authenticated 21-layer acquisition with the release binary, independent ETag-bound TIFF decoding for each delivered source window, durable Zarr reopening, raw bits, source-validity and AOI masks, matching applicable quality masks, native axes/CRS/provenance, and exact provider time bounds. Its stream replay covered 2,940 chunks and ended with source-version validation. It retains only the three observed redirect origins (LP DAAC, URS, and the admitted LP DAAC CloudFront resource); credentials were supplied to URS only. This qualifies the pinned conventional northern-hemisphere specimen, not other annual tiles or provider grid conventions.

The original receipt above covers conventional northern-hemisphere WGS84 UTM. A subsequent [southern qualification](../dist-ann-southern/README.md) admits the verified T20LRP extended-northern-zone convention through an annual-only physical/MGRS gate, preserving EPSG:32620 and negative northings. Earlier source and output receipts remain unchanged.

Run the bounded harness only with a supplied authorized netrc and a built binary (the tracked dependency set is [reference.txt](reference.txt)):

```sh
uv run --no-project --python 3.12 --with-requirements docs/products/opera-expansion/reference.txt \
  python bench/opera_dist_ann_qualification.py \
  --binary /absolute/path/to/tile-ripper --netrc /absolute/path/to/private.netrc --capture --qualify
```

Raw outputs belong under ignored `prototype-output/opera-expansion/dist-ann/`; credentials and authenticated redirect URLs are never written to receipts.

The PSD documents the annual filename convention, native layers, and status/date coding: [OPERA DIST HLS Product Specification V1](https://lpdaac.usgs.gov/documents/1766/OPERA_DIST_HLS_Product_Specification_V1.pdf). The LP DAAC [collection landing page](https://data.nasa.gov/dataset/opera-land-surface-disturbance-annual-from-harmonized-landsat-sentinel-2-product-version-1-0dd70) confirms one COG per layer and annual DIST-ALERT summarization.
