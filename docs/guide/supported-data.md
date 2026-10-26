# Supported data

Tile-Ripper supports specific product versions and file layouts, not whole
missions. A dataset is supported only when its exact version, grid and access
route appear below. Another processing release, grid or provider of the same
mission is not covered until it has been tested against independent readers on
real files.

`tile-ripper products` lists the built-in product names used in product
requests (the names in code font below). Sentinel-2, SWOT and the Zarr stores
use request files instead of product names. `plan` reads the source metadata
and refuses a layout outside its profile before any science data is fetched.

Commands assume `tile-ripper` is on your PATH; from a source checkout use
`./target/release/tile-ripper`. Example paths are relative to the repository
root. Every row uses the build from [Installation](../installation.md).

## Supported datasets

"Earthdata login" means a NASA Earthdata account and a private netrc file
passed with `--auth earthdata --netrc "$HOME/.netrc"`; see
[Earthdata login](earthdata-login.md).

| Dataset and version | Request name or example file | Account needed | Area selection | Key limits | Guide |
| --- | --- | --- | --- | --- | --- |
| Sentinel-2 Collection 1 Level-2A, Earth Search catalog (SpatioTemporal Asset Catalog, STAC, collection `sentinel-2-c1-l2a`) | STAC request: [`docs/native-sentinel/request.json`](../native-sentinel/request.json) | No | WGS84 polygon or bounding box; at most 5° per axis; latitude within ±80° | UInt16 bands on WGS84 Universal Transverse Mercator (UTM) grids only; no antimeridian crossing; each band of each scene is a separate group; direct flags cap 8 items, 8 assets and 1 million output pixels | [Sentinel-2](sentinel-2.md) |
| Landsat 8/9 Collection 2 Level-2 surface reflectance | `landsat-c2-l2-sr` ([example](../../examples/product-landsat.json)) | No (Microsoft Planetary Computer; access is signed automatically at run time) | WGS84 bounding box or polygon; at most 30° per axis and 400 square degrees | North-up 30 m UTM grids only (no polar grids); per request at most 31 days, 32 observations and 16 bands; no surface temperature | [Landsat, HLS and OPERA](landsat-hls-opera.md) |
| Landsat 4/5 TM and Landsat 7 ETM+ Collection 2 Level-2 surface reflectance | `landsat-tm-c2-l2-sr`, `landsat-etm-c2-l2-sr` ([TM](../../examples/product-landsat-tm.json), [ETM+](../../examples/product-landsat-etm.json)) | No (Planetary Computer) | As Landsat 8/9 | Separate TM and ETM+ profiles; Landsat 7 scan-line gaps stay missing (no gap filling); no thermal processing; same request caps as Landsat 8/9 | [Landsat, HLS and OPERA](landsat-hls-opera.md) |
| Harmonized Landsat Sentinel-2 (HLS) L30 and S30 version 2.0 | `hls-l30`, `hls-s30` ([L30](../../examples/product-hls-l30.json), [S30](../../examples/product-hls-s30.json)) | No through Planetary Computer or for the public NASA reference samples; Earthdata login for the NASA LP DAAC production archive | As Landsat 8/9 | L30 and S30 stay separate observations; HLS v1.4 excluded; independent checks cover B04 and Fmask; north-up 30 m UTM only | [Landsat, HLS and OPERA](landsat-hls-opera.md) |
| OPERA Dynamic Surface Water Extent from HLS (DSWx-HLS) v1.0 and v1.1 | `opera-dswx-hls` ([example](../../examples/product-opera-dswx-hls.json)) | No for the public v1.0 sample; Earthdata login for current PO.DAAC v1.1 | As Landsat 8/9 | Tested layers WTR, DIAG, SHAD and CLOUD, not every diagnostic layer; classes stay raw codes | [Landsat, HLS and OPERA](landsat-hls-opera.md) |
| OPERA radiometric terrain-corrected Sentinel-1 backscatter (RTC-S1) v1.0 | `opera-rtc-s1` ([example](../../examples/product-opera-rtc-s1.json)) | No for the public sample; Earthdata login for current ASF production | As Landsat 8/9 | Float32 linear gamma0 power (VV, VH) plus mask; no dB conversion; tested polarizations only; older ASF datapool URLs not supported | [Landsat, HLS and OPERA](landsat-hls-opera.md) |
| OPERA DSWx-S1 v1.0 | `opera-dswx-s1` ([example](../../examples/product-opera-dswx-s1.json)) | Yes: Earthdata login (PO.DAAC) | As Landsat 8/9 | Product 1.0 from processing software 1.2 only; UInt8 layers WTR, BWTR, CONF and DIAG | [Landsat, HLS and OPERA](landsat-hls-opera.md) |
| OPERA surface disturbance alert from HLS (DIST-ALERT-HLS) v1 | `opera-dist-alert` ([example](../../examples/product-opera-dist-alert.json)) | Yes: Earthdata login (LP DAAC) | As Landsat 8/9 | One alert observation, not a disturbance history; DIST-ANN (annual) not supported | [Landsat, HLS and OPERA](landsat-hls-opera.md) |
| GOES-19 Advanced Baseline Imager (ABI) L2 Cloud and Moisture Imagery (CMI), contiguous United States (CMIPC), bands 13 and 1 | `goes19-abi-cmipc-c13`, `goes19-abi-cmipc-c01` ([example](../../examples/product-goes-abi.json)) | No (NOAA open data on AWS) | WGS84 bounding box or polygon (no antimeridian edges), or native row/column indices | GOES-19 Mode 6 CONUS only; one observation per request; catalog search spans at most one hour; L1b radiances, full disk and other ABI products not supported | [GOES, SMAP and NISAR](goes-smap-nisar.md) |
| GOES-19 ABI L2 CMI mesoscale-1 (CMIPM1), band 2 | `goes19-abi-cmipm1-c02` ([example](../products/wave4/goes-geographic-request.json)) | No (NOAA open data on AWS) | As GOES CMIPC | Inspected 2000×2000, 0.5 km mesoscale-1 grid only; one observation per request | [GOES, SMAP and NISAR](goes-smap-nisar.md) |
| SMAP SPL3SMP_E version 6 (release R19240), global AM and PM | `smap-spl3smp-e-v6-am`, `smap-spl3smp-e-v6-pm` ([example](../../examples/product-smap.json)) | Yes: Earthdata login (NSIDC) | WGS84 bounding box or polygon on the global 9 km Equal-Area Scalable Earth Grid 2.0 (EASE-Grid 2.0, EPSG:6933), or native indices | AM and PM stay separate; one observation per request; other releases and SMAP L4 not supported | [GOES, SMAP and NISAR](goes-smap-nisar.md) |
| SMAP SPL3SMP_E version 6 (release R19240), northern polar AM and PM | `smap-spl3smp-e-v6-north-am`, `smap-spl3smp-e-v6-north-pm` ([example](../products/wave4/smap-am-ordinary-request.json)) | Yes: Earthdata login (NSIDC) | WGS84 bounding box or polygon in the northern hemisphere, longitude not wrapping, on the northern 9 km EASE-Grid 2.0 (EPSG:6931), or native indices | AM and PM stay separate; one observation per request | [GOES, SMAP and NISAR](goes-smap-nisar.md) |
| NISAR L2 GCOV (geocoded covariance), provisional, frequency A, HHHH (HH-polarized backscatter power: horizontal transmit, horizontal receive) | `nisar-gcov-p1-fa-hhhh` ([geographic](../../examples/product-nisar-geographic.json), [native index](../../examples/product-nisar.json)) | Yes: Earthdata login (ASF) | WGS84 bounding box or polygon (at most 5° per axis) on the two inspected UTM zone 16N grids (10 m and 20 m), or native indices | Provisional product (productVersion 1.0.11, CRID P05023); Float32 linear gamma0 power, no dB conversion; other grids, HVHV, frequency B and complex covariance not supported | [GOES, SMAP and NISAR](goes-smap-nisar.md) |
| SWOT L2 HR Raster version D, 100 m UTM grid, processing release PGD0 (composite release identifier, CRID) | Job file [`examples/swot-job.json`](../../examples/swot-job.json), or a selection file such as [`examples/swot-aoi-selection.json`](../../examples/swot-aoi-selection.json) turned into a job by `discover-job` | Yes: Earthdata login (PO.DAAC) for data; catalog search with `discover` needs no account | WGS84 bounding box or polygon, at most 5° per axis, latitude within ±80°, not touching the antimeridian; or a native index window | Only release PGD0: forward-processed PID0 granules are rejected (`discover` marks them unsupported); 100 m grid only, no 250 m; one to four explicit observations per job, kept separate (not an aligned time series); five layers `wse`, `wse_uncert`, `wse_qual`, `wse_qual_bitwise`, `n_wse_pix`; other versions and SWOT products not supported | [SWOT Raster](swot.md) |
| ERA5 through WeatherBench2, 64×32 equiangular 6-hourly Zarr v2 store | [`examples/weatherbench2.json`](../../examples/weatherbench2.json) | No | WGS84 bounding box; every grid cell that overlaps it is selected | Only this exact public store; Float32 values; longitudes run 0–360° east with no wrapping, so give western longitudes as 180–360 and split a box that crosses 0° (a box at −10 to −1 does not fail but selects only the cell centred on 0°); other ERA5 archives, including ECMWF's ARCO Zarr, not supported | [ERA5, IMERG and GFS](zarr.md) |
| GPM IMERG daily through NASA POWER (the provider's own Zarr v2 store) | [`examples/imerg.json`](../../examples/imerg.json) | No | WGS84 bounding box | Only the NASA POWER daily spatial store; Float32 in source units (mm/day, %); other IMERG accumulations not supported | [ERA5, IMERG and GFS](zarr.md) |
| NOAA GFS analysis as Zarr v3, dynamical.org store v0.1.0 | [`examples/zarr-v3-gfs.json`](../../examples/zarr-v3-gfs.json) | No | Rectangle in the store's own coordinates and CRS (JSON request) | A derived Zarr published by dynamical.org, not NOAA's official GRIB2 archive; UInt16 or Float32 time-first arrays; no whole-shard decoding | [ERA5, IMERG and GFS](zarr.md) |
| Your own files: a local Zarr v2 store in Tile-Ripper's custom-grid layout, or a direct GeoTIFF asset | Request JSON with `"format": "zarr_v2"` or `"cog"` (see [`examples/zarr-request.json`](../../examples/zarr-request.json), [`examples/cog-request.json`](../../examples/cog-request.json)) | No | Rectangle in the file's own CRS | Zarr: immutable local directory store; GeoTIFF: self-contained tiled single-band UInt16 for the native reader; other layouts fail | [Local Zarr](../zarr-workflow.md), [native GeoTIFF](../native-cog/report.md) |

Where a row names two routes, they are separate: a public reference sample
does not prove access to the production archive, and the Planetary Computer and
NASA Earthdata copies of HLS are checked as different providers. Product
requests choose between them with `--provider auto|planetary_computer|earthdata|pinned`;
see the [provider routing report](../providers/qualification.md).

## Not supported

- AlphaEarth embeddings: a streaming prototype only, with no saved output
  ([prototype notes](../alphaearth/workflow.md)).
- ICESat-2 ATL03, ATL06 and ATL08, GEDI, and NISAR HVHV.
- Archived NetCDF-4/HDF5 files and SWOT Raster version C: narrow experiments
  only ([HDF5](../native-hdf5/workflow.md), [SWOT C](../swot-wse/report.md));
  they do not establish general support.
- Reprojection, resampling, mosaicking, averaging, compositing, longitude
  wrapping, implicit calibration and a universal cloud mask.
- Generic NetCDF, HDF5 or CF support; Python acquisition bindings; a scheduler.

Datasets not yet supported, and what each still needs, are graded in
[Roadmap and candidates](../products/roadmap.md). The [support contract](../alpha/contract.md)
links every row above to its detailed report.
