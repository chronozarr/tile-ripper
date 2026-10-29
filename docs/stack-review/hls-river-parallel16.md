# Python acquisition across scenes at sixteen HTTP requests — 28 October 2026

With both paths using sixteen asset workers across scenes under the same sixteen-request HTTP ceiling, **Python completed in 4.66 seconds median versus Tile-Ripper's 5.06 seconds**. Python used **7.9% less elapsed time**; native took 8.5% longer, a difference of about 0.40 seconds. Python's median fell **50.8%**, from 9.48 seconds with its previous scene-at-a-time scheduling in contemporaneous controls. Scheduling explains much of the earlier native advantage.

| Workflow, HTTP ceiling sixteen | Three fresh-process wall times | Median | Median sampled peak process-tree RSS |
| --- | --- | ---: | ---: |
| Previous Python: four asset workers, four GDAL connections each | 9.44, 9.48, 10.25 s | 9.48 s | See receipt |
| Python: sixteen asset workers across scenes, one connection each | 4.56, 4.66, 4.78 s | **4.66 s** | 277.6 MiB |
| Tile-Ripper: sixteen asset workers, per-host limit sixteen | 5.06, 5.00, 5.17 s | **5.06 s** | 209.1 MiB |

All three parallel Python observations were faster than all three native observations. These are three repeats per configuration on public network/storage, so the approximately 0.40-second difference does not establish a universal performance ranking. Native used about 25% less sampled peak RSS and still retains its additional raw observation archive and conversion step.

## Execution changes

The original Python science recipe is unchanged. A small executor adapter, `bench/hls_scene_pool.py`, submits source assets across scene boundaries using sixteen workers. It calls the original `read_asset` closure, preserving every source HEAD, ETag/size check, conditional GDAL read, type/grid/calibration assertion and exception. Original QA, polygon fill, source hashes, scene/band order, provenance and Chronozarr encoding run unchanged.

Each worker is restricted to one HTTP/1.1 connection, with GDAL multi-range planning retained (`MULTIRANGE=YES`, `MAX_TOTAL_CONNECTIONS=1`, `HTTP_VERSION=1.1`, `MULTIPLEX=NO`). Explicit HEADs and GDAL reads run sequentially within each worker. This permits sixteen requests across metadata and acquisition without nested range parallelism exceeding the cap.

At most 32 pending futures are retained. All 32 frozen asset jobs can be queued so a slow early scene or chronological encoding cannot prevent other workers from starting later scenes. Selected raw arrays for the whole frozen dataset occupy at most about 5.9 MiB, excluding the existing encoder/QA scratch and library allocations. The GDAL cache remains 8 MiB. Results are delivered strictly in the original chronological scene and B03/B04/B8A/Fmask order. Failures propagate and pending jobs are cancelled on shutdown. Checks demonstrate sixteen simultaneous workers, acquisition progressing beyond four scenes before ordered output, bounded retention, exact output order and terminal failure propagation.

Native production code and binary remain unchanged at SHA256 `a63f38090a89fd4d8500aaf857d10b0679333fe3fd76ee51cb2a44244984aba8`. Its sixteen-worker/per-host setting, original 64 MiB working budget, raw archive, validation and durable publication remain. No Rust tests were rerun for these benchmark-only changes.

## Actual HTTP traces

| Successful live trace | HEADs | Header GETs | Payload GETs | HEAD peak | Header GET peak | Payload peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Python across scenes | 96 | 32 | 136 | 16 | 15 | 16 |
| Tile-Ripper | 96 | 32 | 142 | 16 | 16 | 16 |

Both paths actually reached sixteen concurrent source requests and never exceeded sixteen. Header GET overlap reached fifteen in the Python sample because individual requests completed at different times; metadata execution has sixteen workers. Python's 200 GDAL transfers and 64 explicit HEADs are completely paired. Every GDAL transfer qualified for exact body-completion timing, with zero issues, inexact endpoints or pending transfers. Native's 270 transactions also completed with expected HTTP status.

