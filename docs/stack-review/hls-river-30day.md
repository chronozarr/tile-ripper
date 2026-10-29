# Thirty-day HLS river workflow comparison — 28 October 2026

This is the original run. Subsequent [profiling](hls-river-performance.md) and [HTTP timeline/concurrency](hls-river-http.md) follow-ups preserve these artifacts and include contemporaneous old/new controls.

Ordinary Python completed this workflow faster. Both paths produced identical native science values, quality masks, index statistics and static playback behavior. Tile-Ripper's median runtime was **18.82 s**, versus **12.61 s** for Rasterio/NumPy: **1.49× the runtime**. Its sampled process-tree RSS was about **15% lower**. Its extra raw observation archive is useful when that archive is a requested deliverable; it added work and storage in this playback-first comparison.

The result supports **Python directly feeding Chronozarr for this HLS workflow**. Chronozarr's playback/query path works through either acquisition route. This experiment establishes no speed advantage or human usability advantage for Tile-Ripper. Keep product expansion held; a narrower acquisition role needs a concrete user requirement for retained raw observations, bounded decoding or its stronger provenance checks.

All changes and outputs are local and unreleased. No commit, push, publication or deployment was performed.

## What ran

The river AOI is a roughly 10 × 10 km window inside the saved Ucayali/Santa María demo: WGS84 bounds **[-75.075, -7.68, -74.985, -7.59]**. The full saved 25 km demo crosses a tile boundary; this experiment deliberately uses one complete native MGRS tile, **T18MVS**, with no mosaic. The test period is **2025-07-01 00:00:00 through 2025-07-30 23:59:59.999 UTC**, thirty calendar days with **eight actual acquisition dates**.

Both paths used the same **32 immutable object pins** from Planetary Computer `hls2-s30` v2: **B03 green, B04 red, B8A narrow NIR and Fmask** for each date. All catalog matches within the selected tile were retained, including fully cloudy dates. The complete parent-box catalog returned 15 features with no next-page link. The science grid was **333 × 332**, native 30 m, with **109,561 AOI pixels = 98.6049 km²**. The 995 envelope pixels outside the AOI are excluded from denominators and filled with source nodata in both deliverables.

