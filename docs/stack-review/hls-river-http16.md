# Matched sixteen-request HLS benchmark — 28 October 2026

Historical result with Python still processing one scene at a time. The later [sixteen-way Python metadata/acquisition rerun](hls-river-parallel16.md) parallelizes source work across scenes under the same HTTP ceiling and changes the measured ranking. These earlier observations remain intact.

At the same **sixteen-request HTTP ceiling**, Tile-Ripper completed the frozen HLS river workflow in a median **5.16 seconds**, versus **9.85 seconds** for ordinary Python: **47.6% less elapsed time**, or **1.91× throughput** on this workload. Separate live traces confirmed that both paths actually reached sixteen successful concurrent payload requests. Science values, QA masks, analysis and full source provenance match the original experiment.

| HTTP ceiling | Ordinary Python median | Tile-Ripper median |
| --- | ---: | ---: |
| Four, contemporaneous control | 11.45 s | 12.30 s |
| Sixteen | **9.85 s** | **5.16 s** |

Increasing the ceiling from four to sixteen reduced native runtime **58.0%** and Python runtime **14.0%** in these interleaved measurements. Three fresh processes were measured for each route/ceiling combination. This is a live-network workflow result, not a general language or decoder performance claim.

## Same science, higher HTTP capacity

The protocol, request and conventions are byte-for-byte copies of the previous frozen experiment: Ucayali/T18MVS, July 1–30 2025, eight acquisition dates, 32 pinned HLS S30 v2 objects, B03/B04/B8A plus Fmask, 333 × 332 native grid and 109,561 AOI pixels. QA, outside-AOI fill, actual source CRS/coordinates, calibration, nodata and source identity checks are unchanged. Encoding still uses 256-cell chunks, two levels, Zstd level 5 and one encoder worker. The native 64 MiB working budget, GDAL 8 MiB cache, retained native raw archive and Chronozarr conversion remain part of the comparison.

The source protocol retains its original four-worker preset; a separate runtime-control record explicitly overrides only concurrency. Native uses the existing `--workers 16 --per-host 16` options. Python retains its original four asset workers and allows four GDAL HTTP/1.1 connections per worker with `GDAL_HTTP_MULTIRANGE=YES`, `GDAL_HTTP_MAX_TOTAL_CONNECTIONS=4`, `GDAL_HTTP_VERSION=1.1` and `GDAL_HTTP_MULTIPLEX=NO`. Each worker performs its explicit HEADs and GDAL operations sequentially, so four workers with at most four active GDAL connections each stay within sixteen source requests. The four-request control uses native workers/per-host four and one GDAL connection per Python worker.

No production Rust source or binary changed. The binary remains `a63f38090a89fd4d8500aaf857d10b0679333fe3fd76ee51cb2a44244984aba8`, including the previous validation-preserving HEAD consolidation. Changes are limited to benchmark controls, diagnostic instrumentation and reports. Work is local and unreleased.

## Actual HTTP behavior

| Successful live trace | HEAD | Header GET | Payload GET | Header bytes | Payload bytes | Payload peak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Tile-Ripper, sixteen | 96 | 32 | 142 | 262,144 | 13,810,214 | 16 |
| Python, sixteen | 96 | 32 | 136 | 524,288 | 13,810,214 | 16 |

Each path's complete request interval multiset and body-byte total exactly match its previous four-request trace. No extra selected imagery, broader payload ranges or omitted source objects produced the speed difference. Every retained GET is conditional. Both traces contain complete successful responses; GDAL's 200 transfers are individually paired with verified body-completion timestamps and zero pending, ambiguous or inexact transfers. Python's 64 explicit HEADs are recorded separately. Signed queries, raw headers and bodies are absent from diagnostic logs.

The ceiling gives each implementation equal maximum HTTP capacity; their scheduling remains different. Native peaked at sixteen for HEADs, TIFF headers and payloads. The original Python recipe peaked at four for HEADs and headers, and sixteen during multi-range payload reads. Native schedules across the 32 source assets, while Python retains its scene-at-a-time recipe. The source-timeline mean number of active transactions was about **11.8 native versus 4.9 Python** in the diagnostic runs. Native therefore uses more of the permitted capacity across more phases. This result does not compare against a redesigned Python pipeline that prefetches several scenes simultaneously.

Native transaction intervals span dispatch through response consumption; GDAL intervals use HEADER_OUT through verified final received body bytes. Those boundaries differ slightly, so the averages describe scheduling rather than an identical wire-level utilization metric. Both measured peaks are sixteen. Tracing was disabled for every timed run.

