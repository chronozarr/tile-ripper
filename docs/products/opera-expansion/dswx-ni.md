# DSWx-NI v0.1 public engineering prototype

`opera-dswx-ni-prototype` is a deliberately narrow, explicit-observation profile for one successful public OPERA engineering job. It is not a production archive route, a CMR collection, or an admission of final DSWx-NI v1.0.

The only admitted granule identity is `OPERA_L3_DSWx-NI_T42RVP_20260112T004524Z_20260417T094901Z_LSAR_30_v0.1`; all four layer URLs and strong ETags are fixed in [the example request](../../../examples/opera-dswx-ni-prototype.json). The source is an anonymous `opera-pst-rs-pop1` transfer object from a job whose exit marker was `0`. A sibling failed job and any unpinned transfer object are excluded.

The profile retains four one-band UInt8 COG layers, all with source fill 255:

| Layer | Stored meaning | Closed source-value domain |
| --- | --- | --- |
| WTR | Interpreted water class | 0, 1, 3, 8, 250, 251, 254, 255 |
| BWTR | Binary water class | 0, 1, 250, 251, 254, 255 |
| CONF | Categorical algorithm/wetland class | 0, 1, 2, 5, 6, 7, 30, 31, 32, 35, 36, 37, 250, 251, 254, 255 |
| DIAG | Rounded fuzzy score | 0–100, 250, 251, 254, 255 |

The raw class arrays remain authoritative. `250` (HAND), `251` (layover/shadow), and `254` (ocean) are contextual, source-valid class values; they are not converted to fill. `255` alone is the source missing value. The optional derived masks are explicit and local to their raw layer:

- `dswx_ni_wtr_openwater`: WTR `{1}`.
- `dswx_ni_wtr_water`: WTR `{1,3}`.
- `dswx_ni_wtr_edgewater`: WTR `{1,3,8}`; class 8 is the delivered partial/water-edge class, not a fractional-water estimate.
- `dswx_ni_bwtr_water`: BWTR `{1}`.

No S1 policy is widened or reinterpreted for NI.

## Pinned physical contract

Every layer must retain the native COG reader's physical metadata and exactly match the inspected specimen: 3660 × 3660, 30 m, 512 × 512 tiles, UInt8, nodata 255, EPSG:32642 / WGS 84 UTM 42N, pixel-is-area, affine `[399960,30,0,2900040,0,-30]`, DEFLATE/predictor 2, and single-band palettes for WTR/BWTR/CONF (DIAG is MinIsBlack). The physical identity gate requires the TIFF tags `DSWX_PRODUCT_VERSION=0.1`, `SOFTWARE_VERSION=0.3.0`, `PRODUCT_SOURCE=NISAR_GCOV`, `SENSOR=LSAR`, and `SPACECRAFT_NAME=NISAR`. Compression and predictor are evidenced by the exact ETag-bound source receipt because they are not retained in the current physical-metadata dictionary.

Four different times are retained rather than forced to agree:

- nominal filename acquisition: `2026-01-12T00:45:24Z`;
- source mosaic / zero-Doppler interval: `00:44:50Z` through `00:45:58Z`;
- filename processing stamp: `2026-04-17T09:49:01Z`;
- TIFF `PROCESSING_DATETIME`: `2026-04-17T09:49:12Z`.

## Qualification status

The version-bound [source receipt](dswx-ni-source-receipt.json) recorded strong `If-Match` range reads under a two-MiB body and 128-request cap per object: 72,441 bytes / 11 ranges for WTR, 55,285 / 11 for BWTR, 281,244 / 11 for CONF, and 440,730 / 9 for DIAG. It confirmed the physical TIFF layout and selected compressed tiles without a full-object request.

The [qualification receipt](dswx-ni-qualification.json) records separate 24-pixel interior, fill-edge, and tile-crossing fetches for all four layers. An independent rasterio/GDAL reader was served through a local proxy that permits only upstream ETag-checked bounded ranges. Raw bits, source-valid and AOI masks, native coordinates/CRS, requested quality masks, stream bytes/completion, source provenance, and durable Zarr reopening matched. The interior WTR window contains classes 0/1/3/8; the fill window contains 255 only; the crossing window contains 0 only. No occurrence of 250/251/254 is claimed.

The harness does not yet claim source-mutation, cancellation, or private-staging fault qualification for this public specimen; those remain separate output-transaction and transport gates.

Run only after building the requested native binary:

```sh
uv run --no-project --python 3.12 \
  --with-requirements docs/products/opera-expansion/reference.txt \
  python bench/opera_dswx_ni_qualification.py \
  --binary /absolute/path/to/tile-ripper --capture --qualify
```

Raw captures stay under ignored `prototype-output/opera-expansion/dswx-ni/`; compact, source-bound receipts are stored beside this document. A public final-v1.0 object, exact collection/distribution contract, and separate physical qualification are required before any promotion beyond this prototype.
