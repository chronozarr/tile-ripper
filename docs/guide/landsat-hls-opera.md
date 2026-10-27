# Landsat, HLS and OPERA

Product requests fetch named Landsat, Harmonized Landsat Sentinel-2 (HLS) and OPERA products for an area and date range. Tile-Ripper finds the files (or uses ones you pin), reads only the pixels in your area, and writes a new Zarr store with one group per source file. Values stay as stored. Quality masks are separate arrays you ask for by name.

## Before you start

Uses the build from [Installation](../installation.md). Commands assume `tile-ripper` is on your PATH; from a source checkout use `./target/release/tile-ripper`. Run them from the repository root, because `examples/...` paths are relative to it.

Whether you need an account depends on the product and route. LP DAAC, PO.DAAC and ASF are NASA data centers.

| Product name | Layers tested against independent readers | Account |
| --- | --- | --- |
| `landsat-c2-l2-sr` | Landsat 8/9 Collection 2 Level-2 surface reflectance: SR_B4, SR_B5, QA_PIXEL, QA_RADSAT, SR_QA_AEROSOL | No (Planetary Computer, signed automatically) |
| `landsat-tm-c2-l2-sr`, `landsat-etm-c2-l2-sr` | Landsat 4/5 TM and 7 ETM+: SR_B1 to B5, SR_B7, QA_PIXEL, QA_RADSAT, SR_CLOUD_QA, SR_ATMOS_OPACITY | No (Planetary Computer) |
| `hls-l30`, `hls-s30` | HLS version 2.0: B04, Fmask | No via Planetary Computer or the public NASA reference samples; Earthdata for LP DAAC production |
| `opera-dswx-hls` | OPERA Dynamic Surface Water Extent (DSWx) from HLS, v1.0 and v1.1: WTR, DIAG, SHAD, CLOUD | No for the public v1.0 sample; Earthdata for PO.DAAC v1.1 |
| `opera-rtc-s1` | OPERA terrain-corrected Sentinel-1 backscatter v1.0: VV, VH, mask | No for the public sample; Earthdata for ASF production |
| `opera-dswx-s1` | OPERA DSWx from Sentinel-1, product 1.0 from software 1.2: WTR, BWTR, CONF, DIAG | Earthdata (PO.DAAC) |
| `opera-dist-alert` | OPERA DIST-ALERT-HLS v1 (disturbance alert), one observation: nine layers | Earthdata (LP DAAC) |
| `opera-dswx-ni-prototype` | Pinned public DSWx-NI engineering specimen only: UInt8 WTR, BWTR, CONF and DIAG | No, for the four exact public objects only |
| `opera-dist-alert-s1` | DIST-ALERT-S1 v1.0: ten qualified COG layers | Earthdata (ASF Cumulus) |
| `opera-dist-ann-hls` | DIST-ANN-HLS v1: 21 qualified annual layers on the northern T17SNU specimen; the example reserves 128 MiB | Earthdata (LP DAAC) |

`tile-ripper products` prints these names as JSON, with bands, stored types and policies. It also lists bands that were not tested; claims here cover only the layers above. A public reference sample is a few real published files; it does not prove access to the production archive. Searching NASA's catalog needs no account; reading protected files does. See [Earthdata login](earthdata-login.md).

## Fetch

`discover` prints the observations found for an area as JSON, no account needed:

```sh
tile-ripper discover --product landsat-c2-l2-sr \
  --bbox -78.95,35.95,-78.94,35.96 \
  --start 2024-06-29T00:00:00Z --end 2024-06-29T23:59:59Z \
  --bands SR_B4,SR_B5,QA_PIXEL --limit 8
tile-ripper plan examples/product-landsat.json \
  --gdal-free --human
tile-ripper fetch examples/product-landsat.json \
  --gdal-free --output ./landsat.zarr > landsat-report.json
```

In a test run the plan found one scene with five layers (32 x 39 native pixels each) through `planetary_computer`, `credentials required: false`; the fetch wrote five groups. Planetary Computer's signing service returned HTTP 504 on several first attempts; the command fails without switching provider, and a rerun succeeded.

Product `fetch` prints its report as JSON (the same text saved as `manifest.json`); `--human` only changes `plan`. Choose a new `--output` path; existing paths are refused.

A public reference example needs no account. It pins one HLS L30 granule in its `observations` array:

```sh
tile-ripper plan examples/product-hls-l30.json \
  --gdal-free --human
tile-ripper fetch examples/product-hls-l30.json \
  --gdal-free --output ./hls-l30.zarr > hls-l30-report.json
```

Earthdata routes add the login. Without it, `plan` stops with `selected Earthdata route requires explicit --auth earthdata --netrc PRIVATE_FILE`.