The first native diagnostic recovered one brief HEAD transport failure through the existing bounded retry path; its completed output is retained, but the clean confirmation trace supplies the table above. The first Python diagnostic exposed a race in the benchmark-only curl interposer: it removed tracking state after freeing a handle, allowing another thread to reuse the address before removal. The interposer now detaches the old state before cleanup and keeps its allocation alive through cleanup, with diagnostic callbacks disabled for the finished handle. This respects libcurl's [cleanup lifetime](https://curl.se/libcurl/c/curl_easy_cleanup.html); libcurl also documents that [debug callbacks can involve internal handles](https://curl.se/libcurl/c/CURLOPT_DEBUGFUNCTION.html), which remain explicitly guarded. Neither the original incomplete trace nor tracing overhead contributes to the timing result.

A new local stress check passed 96 concurrent tiled-TIFF opens/reads, 576 paired HTTP transfers and 47,045,760 body bytes with exact image readback, zero tracking issues and matching independent server counts, bytes and observed overlap peak. A simpler 288-transfer reuse check also passed. The corrected live Python trace then qualified every transfer at the sixteen-request ceiling. Earlier diagnostic artifacts and the previous tracer binary remain intact.

## Interleaved fresh-process measurements

For each original route/repetition slot, a four-request and sixteen-request run were paired, alternating which ceiling ran first. The resulting twelve-run order is recorded in the receipt. Within either ceiling, the original route order remains Python, native, native, Python, Python, native. Every process performs fresh signing, source validation, acquisition, encoding and analysis. Builds, tests, tracing and other experiment traffic were absent during timing.

| Route and ceiling | Three wall times | Median | Median sampled peak process-tree RSS |
| --- | --- | ---: | ---: |
| Python, four | 12.02, 11.45, 11.43 s | 11.45 s | 234.6 MiB |
| Python, sixteen | 10.50, 9.85, 9.73 s | 9.85 s | 235.0 MiB |
| Native, four | 12.48, 11.93, 12.30 s | 12.30 s | 203.3 MiB |
| Native, sixteen | 5.33, 5.16, 5.07 s | 5.16 s | 209.0 MiB |

All three native sixteen-request observations were faster than all three Python sixteen-request observations. All three paired increases in the HTTP ceiling improved both routes. Public caches and network conditions remain uncontrolled; these observations support this bounded workload and environment rather than a universal speed ratio. The four-request controls are contemporaneous, so the concurrency comparison does not depend on earlier sessions' timings.

Native acquisition's median fell from 11.06 to 4.03 seconds; the Python acquisition timer fell from 10.23 to 8.53 seconds. These route-specific stage timers exclude different setup work and are not directly interchangeable. Native still writes its approximately 36 MiB raw observation archive and converts it to playback. Its retained run-directory size remains roughly 43.67 MiB; Python retains about 4.51 MiB. The diagnostic native scheduler admitted all sixteen workers and reserved approximately 12.4 MiB within the unchanged working budget. That working reservation is not an RSS limit.

## Fidelity and provenance

All twelve timed stores were reopened and compared against the original native result at both pyramid levels. Each run matched **3,318,672 stored samples** and **1,106,224 mask samples**, including cloudy raw values and polygon crop fill. Across repeats, that is **39,824,064 value checks and 13,274,688 mask checks**. All analysis JSON equals the original, and every playback store passes Chronozarr validation.

Each of the six native outputs also passed all 32 independent Rasterio raw-array and QA checks and exact comparison of all 32 full source descriptors: **192 raw-asset checks and 192 full descriptor checks** across runs. Dates, source URL/ETag identity, native CRS/transform, calibration, units, QA interpretation and manifest hash binding remain intact. The existing fresh preparation/pre/post source checks, conditional range validation, failure invalidation and durable publication are unchanged.

Local Chromium playback, temporal navigation and pixel queries passed on all eight dates for both sixteen-request stores, including the two fully masked dates: sixteen timestep queries and zero browser errors, with external requests blocked. Screenshots were inspected. Offline NDWI/NDVI results are unchanged; viewer index presets are not newly enabled.

The production source and binary hashes still match the previously tested revision. Rust tests were not rerun for benchmark-only changes. The corrected diagnostic C library compiles with `-Wall -Wextra -Werror`; local stress checks, Python lint/format, diff checks, successful live traces and full retained-output verification passed.

## Evidence and reproduction

The [machine-readable receipt](hls-river-http16-receipt.json) contains all twelve measurements, both HTTP summaries, frozen source/binary hashes, control settings, request accounting, local tracer checks, all-run readback and browser results. Full artifacts are retained in `prototype-output/hls-river-http16-20261028/`; previous experiments remain separate.

Use a fresh **absolute** work directory containing the frozen `protocol.json`, `request.json` and `run-conventions.json`. `bench/hls_http_benchmark.py --mode sixteen --receipt measurements.json` applies the sixteen-request control through the original sampler and recipe; `--mode capped` applies the four-request control. The exact interleaving and commands are recorded in `measurement-order.json` and per-trial control receipts. `bench/verify_hls_http.py --measurements measurements.json controlled-measurements.json` checks both sets against the original. HTTP diagnostics use separate runs; never inject the curl interposer or native HTTP tracing into timing trials.
