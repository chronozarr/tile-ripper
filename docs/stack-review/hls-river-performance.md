# HLS river profiling and performance fixes — 28 October 2026

Historical first performance follow-up. The later [HTTP timeline and validation-preserving HEAD consolidation](hls-river-http.md) measures actual request concurrency, removes two adjacent reader HEADs and reruns both ordinary defaults and a matched four-request control.

Tile-Ripper's runtime fell **25.4%** in contemporaneous interleaved old/new controls: median **22.71 → 16.93 seconds**. The same six-trial comparison with ordinary Python still favors Python: **13.70 s versus 17.47 s**, so Tile-Ripper takes **27.5% longer**. Values, masks, analysis and full source descriptors are unchanged.

The bottleneck was HTTP round trips. Output writing and durable publication were small. The retained raw archive and source checks remain part of the native workflow; none was removed to improve the timing. All work remains local and unreleased, with no commit, push or deployment.

## Measured causes and fixes

1. **Small payload requests across output chunks.** A first-window restriction prevented bounded selection buffering even when the union of planned reads already contained exactly the selection's TIFF tiles. The native reader now buffers at most 2 MiB when the exact tile union matches, then delivers the original chunks in order. It retains the window strategy when selection buffering would exceed the budget or reduce admitted concurrency. Payload GETs fell **180 → 142**. Payload bytes stayed exactly **13,810,214**: no extra science tiles, widening of gaps or unselected imagery were fetched.
2. **Connection pools rebuilt for identity checks.** Each identity pass previously created an async runtime/client separate from the native range client. One anonymous blocking HTTP pool now serves bounded HEAD batches and conditional TIFF reads. All three identity passes remain. Each HEAD still checks the current ETag/length; no object identity is cached. The transport retains bounded retries/timeouts and deterministic request-order errors, and rejects redirects consistently with native range reads. A loopback regression verifies four actual HEADs, including detection of a changed version, over one TCP connection.
3. **Two metadata GETs per observed HLS header.** The first remote parser read now fetches up to 8 KiB once and retains ordinary 4 KiB pages. These 32 source headers already required both pages, so their initial parser requests fall from two to one per file. Later metadata pages and local reads stay at 4 KiB. On other objects, read-ahead can fetch at most one unused extra 4 KiB page. The 512 KiB per-source retained cap and aggregate prepared-metadata cap remain; initial response plus split copies are bounded at 16 KiB with an otherwise empty cache. Metadata pages never satisfy science payload reads.

The selection diagnostics also now report retained bytes using the actual sample width, including Byte and Float32.

One diagnostic profile before and after all fixes gave:

| Stage | Before | After |
| --- | ---: | ---: |
| Identity HEAD batches, all three passes | 4.066 s | 3.499 s |
| Parallel TIFF metadata inspection | 3.325 s | 2.510 s |
| Stream incl. source validation and sink backpressure | 11.373 s | 9.283 s |
| Durable publication | 0.056 s | 0.041 s |

The stream row includes two identity passes, so these rows are not additive. Worker-summed source retrieval was **22.45 → 21.05 s**; these are concurrent worker totals, not wall time. The final scheduler working reservation peak was **3.57 MiB**, within the same 64 MiB request reservation. The profiles explain the work; the quiet matched trials below are the timing authority.

## Same benchmark rerun

The AOI, dates, bands, QA, 32 object pins and benchmark scripts are byte-for-byte identical to the [original experiment](hls-river-30day.md): T18MVS/Ucayali, July 1–30 2025, eight acquisition dates, B03/B04/B8A + Fmask, 333 × 332 native grid with 109,561 AOI pixels. Both paths still use four acquisition workers. Chunk size, encoder options, working reservation, analysis, crop fill and retained native archive are unchanged.

Six fresh-process trials ran serially in the original order Python, Tile-Ripper, Tile-Ripper, Python, Python, Tile-Ripper. They include signing/revalidation, acquisition, encoding and analysis. Common catalog freezing and builds are excluded; no builds, tests or competing experiment traffic ran during timing.

