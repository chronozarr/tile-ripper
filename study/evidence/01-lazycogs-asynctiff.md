# Notes 01: lazycogs / async-tiff / async-geotiff (+ related readers)

Date of audit: 2026-10-09. Read-only. Clones under scratchpad/repos/{lazycogs,async-tiff,async-geotiff}; rustac clone belongs to another agent (I only read it).

Pinned commits (all `main` HEAD at time of reading):
- lazycogs        ca203a9326f58b4b73457be1ed456d8f8c3ed1f1  (2026-07-02)   https://github.com/developmentseed/lazycogs
- async-tiff      cf4bc5943a4d25f56e80c3be8e2827e828bb96b7  (2026-10-08)   https://github.com/developmentseed/async-tiff
- async-geotiff   468cd9d1cd788e1f4198bfe1cff571760e6eb219  (2026-09-29)   https://github.com/developmentseed/async-geotiff
- rustac (read by me, not mine) 4095a77bb46602d87c4a9067313e209990c6f3d8   https://github.com/stac-utils/rustac

Evidence grades: [SRC] read code/docs; [BENCH] reproducible benchmark; [CLAIM] authors' assertion; [INFER] my inference.

---------------------------------------------------------------------------
## 0. One-paragraph verdict

The "Rust-native COG stack" at Development Seed is three layers:
1. async-tiff (Rust crate + PyO3 wheels): the only real Rust. Header/IFD parse, tile byte-range planning, per-tile decode. NO windowing, NO overview selection, NO resampling, NO warp, NO scheduling (zero spawn/rayon/semaphore in the Rust core).
2. async-geotiff: PURE PYTHON (2.8k LOC) over async-tiff's Python wheel. Adds window->tile math, tile stitching in numpy, CRS via pyproj, nodata/mask handling. Its README lists "No resampling, no warping/reprojection, no automatic overview selection" as explicit anti-features.
3. lazycogs: PURE PYTHON (4.2k LOC src). STAC-geoparquet (DuckDB via rustac) -> per-chunk plan -> async-geotiff reads -> pyproj+numpy nearest-neighbour warp -> numpy masked-array mosaic -> xarray BackendArray. Zero Rust of its own. The one Rust warp attempt (rust-warp) is a stale draft PR.

So: nothing existing is a Rust-native end-to-end acquisition engine. What exists is a Python orchestration layer (lazycogs) that is a good *design reference*, plus one reusable Rust crate (async-tiff) that is a *reader*, not an engine, and has correctness gaps (striped TIFF, sparse tiles, planar+predictor bug, panics, JP2 untested).

---------------------------------------------------------------------------
## 1. Repo metadata

| | lazycogs | async-tiff | async-geotiff |
|---|---|---|---|
| License | MIT (LICENSE file; "Copyright (c) 2026 Development Seed") [SRC] | LICENSE file is MIT only (PistonDevelopers 2018 + DevSeed 2024); Cargo.toml says `MIT OR Apache-2.0` but there is no Apache text in repo root [SRC] | MIT [SRC] |
| Created | 2026-04-13 | 2024-08-15 | 2026-01-23 |
| First release | v0.1.0 2026-04-27 | rust-v0.1.0 2025-03-14 (crates.io 0.1.0-beta.1 2025-03-05); py-v0.1.0 2025-03-18 | v0.1.0 2026-02-03 |
| Latest release | v0.7.0 2026-07-01 (13 releases in 9 weeks, then silence ~100 days) | rust-v0.4.0 2026-09-18; PyPI async-tiff 0.7.2 2026-05-06 | v0.5.1 2026-05-18 (no release in ~5 months) |
| Last human commit on main | 2026-07-02 (Henry Rodman) | 2026-09-18 (Wei Ji, release prep); 2026-09-01 Kyle Barron | 2026-07-29 (Kyle Barron, ruff rules); previous real work 2026-05-28 |
| Commits since 2026-04-09 | 65 human + 13 release-bot, ALL one person | 6 human (Kyle Barron 4, David Marteau 1, Wei Ji 1) + 30 dependabot | 10 human (all Kyle Barron, mostly May) + 13 dependabot |
| Contributors (all time) | hrodmn 65, bot 13 (that is the entire list) | kylebarron 159, dependabot 36, weiji14 14, feefladder 6, 5 others <=2 | kylebarron 87, dependabot 18, autra 4, 2 others 1 |
| Bus factor | 1 (Henry Rodman) | ~1 (Kyle Barron), weiji14 secondary | 1 (Kyle Barron) |
| Stars/forks | 55 / 1 | 121 / 16 | 58 / 7 |
| Open issues (excl PRs) | 10 (+4 PRs) | 26 (+10 PRs) | 17 (+5 PRs) |
| Closed issues | 14 | 79 | 42 |
| Downloads | PyPI ~440/month | PyPI ~54k/month; crates.io 38.9k total (35k recent) | PyPI ~24.7k/month |
| Maturity self-label | pyproject "Development Status :: 3 - Alpha"; DevSeed blog calls it "an experiment" [CLAIM] | pre-1.0 (0.4.0); many `feat!` breaking changes in each minor | "Alpha" classifier |
| MSRV | n/a | 1.88 (crates.io metadata for 0.4.0; Cargo.toml `rust-version = "1.88"`) | n/a |

Sources:
- repo metadata: `gh api repos/developmentseed/{lazycogs,async-tiff,async-geotiff}`
- releases: https://github.com/developmentseed/lazycogs/releases ; https://github.com/developmentseed/async-tiff/releases ; https://github.com/developmentseed/async-geotiff/releases
- PyPI: https://pypi.org/project/lazycogs/ ; https://pypi.org/project/async-tiff/ ; https://pypi.org/project/async-geotiff/ ; stats https://pypistats.org/api/packages/{name}/recent
- crates.io: https://crates.io/crates/async-tiff (reverse deps: eorst 1.0.1, rss_core 0.6.0, martin-core 0.13.0)
- DevSeed blog calling lazycogs an experiment: https://developmentseed.org/blog/2026-07-02-groundwork-05/ [CLAIM]

