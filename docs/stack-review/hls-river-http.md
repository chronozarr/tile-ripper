# HLS HTTP timelines, concurrency and identity-check consolidation — 28 October 2026

This report retains the earlier four-request control. The subsequent [matched sixteen-request experiment](hls-river-http16.md) gives both paths a sixteen-request ceiling, verifies actual sixteen-request payload peaks and includes contemporaneous four-request controls.

Removing two adjacent reader HEADs cut Tile-Ripper's median runtime **15.8%** in contemporaneous interleaved old/new controls: **13.01 → 10.95 seconds**. In the original ordinary-default comparison, the new native and Python medians were effectively tied: **10.88 versus 10.92 seconds**. With actual source-request concurrency capped at four for both paths, native took **0.63 seconds / 6.0% longer**: **11.08 versus 10.45 seconds**. These are complete workflow timings on the same frozen sources, including the native raw archive and its conversion to playback.

All 18 timed runs preserve the original values, masks, analysis, calibration, grid and source provenance. Work remains local and unreleased.

## What the HTTP timelines established

Four application workers did not imply four HTTP requests. GDAL's default multi-range reads reached **16 concurrent payload requests** in the final HLS trace; an earlier trace reached 15. Native requests peaked at four. The trace also confirmed five native HEADs per file before the change, including two reader checks immediately following checks at the acquisition layer.

| Live source trace | HEAD | Header GET | Payload GET | Peak requests | Payload bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Native before this change | 160 | 32 | 142 | 4 | 13,810,214 |
| Native after this change | 96 | 32 | 142 | 4 | 13,810,214 |
| Ordinary Python defaults | 96 | 32 | 136 | 16 | 13,810,214 |
| Python with connection cap | 96 | 32 | 136 | 4 | 13,810,214 |

Python's 96 HEADs comprise 64 explicit before/after checks and 32 GDAL opens. Native's remaining 96 comprise preparation, pre-execution and post-execution checks. Phase timing differs between these workflows. Python transferred 524,288 header bytes; native transferred 262,144. All selected payload byte counts agree. Native's complete GET interval multiset is unchanged by this fix; Python's complete request interval multiset is unchanged by its connection cap.

Native and requests timelines span dispatch through response consumption; HEADs end at received headers. A macOS benchmark-only libcurl interposer records each GDAL HEADER_OUT and verifies its final received body timestamp against Content-Length and libcurl's downloaded-byte count. HEAD endpoints use final response-header completion. API completion is recorded separately. It preserves libcurl's completion queue. Every retained live GDAL transaction qualified for exact body timing, with zero ambiguous, pending or unpaired transfers. Both ordinary and capped GDAL traces negotiated HTTP/1.1. Signing is included in workflow wall time but excluded from these pinned-source timelines. Only safe file basenames, numeric ranges, flags, status, byte counts and clocks are retained; no signed queries, raw headers or bodies are logged.

The interposer was checked against an independent local HTTP server: both measured 16 default requests versus four capped requests, with exact image readback and identical request ranges/bytes. Reused, reset and duplicated handles and completion-message behavior passed. Truncation, unknown length, encoding, chunking and redirects deliberately fail exact-timing qualification.

## Matching actual request concurrency