| Metric | Ordinary Python | Optimized Tile-Ripper |
| --- | ---: | ---: |
| Three wall times | 15.44, 12.87, 13.70 s | 16.85, 17.52, 17.47 s |
| Median wall time | 13.70 s | 17.47 s |
| Median sampled process-tree peak RSS | 242.7 MiB | 205.8 MiB |
| Median sampled run-directory peak size | 5.17 MiB | 43.67 MiB |

Memory is sampled at nominal 50 ms intervals and disk about once per second; neither is an exact maximum. The native directory still includes its roughly 36 MiB raw observation archive, playback and diagnostics. The test still compares complete workflows, not isolated codecs. Same machine and Python/GDAL/Chronozarr environment as the original receipt; the source and final binary hashes bind this revision.

## Controls for changing network conditions

The original report had medians 18.82 s native and 12.61 s Python. The rerun's Python observations varied from 12.87 to 15.44 s, so directly attributing a cross-session time difference to the code would be unreliable.

The preserved original native binary and optimized binary were therefore run in six additional fresh processes, interleaved **before1, after1, after2, before2, before3, after3**. Inputs, scripts, analysis and settings stayed unchanged. Both binaries received fresh signing and source checks. The optimized binary was restored and its hash verified afterward.

| Pair | Preserved original | Optimized | Runtime reduction |
| --- | ---: | ---: | ---: |
| 1 | 22.71 s | 16.19 s | 28.7% |
| 2 | 22.71 s | 17.05 s | 24.9% |
| 3 | 22.96 s | 16.93 s | 26.2% |
| Median | **22.71 s** | **16.93 s** | **25.4%** |

All three pairs improved. This supports a benefit under the measured connection conditions; public caches and latency were not controlled, and three repeats do not establish a universal speed claim. Tile-Ripper still has a gap to Python on the matched end-to-end workload. The remaining measured time is largely source metadata, identity validation and fragmented COG payload retrieval; it does not justify weakening source checks or publication guarantees.

## Fidelity, provenance and validation

- All six matched runs and all six controls completed successfully. All analysis JSON matches the original run. The optimized native store's **full 32 source descriptors**, not just ETags, equal the original descriptors.
- Independent readback against the fresh Rasterio baseline matched **3,318,672 stored samples** and **1,106,224 masks** across both pyramid levels, including cloudy raw samples and crop fill. All 32 raw asset arrays, QA bit calculations, native grid, calibration and timestamps agree. Both stores pass Chronozarr validation.
- Local Chromium static playback and queries passed on all eight dates for both stores, including invalid flags on the two fully masked dates, with zero browser errors and external traffic blocked. Screenshots were inspected.
- Rust validation passed **78 library tests + 135 binary/integration tests = 213**, with three existing fixture-dependent skips. New regressions cover misaligned selection, UInt8/Int16/Float32 raw bits including distinct NaNs and negative zero, exact tile coverage with masked gaps, tight-budget fallback, persistent connection reuse with fresh identity checks, bounded/clipped metadata reads, malformed/truncated responses and preparation poisoning.
- All-target native-Cog Clippy with warnings denied, formatting/diff checks and the no-default-feature build check passed. Independent read-only review found no blockers.

Preparation, pre-execution and post-execution identity checks, conditional payload checks, source failure invalidation, all QA rules, raw-value preservation, staging, filesystem synchronization and atomic publication remain in place. No source route, science resolution or band selection changed.

## Reproduction

The [machine-readable receipt](hls-river-performance-receipt.json) records frozen inputs, source/binary hashes, all profiles, six matched measurements, six native controls and validation. Full local outputs are under `prototype-output/hls-river-profile-20261028/`; the original experiment remains separate and intact. Preserved binaries are `tile-ripper-before`, `tile-ripper-middle` and `tile-ripper-after`.

Use the unchanged `bench/run_hls_river_30day.py --work WORK` and `bench/compare_hls_river_30day.py --work WORK` with a new directory containing the frozen request/protocol. `TILE_RIPPER_STAGE_TRACE=TRACE.jsonl` enables the existing diagnostic stage trace; profiling is excluded from final benchmark timing. The final native release binary SHA256 is `77afe5026e65530969ebf90235a13d0111774cbdd1ffa5c7c9aadefffd39276d`.
