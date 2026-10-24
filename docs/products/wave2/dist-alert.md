# OPERA DIST-ALERT-HLS V1 qualification

The `opera-dist-alert` profile admits nine original layers from the NASA LP DAAC `OPERA_L3_DIST-ALERT-HLS_V1` collection (CMR `C2746980408-LPCLOUD`, catalog version `1`, DOI suffix `V1.001`). Processing filenames end `_30_v1`. DIST-ANN, provisional V0, DIST-S1 and undisclosed future processing versions require separate qualification.

The inspected real observation is `OPERA_L3_DIST-ALERT-HLS_T17SPU_20261021T160701Z_20261023T063220Z_S2A_30_v1`. Its HLS lineage is `HLS.S30.T17SPU.2026294T160701.v2.0`; nominal filename acquisition is 2026-10-21 16:07:01 UTC, actual source sensing/`Update_Date` is 2026-10-21 16:13:49.66491 UTC, and processing is 2026-10-23 06:32:20 UTC. These timestamps have different meanings and remain separate. The current alert is an accumulated state updated at the source observation's cadence; a single alert does not establish a disturbance history.

## Original variables and missingness

| Variables | Stored dtype | Source fill | Meaning |
| --- | --- | --- | --- |
| `VEG-DIST-STATUS`, `GEN-DIST-STATUS` | UInt8 | 255 | Accumulated detection status, preserving first/provisional/confirmed and ongoing/finished distinctions. |
| `VEG-DIST-CONF`, `GEN-DIST-CONF` | Int16 | -1 | Original algorithm confidence scores, with 0 indicating no disturbance; these are not probabilities or quality flags. |
| `VEG-DIST-DATE`, `GEN-DIST-DATE` | Int16 | -1 | Most recent event's first anomaly, in days since 2020-12-31; 0 means no anomaly within the last year. |
| `VEG-LAST-DATE`, `GEN-LAST-DATE` | Int16 | -1 | Last quality-assessed source observation with enough baseline observations; vegetation assesses land/water, generic disturbance assesses land. Same day epoch. |
| `DATA-MASK` | UInt8 | **No TIFF nodata tag** | Current HLS input mask: 0 is `no_data_or_masked`, 1 land, 2 water. This is current observation information, independent of accumulated alert status. |

Both status variables preserve codes 0–8 and 255. Code 0 means no disturbance; 1/2/3 are first/provisional/confirmed in the low category, 4/5/6 the corresponding high category, and 7/8 confirmed low/high finished. For vegetation, low/high distinguish <50% and ≥50% cover loss. For generic disturbance, they refer to spectral-anomaly thresholds; those categories do not express the same physical percentage. Confirmed vegetation status is supported by the algorithm confidence criterion; the confidence score remains stored separately. Vegetation confidence is the mean vegetation-loss anomaly multiplied by the squared number of loss anomalies. Generic confidence is the mean spectral distance multiplied by the number of above-threshold spectral anomalies. Their algorithm-specific accumulation and stopping rules are not a shared probability scale.

`dist_veg_confirmed` and `dist_gen_confirmed` accept status 3, 6, 7 and 8. Their `_ongoing` variants accept 3 and 6. These policies select accumulated detection states. `dist_current_land` accepts `DATA-MASK=1`; `dist_current_assessed` accepts 1 or 2. Each derived mask is intersected with its own source validity and geographic AOI mask. No current-scene mask silently erases an accumulated alert in another raw variable. In particular, DATA-MASK 0 is retained as an original valid source code, and its quality policies reject it. The combined class cannot identify a particular cloud, snow, water or source-gap reason. The first profile acquires no underlying HLS Fmask and does not invent that information.

The inspected grid is WGS84/UTM 17N, 3660×3660, 30 m, with corner affine `(30,0,600000,0,-30,3900000)`. All admitted layers use Deflate. Status uses an original 256-entry palette whose class indices remain unchanged. Eight layers use 512×512 tiles; DATA-MASK uses 256×256 tiles. No resampling, palette expansion, calibration application or inferred event series is performed. Native integer storage and date epochs are preserved; use `decode_cf=False` when inspecting original stored date codes through xarray.

## Evidence and bounded execution