Related Rust readers (matter little):
- `tiff` (image-rs/image-tiff): sync, pure Rust, 130M downloads, 0.11.3; async-tiff forked its tag parsing (README says so). https://github.com/image-rs/image-tiff
- `geotiff` (georust/geotiff): sync, "reading GeoTIFFs, nothing else", crate 0.1.0 (2025-06), ~24k downloads. https://github.com/georust/geotiff
- `cog3pio` (weiji14): Rust COG reader on top of `tiff` crate + optional nvTIFF/CUDA, v0.1.0 2026-05-25, 607 downloads, single author. NOT async-tiff based. https://github.com/weiji14/cog3pio
- `rastera` (chrieke): "Async rasterio for COGs" built on async-geotiff, nearest/bilinear/cubic with GDAL-like kernels + antialiasing, merge, persisted header cache (~6x faster opens, [CLAIM]); 1 star, hobby-scale. Striped TIFFs unsupported. https://github.com/chrieke/rastera
- `eorst` / `rss_core` (Queensland JRSRP, GitLab): Rust EO toolkit using GDAL + async-tiff 0.3 + `stac` crate; LGPL-3.0-or-later. https://crates.io/crates/eorst . I could not inspect its source (GitLab 403 to WebFetch). Worth another agent's look for novelty.
- rust-warp (jakenotjay): Rust warp engine w/ PyO3; README says "This entire codebase is vibecoded with claude, mildly trust what it says"; v0.1.0 2026-02-20, last push 2026-02-28, 5 stars, Apache-2.0. [SRC] https://github.com/jakenotjay/rust-warp

---------------------------------------------------------------------------
## 2. lazycogs: architecture and dependency graph

### 2.1 Dependencies [SRC pyproject.toml @ca203a9]
affine, arro3-core, async-geotiff>=0.5.1, cql2, numpy>=2.4.4, obstore>=0.9.2, pandas>=3.0.2, pyproj>=3.7.2, rasterix>=0.2.0, rustac[arrow]>=0.9.13, xarray>=2026.2.0, xproj; optional dask. requires-python >=3.12. rasterio is dev-only (parity tests). No GDAL. PROJ (C) via pyproj is still a hard dep.
https://github.com/developmentseed/lazycogs/blob/ca203a9326f58b4b73457be1ed456d8f8c3ed1f1/pyproject.toml

Runtime stack = Python orchestration; Rust only transitively: rustac (DuckDB-in-Rust-via-duckdb-rs), obstore (object_store), async-tiff (inside async-geotiff).

