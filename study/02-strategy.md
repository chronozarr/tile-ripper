# 02 — Fork vs compose, and differentiation (Deliverables 2 and 4)

Evidence tags `[N0x]` → `evidence/0x-*.md`; the evidence grades are defined in [01](01-ecosystem.md).

---

## 1. The three alternatives

| | **A. Fork lazycogs, replace Python orchestration** | **B. Standalone Rust engine on existing low-level crates (+ upstream contributions)** | **C. Extend an existing Rust project directly** |
|---|---|---|---|
| **Code reuse** | **Near zero in Rust.** lazycogs is 100% Python (GitHub: 370 KB Python, no Rust); warp is pyproj+numpy; window math is in async-geotiff (Python); only async-tiff has Rust. Nothing to "extract" `[N01 §1]`. Reusable *ideas*: `explain()` dry-run, ordered-drain early-stop first-valid mosaic, warp-map cache keyed on window transform+CRS, item-level dtype/nodata contract checks, numpy mosaic semantics as a test spec | **High, at the right layers**: `object_store` (wrap), `stac`/`stac-io` (types + client), async-tiff (header/tile index; upstream), `zarrs` (sink, pinned), `libdeflater`, OpenJPEG, `proj`, `gdal-sys` (warp). Design ideas from Rasteret (layout index schema), sentle (overlap dedupe), odc-geo (`_can_paste`), AWS TM (hedging design) | **Only fragments**: no existing Rust project is an engine. async-tiff is a reader; rustac is discovery; object_store is I/O; zarrs is storage |
| **Architectural fit** | **Poor.** Chunk-centric (xarray lazy backend, one DuckDB `ST_Intersects` per chunk per step through a single-worker pool, one asyncio task per item, 32-slot per-chunk semaphore), no cross-chunk header cache, whole chunk in RAM on `.compute()`, no sink concept `[N01 §2]`. Fundamentally the opposite of a global plan | **Designed for it**: stage separation, plan-as-value, sink-driven ordering, memory budget ([03](03-architecture.md)) | Adding a scheduler/planner/sinks/warp to async-tiff would change a reader library's scope; its wheel underpins lazycogs, async-geotiff and virtual-tiff, so API churn affects them |
| **Performance ceiling** | Capped by the Python warp (PR #58 benchmarks show numpy warp dominates wall time; the Rust-warp PR depends on a dormant AI-written crate), by async-tiff decode (~1.7× slower than GDAL at 16 threads, FOSS4G 2025) and by the GIL/asyncio orchestration `[N01 §1,4]` | Set by network and decode physics; decode via C libraries (libdeflate, OpenJPEG); warp delegated to GDAL until proven a bottleneck. Cheap very-high concurrency is structural (async, not thread-per-request) `[INFER]` | Same ceiling as B for the specific layer improved; no end-to-end gain unless the layer is the bottleneck |
| **Maintenance burden** | Inherit a stalled single-maintainer project (13 releases in 9 weeks, then silence; ~440 downloads/month; "an experiment" per its authors; issue #89 proposes merging into odc-stac) and then **diverge**; rewriting 4.2k LOC of Python into Rust is not a fork | **Highest direct cost**, bounded by explicit non-goals. Three single-maintainer dependencies (async-tiff, rustac, zarrs) isolated behind traits | Lowest cost per change, but upstream review latency and scope disagreements; contributions tend to be small |
| **License compatibility** | MIT — compatible | All permissive: object_store Apache-2.0/MIT, rustac MIT/Apache-2.0, async-tiff MIT (Cargo says MIT-or-Apache; LICENSE is MIT only), zarrs MIT/Apache-2.0, libdeflate MIT, OpenJPEG BSD-2, PROJ/GDAL MIT/X. **Avoid** Grok (AGPL), Kakadu (proprietary), `eorst` (LGPL) | Upstream's license |
| **Extensibility** (new formats, sinks, providers) | Low: xarray/DuckDB/asyncio coupling | High via `ChunkedFormat`, `Catalog`, `Sink`, `RangeSource` traits | Limited to what the host project accepts |

---

## 2. Recommendation

**B, gated, with C used only as targeted upstream contributions. Do not do A.** And B's first deliverable is not an engine — it is Phase 0 of [06](06-roadmap-risks.md): the measurement that tells you whether B should exist.

### 2.1 Why not A (repository evidence)
1. There is no Rust to keep: lazycogs `pyproject.toml` lists rustac, async-geotiff, obstore, pyproj, rasterix, xarray, pandas; GitHub language stats are `{"Python": 370430}` `[N01; spot-checked]`.
2. The architecture is chunk-first and Python-orchestrated; the properties the brief asks for (global planning, bounded memory, sink streaming) are what it lacks `[N01 §2]`.
3. The one attempt to put Rust under its hot path (PR #58, rust-warp) is unmerged, depends on a crate whose README says "vibecoded with claude, mildly trust what it says", 5 stars, dormant since Feb 2026 `[N01 §1]`.
4. The project itself may be absorbed upstream (issue #89) or stall further.
5. What to *keep* from lazycogs: its numpy mosaic as a **reference implementation of semantics**, its rasterio-parity test idea (strengthened to real data), and its design ideas (§1 row "Code reuse").

### 2.2 Why B rather than C
No existing Rust project provides the engine layer; C would mean growing one of four libraries beyond its remit. The generic gaps are best fixed upstream **because they help everyone** and reduce what Tile-Ripper must own:

| Upstream | Contribution (evidence for need) |
|---|---|
| async-tiff | decode-into-caller-buffer (#234, #203); libdeflate (#270 open); striped + sparse tiles (PR #340); 15-bit unpack (PC); remove `.expect()` panics (#318, #97); header-cache fix (#344); `ReqwestReader::get_byte_ranges` override; fix/verify planar+predictor (#346/#348); license consistency `[N01 §3]` |
| object_store | `HttpConnector` shared-client ergonomics, operation-aware interception/metrics (#705), `Retry-After`, retry budget (#268), configurable coalesce gap/parallelism, parallel ranged download (#274), `HttpStore` 200 handling (#806/#858) `[N03]` |
| rustac | concurrent windowed STAC paging; bbox-covering predicates in `stac-duckdb`; non-buffering `search_to` `[N02 §4–5]` |
| zarrs | bounded-memory shard writer (#482); document all-fill-chunk delete behaviour `[N06 §4–5]` |
| Ecosystem | a **published tile-offset layout index** for public S2 COGs (the STAC-GeoParquet archives carry none) — instead of each user building one as Rasteret does `[N08 open Q5]` |

### 2.3 Conditions under which B should *not* proceed
(From [06](06-roadmap-risks.md) Gate G0.) If a tuned GDAL/odc-stac (or Rasteret) is within ~1.4× of the measured floor on the owner's workloads, "extending the ecosystem" beats "building a new engine". The deliverables then are the harness, the tuned recipe, and the upstream contributions above.

### 2.4 A narrower fallback (labelled `[INFER]`, unproven)
If G0 lands in the "narrow" zone: keep GDAL as the pixel engine and build only the planner/scheduler as a library that produces an explainable plan and **prefetches ranges** (e.g. feeding GDAL's caches or serving `/vsi` reads from a scheduler-owned buffer through a VSI plugin handler). Unknowns: whether GDAL's cache/plugin APIs let a scheduler own connection/concurrency policy without re-introducing per-thread connection state; I have not verified feasibility. This fallback loses the "no GDAL dependency" benefit and the zero-warp memory advantage, so it should be chosen deliberately, not drifted into.

---

## 3. Differentiation (Deliverable 4)

### 3.1 What Tile-Ripper is **not** differentiated on
These are where the evidence says to avoid claims:
- **Rust itself.** C wins on deflate, zstd and JP2 decode and on warp generality; async-tiff decode was ~1.7× slower than GDAL's at 16 threads on the maintainers' own benchmark `[N07, N01]`.
- **Discovery.** A STAC page is 0.3–3 s; this is solved `[N02]`.
- **Range coalescing, parallel fetch, multithreaded decode, overview selection, signing/retry basics.** GDAL has them `[N04 §2]`; `object_store` has coalescing.
- **Pixel-reading speed on a ~1 Gbps laptop.** Bytes-bound; nothing helps except reading fewer bytes `[N08 §B.7]`.

### 3.2 Candidate differentiators, ranked by evidence

| # | Differentiator | Already exists? | Evidence it matters | Confidence |
|---|---|---|---|---|
| D1 | **Floor-relative, reproducible, pre-registered benchmark harness** (oracle-checked, frozen inputs, interleaved runs) | No public equivalent; every published speedup is against an untuned/different baseline `[N04, N01]` | Directly addresses the field's credibility gap | **High** (but it is a research asset, not a product) |
| D2 | **Global plan with a persistent ETag-keyed layout index**, cross-object dedupe, header pipelining, QA-first conditional cancellation, `explain()` | Pieces exist in Python: Rasteret (index-first, plans all ranges, per-URL coalescing), sentle (overlap dedupe), lazycogs (`explain`); **none combine them, none do QA-first skipping** `[N08, N04]` | Model: header cache ≈ +20% at C=64, 1.5× at C=16; dedupe 408→94 synthetic requests; Kayrros 110→3 requests for JP2-TLM `[N08 §B]`; **no head-to-head** | **Medium** — the central thesis; unproven against a tuned baseline |
| D3 | **Adaptive per-host concurrency + hedged range reads**, retry budget, `Retry-After`, SAS single-flight refresh | **No shipped raster or object-store library**; AWS TM has hedging (S3-only, whole object, not for production) `[N03, N08]` | Tail latency with ≥ 1000 requests; AWS retry guidance; Pangeo/odc anecdotes of 10× config effects `[N04]` | **Medium–low** (value only if E7/E9 show a p99 win) |
| D4 | **Bounded-memory streaming from plan to sink, sink-driven chunk ordering**, ChronoZarr adapter | No acquisition tool writes a sink with backpressure; incumbent path is numpy→dask→rechunk→`to_zarr` (22% of the wide-vs-deep loss attributed to fusing) `[N04]`; sentle writes Zarr but boundedness unverified | Robustness/predictability (memory), earlier chunk completion; **not** raw speed (encoder ~110–170 MB/s isn't the bottleneck) `[N06]` | **Medium** |
| D5 | **Cheap very-high concurrency** (hundreds of in-flight requests without a thread each) | GDAL's curl state is thread-local (3.12); master changes it (unbenchmarked) `[N04 §5]` | Needed at ≥ 25–100 Gbps with HTTP/1.1 (C* ≈ 200–250 connections for 100 Gbps) `[N08 §B.4]` | **Medium**, only for large-NIC cloud runs |
| D6 | **Native-grid zero-warp fast path** (identity/integer-shift/scale), explicit zone policy | GDAL has no identity fast path; odc-geo has `_can_paste` (Python) `[N07 §6]` | Removes warp cost for the dominant same-zone case; small build | **Medium**, low effort |
| D7 | **Python-free, GDAL-free deployment for native-grid COG extraction** | async-tiff + wheels; lazycogs needs PROJ via pyproj | Operational simplicity (single binary); lost once warp is delegated to GDAL | **Low–medium** |

### 3.3 Overlap with the strongest threats

| Threat | What it already covers | What it doesn't | If it improves |
|---|---|---|---|
| **Rasteret** (Python, Apache-2.0, v0.4.1) | Index-first layout cache, plan-all-ranges, async obstore reads, thread-pool decode; arrays/xarray out | Per-URL coalescing only (1 KiB gap, batches of 20), one 150-slot semaphore, no dedupe/priorities/hedging, rasterio for warp, no sink/Zarr writer, header parse ≥ 4 RTTs/band/scene, benchmark excludes index build | D2 collapses into "do Rasteret in Rust" |
| **sentle** | End-to-end (AOI,time)→Zarr cube; overlap dedupe | GDAL/joblib orchestration; Sentinel-1/2 specific; bounded memory unverified | D4 weakens if verified bounded |
| **VirtualiZarr + virtual-tiff + Icechunk / EOPF Zarr** | Skips acquisition entirely (references or native Zarr); no re-encode | No mosaic/reproject; each virtual chunk fetched separately (never merged); defaults ≈ 1.9 Gbps untuned `[INFER]`; no public virtual S2 archive; EOPF Europe-only, operational ≈ late 2026/early 2027 | Shrinks the S2 "acquire pixels" case; other collections remain |
| **GDAL 3.13/3.14** | `ReadMultiRange` retry+sorting, single shared curl multi handle (master), GTI over STAC-GeoParquet, Zarr v3 sharding, `gdal raster pipeline` | Python-free planner/streaming sink, hedging/adaptive limits | Raises the baseline bar (re-measure when 3.14 ships, due 2026-11-08) |
| **odc-stac (+ lazycogs merger, #89)** | Mature planning, mosaic semantics, community | Per-read opens, serial default, Python planning/graph cost | Would absorb the "Rust-backed loader" niche |

### 3.4 One-sentence positioning (to be tested, not asserted)
> Tile-Ripper is an **observable, memory-bounded acquisition planner and scheduler** — it knows every byte it will read before it reads it, keeps a persistent layout index, schedules across all assets and hosts, and streams the result into a sink — built on best-in-class C codecs and GDAL where Rust isn't faster.

It is **not** "faster than GDAL". Whether the planning and scheduling is worth ≥ 1.5× end-to-end on any workload that matters to the owner is exactly what Gate G0/G1/G2 measure.