[Inspection receipts](dist-alert-inspection.json) retain exact object sizes/ETags, TIFF tags, raw GDAL metadata XML, GeoKeys, metadata/encoded-tile range hashes, CMR lineage and independently decoded rasterio references. [Pinned ordinary request](dist-alert-request.json) selects a small geographic AOI and the nine layers. The ignored mirrors retain original byte offsets and only frozen ranges; they cannot serve uncaptured source bytes. Each selected encoded tile includes the original COG leader/trailer bytes. Capture retains 729,426 source bytes over nine objects, bounded by a 4 MiB cap; the cap excludes small CMR catalog metadata and authentication control traffic.

[Central acceptance](dist-alert-validation.json), [edge acceptance](dist-alert-validation-edge.json), and [ordinary production discovery/authenticated fetch](dist-alert-live-validation.json) compare native fetch output with independent rasterio reads and independently reopen Zarr with Zarr-Python/xarray. An additional independent rasterio/NumPy→Zarr workflow writes all equivalent raw values, AOI/source validity, coordinates/time, grid scalar and derived masks, then compares all arrays bit for bit. The edge fixture includes current DATA-MASK zeros while accumulated status remains source-valid. The selected specimens exercise no-disturbance, first/provisional, confirmed/finished states and nonzero confidence/onset values. Missing status/confidence/date sentinels are verified from the original TIFF metadata; the selected windows do not claim to exercise every fill/class combination.

The harness runs normal `plan`, `stream --values` and `fetch`; records first native chunk, planning, completion/output publication, encoded bytes, requests, physical tile overfetch, CPU, RSS, working reservations and output object counts; and checks rejected source read, changed final ETag and a CLI interrupt during a held read. Failed/cancelled outputs must remain unpublished. Loopback timing is a small-window correctness/resource measurement. The independent Python workflow includes process/library startup and uses local sparse files. It writes smaller metadata than the native provenance and complete physical-index manifest, so timing does not compare identical metadata workloads and establishes no remote-provider or general performance advantage.

```sh
# Explicit authenticated capture; paths are examples, not inferred credentials.
python bench/wave2_dist_alert.py capture --netrc /private/earthdata.netrc
python bench/wave2_dist_alert.py verify
python bench/wave2_dist_alert.py replay --binary target/debug/tile-ripper --output NEW.zarr
python bench/wave2_dist_alert.py replay --edge --binary target/debug/tile-ripper --output NEW_EDGE.zarr

# Normal native production acquisition with explicit authentication.
target/debug/tile-ripper fetch docs/products/wave2/dist-alert-request.json \
  --auth earthdata --netrc /private/earthdata.netrc --backend native \
  --output NEW_PRODUCTION.zarr --output-format zarr_v2 --json
```

The independent Python environment uses the existing benchmark requirements plus `tifffile==2024.6.18`. Production Tile-Ripper does not require Python, rasterio or GDAL. The live route is the exact LP DAAC Earthdata HTTPS archive→URS→archive→exact CloudFront resource chain. Credentials go only to URS; origin cookies remain isolated; signed resource queries stay in memory. The first profile does not qualify a public credential-free alternate route. No requester-pays requests or paid cloud subscription are involved.

## Primary definitions

- [NASA/JPL DIST product suite](https://www.jpl.nasa.gov/go/opera/products/dist-product-suite/).
- [Product Specification V1.0, Table 4-1 and metadata tables](https://d2pn8kiwq2w21t.cloudfront.net/documents/OPERA_DIST_HLS_Product_Specification_V1.pdf).
- [CMR collection metadata](https://cmr.earthdata.nasa.gov/search/collections.umm_json?concept_id=C2746980408-LPCLOUD).
- [Exact CMR granule](https://cmr.earthdata.nasa.gov/search/granules.umm_json?collection_concept_id=C2746980408-LPCLOUD&granule_ur=OPERA_L3_DIST-ALERT-HLS_T17SPU_20261021T160701Z_20261023T063220Z_S2A_30_v1).

CMR documents a July 21, 2026 processing change: `Input_DIST-ALERT_granule` can contain multiple comma-separated prior granules. Retain that entire lineage field, not an assumed single identifier. The selected specimen has one prior granule. Baseline years, calendar window and image IDs remain in the CMR properties; no history is reconstructed from those names.