### 2.2 Item discovery / pruning [SRC]
- `open()` accepts only a local geoparquet file (or, with a caller-supplied `DuckdbClient`, a hive-partitioned parquet dir). It does NOT query a STAC API. User must first call `rustac.search_to("items.parquet", api_url, ...)`. Author reasoning: avoid hundreds of concurrent STAC API calls from dask tasks. https://github.com/developmentseed/lazycogs/blob/ca203a9326f58b4b73457be1ed456d8f8c3ed1f1/ARCHITECTURE.md
- At open: one `search_to_arrow` over the parquet for bbox/datetime/filter to build time steps (`_core.py` `_build_time_steps`), plus open one representative COG per band to infer dtype/nodata.
- Per chunk, per time step: one DuckDB `client.search(parquet, bbox=chunk_bbox_4326, datetime=step.datetime_filter, sortby, filter, ids, include=[id, assets, ...])` (`_backend.py` `_search_items_sync`, L152-159). rustac builds `SELECT ... FROM read_parquet('<href>', hive_partitioning=..., union_by_name=...) WHERE ST_Intersects(geometry, ST_GeomFromGeoJSON(?)) AND datetime...` (rustac `crates/duckdb/src/client.rs` L361-381, L444). [SRC] https://github.com/stac-utils/rustac/blob/4095a77bb46602d87c4a9067313e209990c6f3d8/crates/duckdb/src/client.rs
- [INFER] Every chunk x time-step re-scans the parquet with an ST_Intersects predicate (no persisted spatial index, no use of GeoParquet bbox covering columns visible in this code). Cost scales with archive size x number of chunk-queries. Authors acknowledge DuckDB serialises on one connection (issue #30/#31).
- DuckDB queries run on a dedicated 1-worker thread pool with a 2-slot submission gate (`_executor.py` `_DUCKDB_MAX_WORKERS = 1`, `_DUCKDB_MAX_SUBMISSIONS = 2`). So item discovery is effectively serial across the whole process.
- rustac Client does `INSTALL spatial/icu/httpfs; LOAD ...` at construction (client.rs L81-86): first use needs network to fetch DuckDB extensions. [SRC]
- Item-level filtering: CQL2 `filter=` (e.g. `eo:cloud_cover < 20`) and `sortby=`; no pixel-level quality masking (see 2.6).

### 2.3 Read planning and concurrency [SRC]
Unit of work: one dask/xarray chunk -> `MultiBandStacBackendArray._async_getitem` -> for each requested time step (all gathered concurrently) -> DuckDB query -> `read_chunk_async(items, bands, ...)` -> one asyncio task per ITEM (all bands of the item together).
- Concurrency bound: `max_concurrent_reads=32` (asyncio.Semaphore) per chunk materialisation, shared across that chunk's time steps (v0.7.0, PR #81). Not a request-rate limiter: one item read opens B band COGs, each async-geotiff read can issue several range requests (README "Concurrency notes"). There is NO global in-flight-bytes or in-flight-request budget; with dask, N concurrent chunks each get their own 32 (they all share ONE persistent asyncio loop thread). [INFER]
- `_read_item_band` (`_chunk_reader.py` L430-510): (1) `GeoTIFF.open(path, store)` for every band concurrently (L462), (2) per band choose overview + compute source window, (3) `reader.read(window=...)` for all bands concurrently, (4) `run_in_executor` on the reprojection thread pool to warp.
- Results are fed to the mosaic in source-list order via `_drain_in_order` (L519) even though I/O completes out of order. `FirstMethod` stops early and cancels outstanding tasks once the output is filled; but up to 32 items are speculatively in flight.
- Header caching: NONE across reads. `GeoTIFF.open` is called per item x band x chunk x time step; the readahead metadata cache inside async-tiff is local to each open. ARCHITECTURE.md itself says spatial chunking "adds more DuckDB queries, more COG opens". [SRC]
- Connection reuse: obstore stores are cached per `scheme://netloc` in a process-global dict (`_store.py` L23-33) so the underlying HTTP client/pool is reused. [SRC]
- Range coalescing: delegated to async-tiff -> object_store `get_ranges` (1 MiB gap coalescing, max 10 parallel; see 3.3). lazycogs does no coalescing of its own, and does not coalesce across items/bands.
- Executors: one daemon asyncio loop thread; DuckDB pool (1 worker); reprojection ThreadPoolExecutor of `min(cpu_count, 4)` (env `LAZYCOGS_REPROJECT_WORKERS`) because pyproj + numpy fancy indexing are "memory-bandwidth-bound" (`_executor.py` L31-39). [INFER] mosaic `feed` + `assemble_tiles` + `_array_to_masked` run on the single loop thread -> GIL/loop-thread serialisation point.

### 2.4 Reproject / resample [SRC]
- Warp = `pyproj.Transformer.transform` on a full destination-pixel-centre grid (np.meshgrid) -> inverse affine -> `np.floor` -> integer row/col arrays ("WarpMap") -> numpy fancy-index gather. https://github.com/developmentseed/lazycogs/blob/ca203a9326f58b4b73457be1ed456d8f8c3ed1f1/src/lazycogs/_reproject.py#L53-L128
- Nearest-neighbour ONLY. No bilinear/cubic/average. ARCHITECTURE.md: "Nearest-neighbor is the only supported resampling method." Issue #25 (bilinear/cubic, open since 2026-04-27) points to a spec using `interpn`.
- WarpMap cache keyed on `(tuple(window transform), src_crs)` shared across bands/time steps in a chunk.
- Fast path: same CRS + same affine + same shape -> skip warp.
- Source window = envelope of the 4 projected chunk corners only (`_chunk_bbox_native`, `_chunk_reader.py` L267; `_native_window` L292). [INFER] For large chunks in strongly non-conformal CRS pairs, curved edges can fall outside the 4-corner envelope -> possible missing edge pixels (no edge densification). Not tested by authors.
- Overview selection: coarsest overview with pixel size <= target pixel size in source CRS, else full-res (`_select_overview` L235). No anti-aliasing step when downsampling from the chosen overview with nearest. [INFER] aliasing between overview levels (same as odc-stac nearest).
- Rust warp status: branch `feat/rust-warp` + draft PR #58 "[WIP] feat: use rust-warp instead of pyproj + numpy for reprojection" (opened 2026-05-13 by hrodmn, last updated 2026-05-14, still open, never merged; branch 7 ahead / 20 behind main). PR's CI benchmark comment (offline local COGs, pytest-benchmark, GH runner) shows large wins from replacing numpy/pyproj warp: test_full_compute 739.5 -> 395.0 ms (-46.6%); test_band_access_pattern[multi_band] 1447.8 -> 501.4 (-65.4%); test_native_crs_resolution 11367.1 -> 1322.0 (-88.4%); test_time_step_parallelism[no_dask] 1025.6 -> 465.4 (-54.6%). [BENCH] (scripts/prepare_benchmark_data.py + tests/benchmarks; hardware = GitHub runner). Interpretation [INFER]: on local files the numpy/pyproj warp dominates lazycogs's wall time; network I/O is not what these numbers measure. The odd -88% case is a "native CRS, no reprojection expected" test that nevertheless took 11 s on main; I did not root-cause it. https://github.com/developmentseed/lazycogs/pull/58
- rust-warp README speed table is a [CLAIM] (1024x1024 float32, UTM33N->4326, Apple Silicon; nearest 1.6 ms vs 10.4 ms GDAL etc.). https://github.com/jakenotjay/rust-warp
- rust-warp supports only f32/f64/i8/u8/u16/i16 (per lazycogs spec `_SUPPORTED_DTYPES`), 2-D only (lazycogs loops bands in Python). https://github.com/developmentseed/lazycogs/blob/1095e8e53f4512354ca73ddda3a1e445da63c113/dev-docs/specs/rust-warp-integration.md

### 2.5 Mosaic [SRC] `_mosaic_methods.py`
FirstMethod (early stop), Highest, Lowest, Mean, Median, Stdev, Count. Pure numpy masked arrays. A pixel is "masked" only if ALL bands == nodata (`_array_to_masked`, `_chunk_reader.py` L220). No SCL/QA-band/bitmask support, no per-pixel cloud masking, no cross-asset masking (an SCL asset cannot gate B04). Item-level cloud filter only via CQL2. Median/Stdev hold full stacks in memory.
Time grouping: `time_period` P1D/PnD/P1W/P1M/P1Y/PTnH/None, calendar-day (UTC) not solar-day (odc-stac default is solar_day) -> outputs differ (see 2.8).

### 2.6 Laziness / memory / writing [SRC]
- xarray `BackendArray` + `LazilyIndexedArray` (no dask unless `chunks=` given). `da.compute()` without chunks materialises the entire (band,time,y,x) result in one `_async_getitem`. ARCHITECTURE "Chunking strategy": omit chunks for max throughput if it fits in memory; otherwise `{"time":1,"x":N,"y":N}` with big spatial chunks.
- No writer, no streaming sink. Zarr/anything is "whatever xarray `.to_zarr()` does over dask". Bounded memory is only via dask chunk sizing + `max_concurrent_reads`.
- Also a low-level API `await lazycogs.read_chunk_async(...)` and `da.lazycogs.explain()` (dry-run planner that lists COG reads, overview level, windows w/o pixel I/O; ~760 LOC `_explain.py`). The explain plan is a decent design reference.

### 2.7 Library vs framework; what is extractable
- It is a library whose public surface is `lazycogs.open() -> xarray.DataArray`; the pieces are tied to xarray indexing keys, DuckDB client objects and asyncio. `read_chunk_async` is the nearest thing to a framework-free entry (items + bands + chunk affine + dst_crs -> numpy dict), still Python.
- There is no Rust in lazycogs to extract. What is portable is the DESIGN: chunk -> per-time-step item query -> per-item task fan-out under a semaphore -> overview pick -> window from projected corners -> warp-map cache -> ordered drain + early-stop mosaic.
- Python-dependent: all of it. Rust equivalents already exist for: STAC-geoparquet query (rustac/stac-duckdb), storage (object_store), TIFF IO (async-tiff). Missing in Rust: window/overview planner, warp/resample kernels (rust-warp is unmaintained/AI-written; GDAL not allowed), mosaic, scheduler.

### 2.8 Tests, benchmarks, evidence
- 297 test functions, 5.5k test LOC; CI runs pytest on py3.12/3.13/3.14; separate benchmark workflow on local data. [SRC]
- Pixel correctness: `tests/test_rasterio_parity.py` compares lazycogs read+warp to `rasterio.warp.reproject(nearest)` on a SYNTHETIC COG at 18 resolutions bracketing overview boundaries: same-CRS bit-identical; cross-CRS (EPSG:32632 -> 3035) allows <=3 differing pixels and abs diff up to 2048*16+1 (one overview row). Nearest only; one synthetic file; no real-data parity, no bilinear/cubic, no antimeridian/polar. https://github.com/developmentseed/lazycogs/blob/ca203a9326f58b4b73457be1ed456d8f8c3ed1f1/tests/test_rasterio_parity.py
- Published comparison (docs/performance.md; notebook docs/notebooks/lazycogs-odc-stac.ipynb): Sentinel-2 C1 L2A (earth-search `sentinel-2-c1-l2a`), Midwest, summer 2025, EPSG:5070 @100 m (7100x13000, 121 daily steps, 3 bands), odc-stac 0.5.2, "on a laptop in a single run". Results: init 0.77 s vs 45.47 s (odc-stac; plus 10.44 s to load items into memory); point extract 30 days 4.96 s vs 17.13 s; 3-timestep subset 13.55 s vs 28.81 s. [BENCH, weak]: code+notebook public, but single run, hardware/network location unspecified, unmatched chunking (lazycogs time=1 unchunked space vs odc 512x512), unmatched time grouping (calendar day vs solar_day) and unmatched dtype (int16 forced vs uint16) - output values visibly differ on 2 of 30 point-extract days in the notebook output (e.g. 11786 vs 9976). The "init" gap is mostly "odc-stac needs all items in RAM" vs "lazycogs reads parquet lazily", not a pixel-pipeline speedup. https://github.com/developmentseed/lazycogs/blob/ca203a9326f58b4b73457be1ed456d8f8c3ed1f1/docs/performance.md
- No benchmark vs stackstac in the repo despite README framing. No network-bound throughput benchmark (MB/s, requests/s, per-hardware scaling). Benchmarks in `tests/benchmarks/` are on local files (30x30 km AOI near Colorado, 2 bands, ~10 items).
- Contradiction: README says "No GDAL required" and the table lists reprojection as `pyproj + numpy` - accurate - but lead-in text claims "Rust-native stack"; the pixel path (warp, mosaic, stitching, orchestration) is Python/numpy.

### 2.9 Substantive open issues (lazycogs)
- #89 "Consider merging with odc-stac" (alexgleith, 2026-08-27, 0 replies): suggests odc-loader backend (odc-stac "abstracts the loading engine") + STAC-geoparquet indexes. Relevant: odc-loader is the abstraction point where a Rust engine could plug in.
- #25 bilinear/cubic resampling (open).
- #30/#31 DuckDB blocks loop / "cleaner concurrency model" (kylebarron): API split of sync/async, DuckDB single-connection serialisation.
- #42 pystac_client integration; rustac "builder error" on DEA STAC (users blocked by rustac).
- #85 / PR #79 single-scene native-grid loading; #50 multi-band COG support (open); #51 progress hook; #16 point sample().
- Maintainer silence since 2026-07-02 (open PRs #87, #88 unmerged).

---------------------------------------------------------------------------
## 3. async-tiff (Rust crate + Python bindings)

### 3.1 Dependencies / features [SRC Cargo.toml @cf4bc59]
Core deps: async-trait, bytemuck, byteorder, bytes, flate2 (deflate), jpeg-decoder 0.3 (JPEG; upstream repo says "currently in maintenance mode"), weezl (LZW), zstd, tokio (sync only), thiserror, num_enum. Optional features: `object_store` + `reqwest` (default), `jpeg2k` (OpenJPEG bindings), `lerc`, `lzma` (lzma-rust2), `webp`, `ndarray`, `tokio` (io-util TokioReader). Default features = `object_store, reqwest` ONLY: a plain `cargo add async-tiff` gets Deflate, LZW, JPEG, ZSTD, None; LERC/LZMA/WebP/JPEG2000 need features. Python wheel enables jpeg2k+webp+lzma+lerc.
https://github.com/developmentseed/async-tiff/blob/cf4bc5943a4d25f56e80c3be8e2827e828bb96b7/Cargo.toml
- Python crate `py-async-tiff` 0.7.2: pyo3 0.29, pyo3-async-runtimes, pyo3-object_store 0.12, pyo3-bytes, rayon, tokio-rayon; abi3-py311.
- crates.io: 0.4.0 (2026-09-18), MSRV 1.88; 3 reverse deps (eorst, rss_core, martin-core). Pre-1.0, breaking (`feat!`) changes in 0.2->0.3 (transparency masks, Compression rename, colormap, band-interleaved) and open refactors of `Decoder` trait (#203) and error handling (#186). Expect API churn.

### 3.2 What it supports [SRC]
- Compression (decoder registry `decoder.rs`): None, Deflate/OldDeflate, LZW, JPEG (modern, w/ jpeg tables), ZSTD default; LERC / LERC+deflate / LERC+zstd, LZMA, WebP, JPEG2000 behind features; user-defined decoders via `Decoder` trait/registry. NOT present: PackBits (enum exists, no decoder), CCITT, JPEG-XL (async-geotiff blog: "We should support ... JPEG XL").
- JPEG2000: only compression tag 34712 (J2K stream inside a TIFF tile) via `jpeg2k` crate. Standalone `.jp2` files (e.g. legacy Sentinel-2 L1C/L2A JP2 on AWS/GCS/CDSE) are NOT supported at all. I found zero tests referencing JPEG2000 in the repo (rg over src/, python/tests/). README nonetheless advertises "JPEG2000".
  https://github.com/developmentseed/async-tiff/blob/cf4bc5943a4d25f56e80c3be8e2827e828bb96b7/src/decoder.rs#L248-L283
- Predictors: Horizontal (all bit depths 8/16/32/64) and FloatingPoint; endianness fixes (`predictor.rs`).
- Data types: u8/16/32/64, i8/16/32/64, f32/f64, Bool(1-bit). Missing: float16, complex (#255); non-byte-aligned depths like 15-bit fail (#334).
- Layout: TILED only. Striped TIFFs: metadata parses but `fetch_tile(s)` returns `Err("Not a tiled TIFF")` (ifd.rs L744/L757). README: "support for tiled TIFF images". async-geotiff additionally warns "Striped GeoTIFFs are not supported" (_geotiff.py L164).
- BigTIFF: yes (version 43 header; `metadata/reader.rs`).
- Planar config: chunky and planar (band-interleaved); tile-interleaved (GDAL COG INTERLEAVE=TILE) unsupported (#247, needs ghost-area parse #201). Cannot fetch a subset of bands from a planar file (fetches all; #249; PR #251 open).
- Sparse tiles (offset=bytecount=0): fetch errors ("Requested range was invalid") - PR #340 open (2026-09-11). GDAL SPARSE_OK COGs would fail until merged.
- Overviews: just more IFDs; selection is the caller's job (async-geotiff sorts by pixel count).
- Masks: transparency-mask IFDs (feat! in 0.3.0), ExtraSamples enum; GDAL_NODATA and GDAL_METADATA tags exposed as strings.
- GeoTIFF keys: `GeoKeyDirectory` parsed in Rust; conversion to a CRS object happens in async-geotiff PYTHON via pyproj (`_crs.py`, 682 lines, "Most of the code to construct a custom PROJJSON is generated by Claude" per its docstring). A Rust engine would have to redo GeoKeys -> CRS (EPSG lookups via a Rust PROJ binding or proj4rs).
- WASM: has `wasm-build.yml` and `async_trait(?Send)` cfgs; Pyodide support requested (#310).

### 3.3 Metadata fetch and tile fetch [SRC]
- Header: `ReadaheadMetadataCache` (`metadata/cache.rs`): first fetch 32 KiB from byte 0, each subsequent miss grows geometrically (x2). Python `TIFF.open(prefetch=32768, multiplier=2.0)`; `TIFF.header_byte_size` (new in py 0.7.2 / rust 0.4) lets a caller learn the minimal prefetch to get all metadata in one request next time. Python `open` calls `read_all_ifds` (every overview IFD, incl. full TileOffsets/TileByteCounts arrays) before returning. Round trips: 1 if all IFDs fit in the prefetch; otherwise 2, 3... (sequential, mutex-held growth). Metadata cache is sequential-from-offset-0 only: non-COG TIFFs with IFDs mid-file make it download most of the file (#344, 2026-09-25; per the thread, PR #328's arbitrary-offset IFD read "probably fixes the worst case", and a swappable-cache API is only a plan).
- Tile bytes: `ImageFileDirectory::fetch_tiles(&[(x,y)], reader)` builds one `Vec<Range<u64>>` and calls `AsyncFileReader::get_byte_ranges` once (`ifd.rs` ~L750, L832-860). Coalescing/parallelism is whatever the reader implements:
  - `ObjectReader` -> `object_store::get_ranges` -> `coalesce_ranges` with gap <= 1 MiB (`OBJECT_STORE_COALESCE_DEFAULT`) and up to 10 parallel fetches (`OBJECT_STORE_COALESCE_PARALLEL`). Not configurable from async-tiff/async-geotiff (issues async-tiff #6 open since 2025-02; async-geotiff #78). obstore exposes a `coalesce` kwarg on its own `get_ranges` (get.rs L449) but async-tiff does not use it.
    https://github.com/apache/arrow-rs-object-store/blob/main/src/util.rs#L86-L89 (constants) ; async-tiff reader.rs L167-L185
  - `ReqwestReader` implements only `get_bytes`; the trait's default `get_byte_ranges` is SEQUENTIAL (reader.rs L43-L51 default impl; ReqwestReader L221+). So the built-in HTTP reader does not parallelise or coalesce. [SRC]
  - `TokioReader::make_range_request` (reader.rs L116-L117) reads into `Vec::with_capacity(n)` (len 0) then `file.read(&mut buffer)`: a zero-length slice reads 0 bytes, so the following length check returns EndOfFile for any n>0. [INFER from reading, not executed; behind the optional `tokio` feature].
- No retry/backoff, no hedging, no connection management in async-tiff itself (inherits object_store's).
- Python store input: obstore `PyObjectStore` -> pure-Rust `ObjectReader`; any obspec-style Python object -> `ObspecReader`, which calls back into Python for every range request (GIL round trip) (`python/src/reader.rs`).

### 3.4 Decode threading [SRC]
- Rust core: NO threading. `Tile::decode(self, &DecoderRegistry)` is a blocking sync call returning `Array` (`tile.rs` L80). Docs: "Decoding is separate from data fetching so that sync and async operations do not block the same runtime." The caller must schedule.
- Python bindings: `tile.decode()` -> `tokio_rayon::spawn_fifo_async` on a global default rayon pool (num_cpus) or a user `ThreadPool(n)`; returns buffer-protocol `Array` (zero-copy to numpy). `decode_sync` holds the GIL. (`python/src/tile.rs` L95, `thread_pool.rs`). Comment in CHANGELOG: LIFO -> FIFO change.
- Rust bench uses rayon `into_par_iter` over tiles (benches/read_tiff.rs).
- Decode allocates `Vec<u8>` per tile and then `TypedArray::try_new` casts/copies (`array.rs`); issue #234 says the zero-copy cast branch is effectively never taken for u16+ so an extra copy always happens; Decoder trait cannot decode into a caller-provided buffer (TODO in `decode_lerc`: "avoid this copy by allowing the return type of the decoder to be a typed array"; refactor #203 open). For an engine that wants decode-straight-into-destination-raster this is the key gap.

### 3.5 Correctness findings (read in source / issues)
- Planar (band-interleaved) + Predictor 2/3: `Tile::decode` passes `samples = samples_per_pixel` to `unpredict_hdiff/unpredict_float` even for planar tiles whose per-band data has 1 sample/pixel -> wrong pixels. Open issue #346 (2026-10-06, by geospatial-jeff with repro) and fix PR #348 (opened 2026-10-09, today, by weiji14). Planar COGs from GDAL with PREDICTOR + multi-band are affected until released. [SRC tile.rs L131; issue]
- `LZWDecoder` uses `.expect("failed to decode LZW data")` -> panic on corrupt/unsupported LZW instead of an error (decoder.rs L243). In a tokio/rayon worker this aborts the task (Python: uncatchable PanicException). [SRC]
- Panics on missing SamplesPerPixel tag (#318, fix PR #319 open), `rust future panicked` on GDAL-autotest TIFFs (#97, open since 2025-05; maintainer asks for per-file issues; reporter says unlikely to file them). `TIFF::header_byte_size` also `expect`s (tiff.rs L50). Library (non-test) IFD parsing panics via `expect` on missing/odd tags: ifd.rs L302-L395 (geo key dir, samples_per_pixel, image_width/height, bits_per_sample, photometric), metadata/reader.rs L274 (`Type::from_u16(..).expect`) and L316 (`checked_mul().unwrap()`). A malformed or exotic file can therefore panic a worker instead of returning Err. [SRC]
- LERC Rust tests only assert shape and dtype (not pixel values); Python integration tests compare with rasterio pixel-for-pixel for ~25 fixtures incl. LZW/predictor, LERC variants, zstd, LZMA, masks, antimeridian, custom CRS. JPEG-XL fixture commented out. JPEG2000 untested.
  https://github.com/developmentseed/async-tiff/blob/cf4bc5943a4d25f56e80c3be8e2827e828bb96b7/python/tests/test_integration_rasterio.py
- YCbCr JPEG decode mismatch vs rasterio/libtiff (#266, PR #267 open).
- 115 Rust tests + 11 Python test functions (parametrised).

### 3.6 Benchmarks that exist
1. In-repo criterion (`benches/read_tiff.rs`, CodSpeed): single Sentinel-2 TCI converted to LZW+predictor 2, local file, fetch all 1849 tiles then rayon-decode with 4 threads. Self-only (no GDAL comparison); CodSpeed "Simulation" mode in PR #139 (instruction-count simulation, reported "3.9 s") is not wall-clock. Regression guard only. [BENCH-self]
2. FOSS4G 2025 talk (Wei Ji Leong), repo weiji14/foss4g2025 (benches/read_geotiff.rs; slides/slides.md): Sentinel-2 TCI (Kenya) 3-band u8, converted to 256x256 tiled LZW+Predictor 2, 272.7 MB, LOCAL read-to-CPU on i5-12600HX. Timings: GDAL LiberTIFF 16 threads 0.26 s / 1 thread 2.2 s; async-tiff 16 threads 0.45 s / 4 threads 0.90 s / 1 thread 2.58 s. GPU path: nvTIFF 0.16 s, LiberTIFF+H2D 0.27 s, async-tiff+H2D 0.48 s (RTX A2000 laptop). [BENCH, I read slides + bench source name, did not re-run] => async-tiff decode is SLOWER than GDAL's libertiff at every thread count tested (1.7x at 16 threads). Slides' own roadmap: SIMD/faster decompressors. Issue #270 "Switch to libdeflate" open; maintainer: no custom SIMD yet.
   https://github.com/weiji14/foss4g2025/blob/main/slides/slides.md (lines ~177-335)
3. "25x faster" headline (async-geotiff blog 2026-02-03) = geospatial-jeff/pyasyncio-benchmark `cog_header` test: opens the SAME S2 B08 COG key repeatedly with 16 KiB prefetch, 500 concurrent, on one EC2 m5.4xlarge for 300 s. CSV: async-tiff 6,989 req/s vs rasterio 285 req/s (24.5x); raw obstore GET 11,087 req/s; aiohttp 7,727. Header parse only, same object, rasterio runs in a thread pool via anyio. [BENCH, narrow] The blog explicitly says the pixel-read speedup is an "expectation", not measured. No pixel-read benchmark vs GDAL/rasterio over a network exists in either repo. https://github.com/geospatial-jeff/pyasyncio-benchmark/blob/8809b62125ae75b3d216475e9964a6df4d96a91c/test_results/20250423_results/ec2_m5_300seconds/aggregated_results.csv ; https://developmentseed.org/async-geotiff/latest/blog/2026/02/03/introducing-async-geotiff/
4. async-geotiff blog also says "can be just as fast as any C or C++ library" [CLAIM].

### 3.7 Substantive open issues (async-tiff) - selected
#346 planar+predictor wrong pixels; #344 readahead cache harmful for non-COG; #334 non-byte-aligned depth; #318 panic no SamplesPerPixel; #270 libdeflate; #266 YCbCr mismatch; #255 all TIFF data types; #250 no-copy interleave exposure; #249 band-subset fetch; #247 tile-interleaved COG; #234 slow-path cast copy; #203 Decoder refactor; #201 GDAL ghost-area parse; #186 error-handling refactor; #97 panics on GDAL test files; #6 configurable coalesce; PR #340 sparse tiles; PR #328 expose more IFD machinery (virtual-tiff).

---------------------------------------------------------------------------
## 4. async-geotiff (Python)

### 4.1 Architecture [SRC]
- Language: 100% Python (`src/async_geotiff/*.py`, 2,771 LOC). Depends on `async-tiff>=0.7.0` (the PyO3 wheel), numpy>=2, pyproj>=3.3, affine, defusedxml; optional morecantile. https://github.com/developmentseed/async-geotiff/blob/468cd9d1cd788e1f4198bfe1cff571760e6eb219/pyproject.toml
- `Store` protocol = obspec `GetRangeAsync` + `GetRangesAsync` (v0.5.1).
- `GeoTIFF.open(path, store, prefetch=32768, multiplier=2.0)` -> `async_tiff.TIFF.open` (3.3). Builds overview list by sorting data IFDs by pixel area and pairing mask IFDs by dims (`_geotiff.py` L137-L170).
- `read(window)` (`_read.py` L58-L130): compute tile index range covering window -> one `fetch_tiles(xy)` (one `get_byte_ranges`) -> `asyncio.gather` of per-tile `decode()` (rayon) -> `assemble_tiles` loop in Python copying each tile slice into a freshly allocated `np.empty((bands,h,w))` -> `RasterArray(data, mask, transform)`. Whole tiles are fetched/decoded even when the window clips them; all bands always read; entire window held in RAM; `read()` with no window reads the whole image. Boundless/outside-image windows raise `WindowError` (#122 open).
- Anti-features (README): "No pixel resampling. No warping/reprojection. Automatic resolution/overview selection" [CLAIM-of-scope, SRC]. Suggests rasterio MemoryFile for those.
- GeoTIFF transform: ignores raster-space tiepoint (I,J): `create_from_model_tiepoint_and_pixel_scale` uses `model_tiepoint[3], [4]` only (`_transform.py` L42-L51). Downstream issue jejjohnson/geotoolz#414 (2026-10-08) documents shift for files written by tifffile/others; GDAL never writes non-zero (I,J). [SRC]
- CRS: pyproj from EPSG or generated PROJJSON for user-defined CRS; custom CRS support has had bugs (fix 2026-05-28 #165; downstream geotoolz#415 CRSError for some datums).
- `GeoTIFF.dtype` mixed sample formats unsupported; single scalar nodata; `compression` property returns None for LERC/WebP/JPEG2000/LZMA even though decoding works (cosmetic).
- Open issues of note: #113 partial-band fetch; #86 avoid copy when window within one tile; #78 configurable coalesce range; #66 custom rayon pool in `fetch_tile/read`; #152 GCPs; #123 affine v3.
- Tests: rasterio-generated fixtures + `geotiff-test-data` submodule; comparisons vs rasterio for transform/CRS/read.
- Releases: 11 releases Jan-May 2026, none since; main only receives dependabot + one ruff commit since June.

### 4.2 Fit
- Nothing here is Rust; `assemble_tiles` + CRS construction + window math are ~600 lines of Python that a Rust engine would reimplement (trivial window math; CRS needs PROJ).

---------------------------------------------------------------------------
## 5. Contradictions between docs and code / across docs
1. lazycogs README pitches a "Rust-native stack" (rustac, async-geotiff, obstore) but the pixel path (warp, window math, mosaic, orchestration) is Python/numpy; Rust-warp PR never merged. [SRC]
2. lazycogs docs/performance.md says "lazycogs only fetches the pixel windows it actually needs" - true at window level, but async-geotiff always fetches+decodes whole COG tiles and all bands in the item request; and headers are re-fetched per read (no cross-chunk cache). [SRC]
3. async-tiff README lists JPEG2000/LERC/LZMA/WebP as supported compressions; default crate features do not enable them, and JPEG2000 has no tests; standalone JP2 is unsupported. [SRC]
4. async-tiff Cargo.toml `MIT OR Apache-2.0` vs LICENSE file MIT only. [SRC]
5. async-geotiff blog: "can be just as fast as any C or C++ library" [CLAIM] vs maintainers' own FOSS4G benchmark (decode 1.7x slower than libertiff at 16 threads, near parity at 1 thread) and issue #270 (flate2 slower than libdeflate). Blog also says LERC "not supported yet" (Feb 2026); LERC landed in async-tiff 0.3 / py 0.5-0.7.
6. lazycogs notebook compares unmatched configs (grouping, dtype, chunking) and shows differing output values; headline speedups are not apples-to-apples. [SRC]
7. async-tiff README feature "Tile request merging and concurrency" is delegated entirely to the `AsyncFileReader` impl; the Rust crate itself contains no merging or concurrency code (only `ObjectReader` -> object_store). [SRC]

---------------------------------------------------------------------------
## 6. Reuse verdicts for a Rust acquisition engine

- async-tiff (Rust crate): REUSE WITH UPSTREAM CONTRIBUTIONS (or vendor/fork if timelines clash). It already gives: async-trait reader abstraction with batch `get_byte_ranges`, header + all-IFD parse incl. BigTIFF and GeoKeys, tile-range planning, pluggable decoders, ndarray bridge, wasm target. Gaps to fill/own: windowed planner + overview choice + resampling/warp (none exists); decode-into-caller-buffer API (#203/#234) to avoid per-tile Vec + cast copies; striped TIFF, sparse tiles (#340), planar+predictor fix (#346/#348), tile-interleave, band-subset fetch, panic removal (`expect` in LZW/ifd), faster codecs (libdeflate, SIMD LZW/predictor; FOSS4G shows it trails libertiff), standalone JP2 (not in scope of this crate), configurable coalescing, cross-open header cache. Risks: pre-1.0 API churn, MSRV 1.88, bus factor ~1 (Kyle Barron), license ambiguity. Healthy: crate published, 35k recent downloads, active releases (Sept 2026), responsive maintainer, third-party Rust users (martin-core, eorst).
- async-geotiff: ONLY AS REFERENCE. Pure Python; nothing to link from Rust. Useful for: which GeoTIFF semantics matter (nodata, masks, alpha, PixelIsPoint half-pixel shift, flipped y, custom CRS PROJJSON), and for the tiepoint (I,J) pitfall.
- lazycogs: ONLY AS REFERENCE (design + issue tracker + benchmarks harness). No Rust to extract; Python/xarray-coupled; single-maintainer, idle since 2026-07-02, alpha. Good ideas to copy: per-chunk time-step item query, ordered drain with early-stop for first-valid mosaic, warp-map cache keyed on (window transform, crs), explain() dry-run plan, `errors="raise|ignore"` policy, dtype/nodata contract validation across items. Anti-patterns to avoid: no cross-chunk header cache, per-chunk-semaphore-only concurrency (no global budget), DuckDB single-worker item query on the hot path, whole-chunk-in-RAM materialisation, nodata-only masking (no SCL/QA).
- rust-warp: do not depend on it (AI-authored per its README, dormant, 5 stars) but its branch/spec shows the swap point and that pyproj+numpy warp is the dominant local cost (PR #58 numbers).
- Others: `tiff` crate = sync decode oracle/test reference (async-tiff already uses it as dev-dep); `geotiff`/`cog3pio` are not competitive as foundations.

Novelty implication [INFER]: no existing OSS project found in my scope implements "AOI + time + collection + bands -> pixels, in Rust, end to end with bounded memory and streaming sinks". async-tiff covers ~"open + fetch + decode" of the pipeline; everything between (planning, overview selection, warp, mosaic, scheduling, sinks) is Python or absent. Competing-in-spirit: odc-stac/odc-loader (GDAL), stackstac, lazycogs (Python, alpha), rastera (hobby), eorst (Rust+GDAL, LGPL, not inspected).

---------------------------------------------------------------------------
## 7. Open questions I could not close
- Real-network pixel-read throughput of async-tiff/async-geotiff vs GDAL (no public benchmark; would need a run, which is out of bounds here).
- Why lazycogs `test_native_crs_resolution` takes ~11 s on main (should be the no-warp fast path).
- Whether DuckDB per-chunk queries on a continental stac-geoparquet become the bottleneck (no data in repo; rustac side).
- eorst/rss_core internals (GitLab blocked).
- PR #328/#344 status for header cache on non-COG layouts.
- Whether obstore-sourced `PyObjectStore` stays ABI-compatible across async-tiff/obstore wheel versions (pyo3-object_store 0.12 pin).

---------------------------------------------------------------------------
## 8. URL index
- https://github.com/developmentseed/lazycogs (ARCHITECTURE.md, docs/performance.md, docs/notebooks/lazycogs-odc-stac.ipynb, src/lazycogs/*.py, tests/test_rasterio_parity.py, tests/benchmarks/)
- https://github.com/developmentseed/lazycogs/pull/58 (rust-warp draft, benchmark table)
- https://github.com/developmentseed/lazycogs/issues/89 (merge with odc-stac), /25, /30, /31, /42
- https://developmentseed.org/lazycogs/ ; https://developmentseed.org/blog/2026-07-02-groundwork-05/ ; https://developmentseed.org/blog/2026-09-28-behindthetech-henry/
- https://github.com/developmentseed/async-tiff ; https://crates.io/crates/async-tiff ; https://docs.rs/async-tiff ; https://pypi.org/project/async-tiff/
- https://github.com/developmentseed/async-tiff/issues/346 , /344 , /334 , /318 , /270 , /249 , /247 , /234 , /203 , /97 , /6 ; PRs /348 , /340 , /328
- https://github.com/developmentseed/async-geotiff ; https://pypi.org/project/async-geotiff/ ; https://developmentseed.org/async-geotiff/latest/blog/2026/02/03/introducing-async-geotiff/
- https://github.com/geospatial-jeff/pyasyncio-benchmark (25x claim source)
- https://github.com/weiji14/foss4g2025 ; https://talks.osgeo.org/foss4g-2025/talk/MRPVGL/ ; https://www.youtube.com/watch?v=iNNxXrvfHes
- https://github.com/jakenotjay/rust-warp ; https://github.com/chrieke/rastera ; https://github.com/weiji14/cog3pio ; https://github.com/georust/geotiff ; https://github.com/image-rs/image-tiff ; https://crates.io/crates/eorst
- https://github.com/stac-utils/rustac/blob/4095a77bb46602d87c4a9067313e209990c6f3d8/crates/duckdb/src/client.rs
- https://github.com/apache/arrow-rs-object-store/blob/main/src/util.rs (coalesce_ranges, 1 MiB / 10 parallel)
- https://github.com/jejjohnson/geotoolz/issues/414 (async-geotiff tiepoint I,J)