Science remains Int16, physical reflectance = stored × 0.0001, offset zero. The source TIFF actually declares **EPSG:32618 with negative northings**; both outputs preserve those coordinates. QA rejects Fmask fill 255 and cloud, adjacency, shadow and snow bits 1–4, then intersects science validity and pixel-centre AOI inclusion. Water and aerosol flags remain admitted under `hls_fmask_clear`. The bit interpretation follows the [NASA HLS quality description](https://hls.gsfc.nasa.gov/algorithms/). No reprojection, resampling, mosaicking, compositing or gap filling occurred.

- **Tile-Ripper:** release native-Cog binary → completed per-observation Zarr → direct `convert_tile_ripper` → shared index analysis. It retains the raw observation store. There is no persistent stacked cube.
- **Ordinary Python:** standard Planetary Computer client + Rasterio/GDAL window reads + NumPy QA → the same Chronozarr encoder → the same index analysis. It streams one scene at a time, with four asset workers and persistent HTTP sessions. It verifies object ETag/length before and after conditional reads; it computes its AOI with PROJ/GDAL during the timed run.

Both encoders use 256-pixel cells, two pyramid levels, Zstd level 5 and one encoding worker. Native acquisition uses four workers/per-host slots and a 64 MiB working reservation; Python uses four workers and an 8 MiB GDAL cache. These settings are not process RSS limits.

## End-to-end results

Three fresh-process runs per path were executed serially in order Python, Tile-Ripper, Tile-Ripper, Python, Python, Tile-Ripper. No builds or competing experiment traffic ran during them. This is a **workflow comparison**, including each route's signing/revalidation, acquisition, final encoding and river analysis. Common catalog/pin freezing and software builds are excluded. Browser verification follows timing. Public server caches were uncontrolled; these are measurements on this Mac and connection, not a universal benchmark.

| Metric | Ordinary Python | Tile-Ripper |
| --- | ---: | ---: |
| Median wall time | 12.61 s | 18.82 s |
| Three wall times | 12.61, 12.57, 12.71 s | 18.77, 18.82, 19.02 s |
| Median sampled peak process-tree RSS | 238.5 MiB | 203.5 MiB |
| Median sampled peak run-directory size | 5.17 MiB | 43.67 MiB |
| Retained playback store | 4.49 MiB | 4.73 MiB |
| Additional retained raw observations | none | 35.96 MiB |
| Retained run files, including diagnostics | 107 | 1,238 |

The larger native playback metadata retains full source descriptors; both outputs retain all object pins. The native run-directory total also includes about 2.96 MiB of CLI diagnostic JSON. Memory samples cover the worker process and its children at nominal 50 ms intervals, and disk samples occur about once per second. They are observed estimates, not exact maxima. Sampled CPU is retained in the receipt as a lower-bound diagnostic; it does not support an efficiency claim. The machine has 16 logical CPUs and 128 GiB RAM; Python 3.13.17, GDAL 3.12.1, Rasterio 1.5.0, NumPy 2.4.4 and the local Chronozarr checkout were used.

This was automated execution by an agent, not a human trial. Setup effort was not assigned a fabricated time or usability score. The installed Python path worked; the local native path needed the two concrete fixes below before completing the same request.

## Scientific result and support

NDWI = (green − NIR)/(green + NIR); NDVI = (NIR − red)/(NIR + red). Both use calibrated reflectance and the shared clear mask; zero denominators are excluded. Positive NDWI is an exploratory water-index candidate, **not validated river area**. Per-date observed candidate areas below use only clear supported pixels and must not be read as full-AOI totals.

| Date | Clear fraction of AOI | Observed positive-NDWI area, km² |
| --- | ---: | ---: |
| 2025-07-03 | 1.62% | 0.6147 |
| 2025-07-08 | 97.25% | 14.2947 |
| 2025-07-10 | 83.65% | 10.8657 |
| 2025-07-13 | 99.31% | 13.3803 |
| 2025-07-18 | 0.00% | unavailable |
| 2025-07-20 | 96.03% | 12.5739 |
| 2025-07-23 | 14.64% | 0.2889 |
| 2025-07-30 | 0.00% | unavailable |

Four dates exceed an **exploratory 80% clear-AOI threshold**: July 8, 10, 13 and 20. Their common defined-NDWI support is **86,363 pixels = 77.7267 km²**. On those exact same pixels:

| Date | Positive-NDWI pixels | Candidate area, km² |
| --- | ---: | ---: |
| 2025-07-08 | 11,612 | 10.4508 |
| 2025-07-10 | 11,277 | 10.1493 |
| 2025-07-13 | 11,254 | 10.1286 |
| 2025-07-20 | 10,772 | 9.6948 |

The indicator decreases **7.23%** between July 8 and July 20 on that common support. Comparing the same pixels removes changing sample support as an explanation; residual QA errors can still affect the values. This does not establish river migration, discharge change or a validated water-area trend: no reference water labels or uncertainty model were used, and viewing/atmospheric/threshold effects remain. July 18 and July 30 have no accepted pixels; their areas are unavailable, not zero. Across the month, 109,508 AOI pixels were clear at least once; **53 AOI pixels were never clear**. No pixel is clear on all eight dates.

## Independent verification

All six final runs passed. All input/configuration, benchmark-script and release-binary hashes were identical across the six runs and remained unchanged afterward. Independent readback established:

- **3,318,672 stored Int16 samples** and **1,106,224 mask samples** match byte for byte across both pyramid levels, including cloudy raw samples inside the AOI and consistent outside-AOI fill.
- All **32 source asset arrays**, including eight raw Fmask arrays, match the independent Rasterio read hashes after the same polygon-crop fill. Native derived QA matches a separate NumPy bit calculation.
- Every retained source URL/ETag matches its frozen pin. Times, band order, CRS, transforms, scale, offset, units and analysis JSON agree; both playback stores pass `chronozarr validate`.
- Chromium opened each store over local static HTTP, navigated all **eight dates**, verified stored and physical click values on the six dates with clear pixels, and reported invalid pixels on the two fully masked dates. Both completed with **zero browser errors**, with external requests blocked.

The browser proof covers native-band playback and query values. NDWI/NDVI statistics are in the analysis files; the source catalog does not label B8A with a common name, so this run does not claim the viewer's index presets are enabled. Screenshots were inspected and show the July 13 river reach and value sidebar.

## Defects found while running

The original request failed before payload acquisition because signing each asset separately reached **HTTP 429**. A preparation-local cache now reuses only verified same-container read/list grants; it revalidates canonical object, grant and expiry for every target. Exact-object blob grants remain exact-object scoped. Bearer signatures are not persisted and no provider fallback was added.

A two-asset diagnostic then exposed the HLS provider's southern-grid convention. A product-private inspected-source gate now checks HLS v2 item/tile/band, immutable ETag, physical CRS, TIFF `HORIZONTAL_CS_CODE`, dimensions, spacing, origin and MGRS frame before allowing the native northern-code/negative-northing selection. Ordinary hemisphere guards remain; no CRS normalization or data warp occurs.

These changes passed **69 Rust library tests**, **25 independent stack regressions**, and native-Cog **all-target Clippy with warnings denied**. The five added unit tests cover grant reuse/scope/expiry/rejection and physical southern-grid admission/rejection. Benchmark scripts passed Ruff/syntax checks and their actual full-run/readback checks.

The exploratory first comparison also identified the outside-AOI fill difference. All in-AOI science samples and all masks already agreed. Python's crop fill was aligned to the qualified native output, and all six reported trials were rerun after that change. Original failures and exploratory results remain as separate local diagnostics and are excluded from these medians.

## Reproduction and retained artifacts

[The machine-readable receipt](hls-river-30day-receipt.json) contains the frozen request/protocol, all 32 pins, six raw measurements, exact checks, environment, browser results and code/binary hashes. The native release binary SHA256 is `ddb79da97c16cdc26501101a269af2335c492cdde9e6eef8da61a645c7df1d1d`.

The experiment lives in `prototype-output/hls-river-30day-20261028/` (Git-ignored): `comparison.json`, `measurements.json`, `viewer-receipt.json`, and the six `final-*` output directories. Each contains `playback/`, `analysis.csv`, `analysis.json` and `water-index-support.npz`; native runs also retain `observations.zarr/` and `fetch.log`. Browser screenshots are `final-tile-1-browser-input.png` and `final-python-1-browser-input.png`.

`bench/hls_river_30day.py prepare --work NEW_DIRECTORY` freezes a fresh selection and source identities; it refuses to overwrite an existing protocol. Alternatively, copy `protocol` and `request` from the receipt into a new work directory to repeat these exact pins. `bench/run_hls_river_30day.py --work WORK_DIRECTORY` performs the serial final trials with a Python environment containing the sibling editable Chronozarr checkout, Rasterio, NumPy, requests, pyproj, planetary-computer and psutil. Then run `bench/compare_hls_river_30day.py --work WORK_DIRECTORY` and the two generated receipts through `bench/stack_review_viewer.mjs`. A native release binary must first be built with `cargo build --release --locked --no-default-features --features native-cog --bin tile-ripper`. Archive availability and the original pins are required; the experiment does not substitute changed objects.