Each path's request interval multiset and transferred bytes match the previous sixteen-request experiment exactly. Payload responses remain **13,810,214 bytes for each path**. Python header responses total 524,288 bytes; native headers total 262,144 bytes. No source object, tile range or validation step was omitted to obtain the improvement. Signed queries, raw headers and bodies are absent from trace logs. Diagnostic tracing was disabled for all timed measurements.

An initial sixteen-future prototype was retained as a diagnostic only. The final 32-future bound keeps the full frozen job queue available to the sixteen workers while chronological output waits. Only the final implementation was timed.

## Same frozen experiment and controls

The protocol, request and run conventions are exact copies of the original Ucayali/T18MVS HLS S30 v2 experiment: July 1–30 2025, eight acquisition dates, 32 pinned assets, B03/B04/B8A and Fmask, native 333 × 332 grid, 109,561 AOI pixels. All QA, fill, actual source CRS/coordinates, scale/offset, nodata and source identity pins remain unchanged. Encoding retains 256-cell chunks, two levels, Zstd level 5 and one encoder worker. Explicit runtime controls override the source protocol's original concurrency preset.

Six primary fresh-process trials retain the original route order Python, native, native, Python, Python, native. Three previous-Python controls are interleaved, alternating which Python configuration runs first. The receipt records the exact nine-run order and commands. Signing, source checks, acquisition, encoding and analysis are included. Builds, tests, tracing and competing experiment traffic were absent during timing. Both old and new Python processes use the same current harness and original source-read/science functions.

The Python acquisition timer now measures waits for ordered scene results after asynchronous prefetch; it is not total worker time and cannot be directly compared with the earlier scene-at-a-time stage timer. End-to-end process wall time is the timing authority. The runtime gap is consistent with additional native archive/bridge work, but stage timing does not establish an exact causal decomposition.

## Fidelity, provenance and playback

All nine timed stores were reopened and compared against the original native output at both pyramid levels. Each matched **3,318,672 stored samples and 1,106,224 masks**, including cloudy raw values and polygon fill. Across repeats, **29,868,048 values and 9,956,016 masks** matched exactly. All analysis JSON equals the original, and all playback stores validate.

All six Python runs' source-check records also exactly equal the original Python source-check records: **192 asset hash comparisons**, including original window values, filled science/QA arrays and derived masks. Each of the three native runs passed all 32 independent raw-array/QA checks and exact full-source-descriptor comparison: **96 native raw checks and 96 full descriptor checks**. Dates, grid, calibration, units, pins and manifest binding remain intact. This retains the original scientific and provenance guarantees while changing when independent reads are scheduled.

Local browser navigation, rendering and stored/physical pixel queries passed all eight dates for both primary stores, including invalid flags on the two fully masked dates: sixteen timestep queries, zero browser errors, external traffic blocked. Screenshots were inspected. Offline NDWI/NDVI analysis remains identical; viewer index presets are not newly enabled.

Pool concurrency/order/failure checks, Python lint/format, diff checks, exact request accounting, all-run readback and browser checks passed. No commit, push, publication or deployment was performed.

## Evidence and reproduction

The [machine-readable receipt](hls-river-parallel16-receipt.json) binds all nine measurements, frozen inputs and code/binary hashes, HTTP summaries, source/raw verification, pool checks and browser results. Full artifacts are in `prototype-output/hls-river-parallel16-20261028/`; previous experiments remain intact.

Use a fresh **absolute** work directory containing the frozen protocol/request/conventions. `bench/hls_http_benchmark.py --mode parallel16 --receipt measurements.json` applies sixteen-way cross-scene acquisition on both routes. `--mode sixteen` retains the previous four-worker/four-connection Python control. `bench/verify_hls_http.py --measurements measurements.json controlled-measurements.json` verifies both sets against the original. `bench/check_hls_scene_pool.py` checks ordering, bounded prefetch and errors. HTTP tracing runs separately from timing.