The control retains `GDAL_HTTP_MULTIRANGE=YES`, sets `GDAL_HTTP_VERSION=1.1`, `GDAL_HTTP_MAX_TOTAL_CONNECTIONS=1` per GDAL worker connection cache, and `GDAL_HTTP_MULTIPLEX=NO`. With four asset workers, both live paths peak at four requests. The cap queues the existing range requests. GDAL's [multi-range options](https://gdal.org/en/stable/user/configoptions.html#config-GDAL_HTTP_MULTIRANGE) allow internal range parallelism, while libcurl's [connection limit](https://curl.se/libcurl/c/CURLMOPT_MAX_TOTAL_CONNECTIONS.html) applies per multi handle and can still allow multiplexing; HTTP/1.1 makes this control explicit.

An exploratory `GDAL_HTTP_MULTIRANGE=SERIAL` run also peaked at four, but changed the read strategy and increased HLS payload responses to 16,465,920 bytes. It is excluded from the matched timing comparison. The selected connection-cap control retains all default Python request intervals and bytes exactly. The original benchmark scripts and default experiment remain unchanged; controls use separately named wrappers and output directories.

## Validation-preserving implementation

Acquisition now passes a private, immutable identity receipt to inspection and creates a separate receipt view for each execution. Each receipt binds the exact asset ID, runtime locator, fully strong ETag and positive length from an HTTP-200 HEAD. This replaces the reader's adjacent anonymous HEAD with evidence already obtained at the same lifecycle boundary. Prepared state retains its original identities; a fresh pre/post pass compares both ETag and length. Inspection receipts are cleared before the prepared object is retained. Overlapping executions own separate receipt views while sharing existing metadata and the preparation failure latch.

The three lifecycle HEAD passes remain fresh. Every metadata and payload GET retains If-Match, status, ETag, complete Content-Range including object length, encoding and bounded-body checks. Metadata layout comparisons, terminal failure invalidation, repeated-execution validation, cancellation, staging, filesystem synchronization and atomic publication remain. Standalone readers without a receipt retain their HEAD fallback. Authenticated sessions keep their existing session validation and reader HEADs. There is no persisted or process-global identity cache and no new public API or dependency.

Independent review caught that the old identity helper accepted any successful 2xx HEAD while the replaced reader required 200. Receipts now require exactly 200; plausible 204/206 responses fail before receipt construction. New tests cover this, zero length, same-ETag length drift, exact binding mismatch, repeated and simultaneous execution, no retained metadata pages, bad GET poisoning, fresh postvalidation and standalone fallback. Review found no remaining blocker. A direct test of an authenticated reader ignoring an anonymous receipt remains absent; the explicit session branch retains its existing HEAD path, and the authenticated feature suite passed.

## Quiet timed reruns

AOI, dates, bands, QA, all 32 source pins and encoder/working-reservation settings are byte-identical to the [original experiment](hls-river-30day.md): Ucayali/T18MVS, July 1–30 2025, eight acquisitions, B03/B04/B8A plus Fmask, 333 × 332 native grid, 109,561 AOI pixels. There is no resampling or gap filling. Each set uses fresh processes in the original route order Python, native, native, Python, Python, native. Trace injection, builds, tests and competing experiment traffic were disabled during timing.

| Three-run workflow set | Python wall times | Native wall times | Python median | Native median |
| --- | --- | --- | ---: | ---: |
| Original default settings | 11.09, 10.92, 10.90 s | 13.12, 10.88, 10.73 s | 10.92 s | 10.88 s |
| Matched four-request cap | 10.68, 10.45, 10.36 s | 11.08, 13.05, 10.99 s | 10.45 s | 11.08 s |

The default difference is 0.4%, too small to claim a native speed advantage. The capped set leaves native 6.0% slower. A higher default request peak does not establish that it caused the earlier wall-time gap: capped Python was also fast in this set. Public network conditions varied, and three repeats per path do not establish a universal performance ranking.

To isolate the code change under current conditions, the preserved previous native binary and new binary were run in the interleaved order before1, after1, after2, before2, before3, after3, with identical recipe/control wrappers:

| Pair | Before | After | Runtime reduction |
| --- | ---: | ---: | ---: |
| 1 | 14.58 s | 10.95 s | 24.9% |
| 2 | 13.01 s | 11.06 s | 15.0% |
| 3 | 12.52 s | 10.54 s | 15.8% |
| Median | **13.01 s** | **10.95 s** | **15.8%** |

All three pairs improved. This supports a benefit from eliminating the 64 duplicate HEADs. Earlier 17.47-second native and 13.70-second Python medians remain historical measurements in the [previous report](hls-river-performance.md); cross-session differences cannot all be attributed to this patch.

Native still retains its roughly 36 MiB raw observation archive. Its default sampled peak RSS was 203.0 MiB versus Python's 236.7 MiB, and sampled run-directory size was 43.67 versus 5.17 MiB. The bridge's median was about 0.51 seconds; archive writing and this conversion remain work absent from the direct Python path. These costs are consistent with a small remaining difference, but the stage medians do not establish an additive causal decomposition.

## Readback, playback and checks

Every one of the 18 timed outputs was independently reopened and compared with the original native output at both pyramid levels: **3,318,672 stored samples and 1,106,224 mask samples per run**, including cloudy raw values and crop fill. Total repeated checks were 59,736,096 values and 19,912,032 masks. All analysis JSON equals the original; all playback stores validate. Each of 12 native runs also passed all 32 independent Rasterio raw-array/QA checks and exact full-source-descriptor comparison: 384 raw-asset checks and 384 full descriptor checks across repeats. Native times, CRS, transform, calibration, units, object pins and manifest binding are retained.

Local Chromium rendering/navigation/pixel queries passed all eight dates for both paths in both comparison sets, including invalid flags on the two fully masked dates: 32 timestep queries, zero browser errors, external requests blocked. Screenshots were inspected; offline NDWI/NDVI analysis remains identical, and index presets are not newly enabled in the viewer.

The native feature suite passed **221 tests**, with three existing fixture-dependent skips. The combined native-COG/native-HDF5/authenticated feature suite passed **283**, with eight existing skips. Warning-denying all-target Clippy for that combined build, no-default-feature checking, owned Rust formatting, Python lint/format and diff checks passed.

One completed Python diagnostic is excluded because the unchanged sampler was given a relative work path and failed while recording its input hashes. The full six-trial sequence was restarted with an absolute path and fresh `matched-*` names. All reported timed measurements completed successfully.

## Evidence and reproduction

The [machine-readable receipt](hls-river-http-receipt.json) binds source and binary hashes, HTTP summaries, frozen inputs, all 18 measurements, all-run verification, browser results and checks. Raw artifacts remain in `prototype-output/hls-river-http-20261028/`; prior experiments are intact. The new binary SHA256 is `a63f38090a89fd4d8500aaf857d10b0679333fe3fd76ee51cb2a44244984aba8`.

Use an **absolute** new work directory containing exact copies of `protocol.json`, `request.json` and `run-conventions.json`. `bench/run_hls_river_30day.py` reproduces ordinary defaults; `bench/hls_http_benchmark.py --mode capped` applies the separately labeled four-request control through the same sampler and recipe. `bench/hls_http_worker.py` plus `TILE_RIPPER_HTTP_TRACE` or the benchmark-only `bench/http_curl_trace.c` records diagnostic timelines; do not inject tracing into timing runs. `bench/summarize_hls_http.py`, `bench/compare_hls_river_30day.py` and `bench/verify_hls_http.py` provide readback and request accounting. Preserved before/after binaries support the interleaved control.
