# OPERA DIST-ALERT-S1 v1.0

`opera-dist-alert-s1` admits the ten named COG layers advertised by the ASF CMR collection `OPERA_L3_DIST-ALERT-S1_V1` (concept `C4090131664-ASF`, catalog version `1`). It is intentionally distinct from optical DIST-ALERT-HLS.

The public CMR result selected on 2026-10-27 was `OPERA_L3_DIST-ALERT-S1_T15UWR_20260228T235832Z_20260714T134753Z_S1C_30_v1.0` (`G4246933966-ASF`). CMR lists `Sentinel-1C`, a 2026-02-28T23:58:32Z acquisition, processing version `v1.0`, and exactly ten COG data URLs at ASF Cumulus. Anonymous conditional-range access to the canonical URL returned a 302 to URS. With the user-authorized private URS netrc, all ten production objects were captured under an eight-MiB per-object cap (2,932,053 bytes total), strongly ETag-pinned, and independently inspected. Credentials, cookies, and redirect-resource URLs were not retained.

The tracked [source receipt](dist-s1-cmr-receipt.json) records this exact specimen and its independent rasterio/GDAL oracle. The included [explicit request](../../../examples/opera-dist-alert-s1.json) pins the same sources; it still requires Earthdata authorization to run.

## Layer semantics locked from the public producer source

The public `opera-adt/dist-s1` producer constants define these layers, stored types, and fills. The `GEN-DIST-CONF` layer is a non-negative real metric, not a calibrated probability; Tile-Ripper supplies no confidence threshold.

| Layer | Stored dtype | Source fill |
| --- | --- | --- |
| `GEN-DIST-STATUS`, `GEN-DIST-STATUS-ACQ` | UInt8 | 255 |
| `GEN-METRIC`, `GEN-METRIC-MAX`, `GEN-DIST-CONF` | Float32 | NaN |
| `GEN-DIST-DATE`, `GEN-DIST-DUR`, `GEN-DIST-LAST-DATE` | Int16 | -1 |
| `GEN-DIST-COUNT`, `GEN-DIST-PERC` | UInt8 | 255 |

Status has the closed delivered domain `0..8,255`: none; first/provisional/confirmed low or high; confirmed finished low or high; and fill. `dist_s1_confirmed` selects `3,6,7,8`; `dist_s1_confirmed_ongoing` selects `3,6`. These derived masks are local to `GEN-DIST-STATUS` and do not recode raw classes or apply a confidence cutoff.

## Admission and qualification boundary

An authorized private netrc is required to acquire ASF Cumulus objects. The source inspection qualifies all ten individual native layers: 3660 × 3660, 512 × 512 tiles, ZSTD COG compression, 30 m north-up EPSG:32615, expected stored dtype/fill, MGRS `15UWR`, Sentinel-1C, and post-RTC acquisition lineage. The TIFFs do not carry a reliable product-release or band-description metadata tag. Tile-Ripper instead binds the exact canonical named-layer URL to its conditional strong ETag and validates the physical source evidence; it does not infer product release from the producer workflow's changing `version` tag.

The checked oracle window is a real valid interior region—not a fill-only corner—at source row/column `(3566,3130)`. Independent rasterio and `gdal_translate` reads match raw 32 × 32 bytes for `GEN-DIST-STATUS`, `GEN-METRIC`, `GEN-DIST-DATE`, and `GEN-DIST-CONF`; the receipt records a byte hash and a valid sample for all ten layers. The local capture is a **full-object bounded capture**, not a sparse-range efficiency claim.

Replay that independent source oracle only against an explicitly retained capture:

```sh
python3 bench/opera_dist_s1_qualification.py \
  --capture-dir prototype-output/opera-expansion/dist-s1/captured \
  --zarr-dir prototype-output/opera-expansion/dist-s1/qualification/oracle.zarr \
  --request prototype-output/opera-expansion/dist-s1/qualification/request-pinned.json \
  --stream-jsonl prototype-output/opera-expansion/dist-s1/qualification/stream-values.jsonl
```

The frozen native binary `df68e52fa5a146cfe984dad0088abb7e25178f4cc54dd8b9ae78b89bfd2bd038` then streamed all ten layers through the reviewed authenticated range client (ten source reads; 1,107,416 response-body bytes) and validated every conditional source version. A reopened Zarr v2 result has ten observation groups; every AOI pixel equals the independently inspected source pixel for its layer, while the AOI and source-valid masks remain explicit. The status masks agree with the closed policy definitions; this particular AOI contains statuses 0, 1, and 4, so neither confirmed policy invents a detection. `GEN-DIST-DUR` is stored with unit `days`, while the two date layers retain `days since 2020-12-31`.

An otherwise identical request with all catalog ETags omitted also resolved and streamed successfully, proving that discovery can acquire the conditional object identity. Replacing one explicit pin with a different strong ETag fails during preparation, before an output directory is published. This is a request-pin mismatch check, not a simulation of a source change after preparation. The shared transaction regressions cover post-preparation source changes and abrupt-writer cleanup.

The public repository contains v0.1 development examples. Those inform producer semantics but were not used to qualify ASF production v1.0 assets.

## Sources

- [ASF CMR collection](https://cmr.earthdata.nasa.gov/search/collections.json?concept_id=C4090131664-ASF)
- [Pinned CMR granule](https://cmr.earthdata.nasa.gov/search/granules.umm_json?collection_concept_id=C4090131664-ASF&granule_ur=OPERA_L3_DIST-ALERT-S1_T15UWR_20260228T235832Z_20260714T134753Z_S1C_30_v1.0)
- [DIST-S1 producer constants](https://github.com/opera-adt/dist-s1/blob/main/src/dist_s1/constants.py)