```sh
tile-ripper fetch examples/product-opera-dswx-s1.json \
  --gdal-free --auth earthdata --netrc "$HOME/.netrc" \
  --output ./dswx-s1.zarr > dswx-s1-report.json
```

DIST-S1 and DIST-ANN use the same explicit private-netrc form after their own
exact archive route has been admitted. Do not reuse an HLS/PO.DAAC locator or
turn an example into an archive-wide access rule:

```sh
tile-ripper plan examples/opera-dist-alert-s1.json \
  --auth earthdata --netrc PRIVATE_FILE --gdal-free --human
tile-ripper plan examples/opera-dist-ann-hls.json \
  --auth earthdata --netrc PRIVATE_FILE --gdal-free --human
```

The DSWx-NI example is different: it supplies four public objects and exact
ETags with `provider: pinned`; it deliberately has no discovery or production
authentication route. See the [prototype receipt](../products/opera-expansion/dswx-ni.md).

## Request fields

| Field | Meaning |
| --- | --- |
| `product` | Name from the table. |
| `aoi` | `{"kind": "bounds", "crs": "EPSG:4326", "bbox": [W, S, E, N]}`, or `"kind": "polygon"` with `coordinates`. |
| `start`, `end` | RFC 3339 times, inclusive. Most products permit at most 31 days. DIST-ANN permits up to 366 days and selects its provider-recorded coverage interval; its filename year is not invented as an instantaneous observation time. |
| `bands` | Native names or aliases such as `red`, `nir`. |
| `quality_policies` | Optional named masks, such as `landsat_pixel_clear` or `hls_fmask_clear`. Each needs its raw layer in `bands`. See [definitions](../products/specifications.md). |
| `max_observations` | Cap on observations found. Exceeding it is an error, not truncation. |
| `observations` | Optional. Pins exact files and skips the search; an empty list selects nothing. You can paste in the `observations` from `discover`. |
| `provider` | `auto` (default), `planetary_computer`, `earthdata` or `pinned`. `--provider` overrides it. |

For HLS, `auto` compares the Planetary Computer and NASA catalogs. Without authorization it prefers a complete Planetary Computer result and lists NASA-only observations, which need `--auth`. `pinned` uses the observations you supplied. A prepared request never switches provider or renews an expired signature; run `plan` again.

## What you get

- `observations/NNNNNN`: one group per source file (observation and band), with `values`, `valid`, `aoi_mask`, `x`, `y`, `time` and `spatial_ref`. Groups are not in request order; read the band from the group's `source` attribute or from `manifest.json`.
- `values` keeps the stored type: Int16 or UInt16 reflectance codes, UInt8 or UInt16 flag layers, Float32 radar power with NaN kept as is.
- `quality_<policy>` (0 rejected, 1 accepted) appears only in the group holding the raw layer the policy reads, such as `QA_PIXEL`. It combines the policy with polygon inclusion and source validity and masks no other band. To combine it with a band, check that grid, window and time match.
- Scale and offset are attributes and are never applied; Landsat surface reflectance carried 2.75e-05 and -0.2. xarray's default open applies them and turns fill into NaN. See [Output, values and masks](output.md).
- `valid` does not mean cloud free.

Use a result only if the command exits successfully and `manifest.json` says `"status": "complete"`. An interrupted run may leave a hidden `.NAME.partial-*` sibling; never use it.

## Limits

- 1 to 32 observations and an interval of at most 31 days (DIST-ANN: at most 366 days). Most profiles allow 1 to 16 bands; DIST-ANN exposes six currently admitted bands.
- Native acquisition requires an area of interest (AOI) of at most 5° per axis, within ±80° latitude, with no antimeridian crossing.
- Catalog discovery with a bounding box allows at most 30° per axis and 400 square degrees. A search must fit one page; narrow the query if it fails. Split larger search areas into AOIs within the native acquisition limits before fetching.
- North-up 30 m WGS84 Universal Transverse Mercator (UTM) grids only; polar grids are not supported.
- No reflectance or decibel conversion, harmonization, compositing, alignment, reprojection or resampling.
- Landsat 7 scan-line gaps stay missing. HLS version 1.4, Landsat thermal products, DIST-ANN claims beyond the current layer-specific receipt scope or northern T17SNU reader representation, final/production DSWx-NI, and older ASF `datapool` URLs are not supported. The southern T20LRP reference's EPSG:32620 with negative northing is not called corrupt; it is an unqualified CRS/hemisphere representation for this reader.
- No resume: restart an interrupted fetch.

## Details and evidence

- [Product workflow](../products/workflow.md), [definitions](../products/specifications.md) and [tests](../products/qualification.md); [Landsat 4-7, DSWx-S1 and DIST-ALERT](../products/wave2/qualification.md).
- [Provider routing](../providers/qualification.md), [Supported data](supported-data.md), [support contract](../alpha/contract.md).
