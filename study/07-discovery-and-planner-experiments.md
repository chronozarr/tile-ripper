# 07 — s2-stac-geoparquet, duckdb-cog, a DuckDB planner, Rasteret and tuned GDAL: measured

Date: 2026-10-09/10 · Status: **measurements, nothing built.** The probes in [`experiments/`](experiments/) are about 1,500 lines of throwaway scripts; the "planner" is one SQL query plus a 15-line decode check. No engine, no fork, no production code touched.

Evidence grades as in the README: **[MEAS]** I ran it (n and session stated), **[SRC]** read the code, **[INFER]** inference. Every number below comes from `experiments/results/*.json`; the full generated tables are in [`experiments/results/tables.md`](experiments/results/tables.md).

## 0. Bottom line

1. **Attribution.** `s2-stac-geoparquet` is Chris Holmes' work (`cholmes` is the only contributor, 232 commits; hosted under `taylor-geospatial`, created 2026-09-15). **`duckdb-cog` is not his**: it is a one-person project (`st-layer`, 65 commits, 4 stars, created 2026-07-05).
2. **s2-stac-geoparquet is a discovery component, and only that.** It improves nothing about pixel retrieval. As a *live* discovery path it loses to the STAC API for one tile-year (0.46 s vs 3.0 s cold). It earns its place as a frozen, offline, reproducible snapshot, if used with the recipe in §2.4 (five of its documented patterns are silently wrong or slow for a planner).
3. **All documented queries reproduce.** Request and byte counts match the docs within ~10% and are exactly deterministic run to run (§2.1). The one documented claim that needs a footnote: "geometry statistics are not used for pruning" holds only for plain `ST_Intersects`; `ST_Intersects_Extent` does prune (§2.2).
4. **For tile queries the cost is the footer, not the data**: 7.3 MiB of footer to read 0.04 MiB of rows. The published `items.idx.json` sidecar exists to remove that, but only the JS explorer can use it; DuckDB cannot.
5. **A DuckDB-driven planner can generate correct, cheap manifests** (§4): 8 ms for 72 assets, 0.5 s for 48,000, and the decoded pixels are bit-exact against GDAL on 72/72 assets across two windows. What it needs and nothing publishes is the tile-offset layout (one 32 KiB GET per COG, §3).
6. **But efficient manifests did not translate into a detectable wall-time win on this machine** (§5). Tuned GDAL at the same concurrency is 1.1–1.4× the manifest executor; Rasteret makes the identical requests as the manifest; the CIs are wide, one session. The big, certain effects are elsewhere: untuned GDAL is **14× slower** (55 s vs 4.0 s, 600 vs 144 requests), and 4 workers are 3.2× slower than 16.
7. **duckdb-cog is not a Tile-Ripper component** (§6): no function exposes tile offsets, its window path is serial (28 s for what GDAL does in 4 s), `read_stac_search` has no cloud filter, and the extension exists only for DuckDB 1.5.5 (not the current 1.5.6).

## 1. What was run, and its limits

- **Machine/network:** MacBook (Apple silicon) on the user's network, Chapel Hill. Measured link (E0): keep-alive request 116 ms to S3 us-west-2, 293 ms to source.coop; single stream 2–5 MB/s; aggregate peaks 12–30 MB/s and varies several-fold between runs (tuned GDAL with 4 threads, identical bytes and requests, took 10.1 s, 13.5 s and 35.5 s in successive repetitions). **Regime: bandwidth-bound and noisy**, as the prior study predicted.
- **Workload (frozen, sha256 `59f0fc4d…`):** tile 17SPV (Chapel Hill), Earth Search Collection 1, 2025, 24 scenes (footprint ∩ AOI, cloud ≤ 20, nodata ≤ 5), bands B04+B08. **W_A**: 400×400 px inside one 1024² block (48 assets, 75.5 MB of tiles). **W_B**: 600×600 px over a 4-block corner (24 assets, 12 scenes, 150.8 MB). Native grid, no warp, no overviews.
- **Tools/versions:** DuckDB 1.5.6 (1.5.5 for duckdb-cog), GDAL 3.12.2 via rasterio 1.5.2, Rasteret 0.4.1 (isolated env: it pins `rasterio<1.5`, i.e. GDAL ≤ 3.10; it uses rasterio only for masking, not for its reads), httpx for the floor executor. Three uv environments, all locked, none overriding your 7-day quarantine.
- **Method:** every pixel run is a fresh process; tool configs interleaved in a new random order each repetition; hashes of every window compared with the tuned-GDAL oracle; request counts from tool-native logs; bytes cross-checked against the `en0` NIC counter (cleanest runs are 4–9% above requested bytes from TLS/HTTP framing and unattributable machine traffic, looser than the spec's ±5%).
- **Not claim-grade, by the spec's own rule:** one machine, one network, one evening session, n = 3–5 repetitions. Wall-time differences below ~1.5× between tuned tools are **indistinguishable from the link's variance**. Request counts, byte counts and pixel equality are exact and are the primary evidence.
- **Not tested:** the cloud-near-data regime (the one the prior study said could differ), Planetary Computer/CDSE, JP2, reprojection, overviews, anything above 150 MB per run. One transient `ECONNRESET` during the first manifest-fetch test (the next three runs were clean; the floor executor has no retry by design), and one orphaned worker from a killed accounting run that I found and stopped while the duckdb-cog runs were in progress (it was hung on a closed pipe, not using the network, but treat the duckdb-cog outliers, e.g. a 122.7 s window run, accordingly).
- **Instrument bug found and fixed:** GDAL's debug log drops lines emitted from its own decode threads, so request counts for multi-block windows under `GDAL_NUM_THREADS=ALL_CPUS` were wrong (48 instead of 96). A control with `GDAL_NUM_THREADS=1` fixed the count (`experiments/e6b_gdal_accounting.py`).

## 2. D — discovery

### 2.1 The documented queries reproduce

README, AGENTS.md and `docs/query-performance.md` (L2A 2026 part, tile 31UFU), cold = fresh DuckDB connection, N = 5 (README) / 3 (doc method), [MEAS]:

| query | here: GET / MiB | doc (2026-09-19) | wall here (median) |
|---|---|---|---|
| README tile+window+cloud (C1 2021) | 8 / 6.17 | – | 4.1 s |
| README assets of one scene | 6 / 12.26 | – | 4.3 s |
| README `s3://` glob, year-pruned | 10 / 0.19 | – | 1.6 s |
| README stats-c1 monthly (12 rows) | **57** / 0.32 | "one row group range read" | 3.2 s |
| doc: tile + 3 months | 37 / 1.48 | 40 / 1.4 | 5.1 s |
| doc: assets by id | 37 / 1.96 | 37 / 1.9 | 2.8 s |
| doc: bbox, one month | 39 / 3.83 | 39 / 3.8 | 3.3 s |
| doc: full-year tile history | 43 / 2.32 | 46 / 2.3 | 3.7 s |
| doc: bbox, no month filter | 138 / 20.84 | 111 / 20.4 | 5.0 s |

Counts were identical on every repetition. Wall times are 2–3× lower than the doc's Copenhagen numbers (different path); warm repeats are 0.2–0.5 s with 0 GETs, as documented. Two points the docs don't make: the 57 GETs are 50 bloom-filter probes of 47 bytes (one per row group, because `year = 2021` cannot be excluded by min/max), and the doc's last row grew from 111 to 138 GETs because the part has more row groups since the 3 October consolidation.

### 2.2 Partitioning and pruning (C1 `year=2025/items.parquet`, [MEAS])

6.31 GB, 825 row groups of ≤ 6,144 rows, 5,065,361 rows, **7.3 MiB footer**, sorted by `_tile`. **`assets` is 5.37 GB of the 6.31 GB (85%).** `_tile` carries a bloom filter. Pruning is verified by mapping every HTTP range DuckDB fetched onto column chunks and checking whether any fetched row group lies *outside* the set the footer statistics admit:

| predicate | outcome | rg admitted by stats | rg fetched | GET | data MiB | footer MiB |
|---|---|---|---|---|---|---|
| `_tile = '17SPV'` | pruned | 1 | 1 | 6 | **0.04** | 7.55 |
| `_tile = … AND datetime in 3 months` | pruned | 1 | 1 | 7 | 0.07 | 7.55 |
| `_tile BETWEEN '17SPU' AND '17SPV'` | pruned | 1 | 1 | 5 | 0.04 | 7.55 |
| `datetime` range alone | **cannot prune** | 825 | ≥ 71 (stopped at 30 s) | 215+ | – | 7.55 |
| `ST_Intersects(geometry, env)` | **not pruned** | 19 | **825** | 829 | **110.8** | 7.55 |
| `ST_Intersects_Extent(geometry, env)` | pruned | 19 | 19 | 23 | 3.55 | 7.55 |
| `ST_Intersects_Extent AND ST_Intersects` | pruned | 19 | 19 | 23 | 3.55 | 7.55 |
| centroid struct range (±1°) | pruned | 21 | 21 | 48 | 0.98 | 7.55 |

- **The doc's pruning claim holds for plain `ST_Intersects` on DuckDB 1.5.6, but `ST_Intersects_Extent` fixes it on these files** (the prior study only inferred this from another project's testbed). Same 86 rows. On the doc's L2A part: bbox/month 39 → 15 GET, 3.83 → 1.83 MiB; bbox/no-month 138 → 51 GET, 20.84 → 5.93 MiB.
- **Geometry-stats pruning is polluted by the antimeridian.** 20 of 825 row groups have a longitude extent > 180° (scenes whose CRS84 footprint wraps ±180). They are admitted for *every* AOI: of the 19 row groups admitted for Chapel Hill, **2 are legitimate and 17 are wrapped-footprint junk**. `_tile` is ~90× cheaper in bytes than the best spatial predicate (0.04 vs 3.55 MiB).
- **Unpruned scans are slow for a DuckDB-specific reason, not a host one.** A multi-column filter-then-fetch scan ran at 5–7 requests/s with average in-flight concurrency ~1, on both doors and for 2, 16 and 32 threads (fewer threads even gave *more* requests). A one-column scan (geometry) ran ~66 requests/s. A planner must never reach the unpruned path.

### 2.3 Discovery compared (one tile + AOI footprint, 2025, cloud ≤ 20 → 36 scenes), [MEAS]

| path | cold wall | requests | MiB |
|---|---|---|---|
| **STAC API** (Earth Search v1) | **0.46 s** | 2 pages | 0.61 |
| GeoParquet, `s3://` door | 3.0 s (warm 0.36) | 12 | 7.76 |
| GeoParquet, documented `https://` door | 5.2 s (warm 0.43) | 12 | 7.76 |
| …plus the `assets` column | 4.1 / 6.2 s | 13 | 14.15 |
| **All years 2015–2026**, GeoParquet, 12 files in parallel | 3.9–6.5 s | 94 | 48.3 |
| same, STAC API (paged) | **1.14 s** | 2 pages | 3.27 |
| same, persisted slice, local DuckDB | **0.5 ms** | 0 | 0.007 |

The `s3://us-west-2.opendata.source.coop` door is **1.6–1.9× faster than `https://data.source.coop`** for identical requests and bytes (median request 126–210 ms vs 400–520 ms; the https door's `server-timing` shows ~330 ms of proxy "backend" time per request even for 80-byte ranges).

### 2.4 Recipe, and the five traps found

For an AOI × time job, a correct and cheap GeoParquet discovery is:
`s3://` door → `year=*/*.parquet` **glob with `id` dedupe** (keep highest `s2:generation_time`) → `_tile IN (…) AND ST_Intersects_Extent(geometry, aoi) AND ST_Intersects(geometry, aoi)` → select only needed columns, **not `assets`** → persist the slice.

| trap | evidence |
|---|---|
| `_tile` alone over-selects ×1.9 | 68 rows vs the API's 36; the 32 extras are partial scenes (median **87.6% nodata**) whose footprint misses the AOI. With the footprint predicate the result is **set-equal** to the API's, for +0.11 MiB |
| `items.parquet` alone misses the live tail | the README examples name `items.parquet`; 2026 also has `live-09.parquet` (139k rows) and `live-10.parquet` (117k). The API had 195 scenes, my archive-only query 194; the missing one is in `live-10`. For a last-60-days job most of the data is in the tails |
| plain `ST_Intersects` reads the whole geometry column | table above |
| the `assets` column is a 6 MiB-per-row-group tax | asset hrefs **derive exactly from `id` + datetime** (36/36 rows) and B04's grid is identical for every scene; use verify-on-use (a 404 falls back to `assets`). The doc says "don't template"; the evidence says templating is safe *if verified* |
| the `https` door is ~1.7× slower | §2.3 |

**Maturity:** one contributor, 24 days old at test, daily refresh, hosted on Source Cooperative; the schema deviates from the stac-geoparquet spec (JSON-string `assets`, `list<double>` bbox, per the prior study). Fine as a cache; do not make it the only path.

## 3. L — the layout index nothing publishes

Neither the STAC API, stac-geoparquet, nor any catalog carries `TileOffsets`/`TileByteCounts`. A request manifest needs them. [MEAS] one ranged GET of the first 32 KiB per COG:

- **72 assets: 72 GETs, 2.36 MB, 1.49 s at concurrency 16; zero extra fetches.** The header actually needed is **4.2 KB** (all 5 IFDs and all tile arrays), so even GDAL's default 16 KiB first read always suffices.
- All B04/B08: 1024² tiles, deflate + horizontal predictor, 121 tiles, classic little-endian TIFF, tiepoint (0,0) → (600000, 4000020); SCL: 512² tiles. Tiles are separated by exactly 8 bytes. Index size: 221 KB of Parquet for 72 files.
- **Rasteret's equivalent build: 500 GETs for 50 assets (10 sequential tiny GETs per asset), 96 KB, 5.2 s**, IFD0 only (no overviews).

## 4. The DuckDB planner (SQL only), [MEAS]

`window (native pixels) → tile indices → offsets[idx], counts[idx] → per-object gap coalescing → manifest` is one query ([`experiments/e5_planner.py`](experiments/e5_planner.py)).

| | W_A (1 block) | W_B (4-block corner), 24 scenes |
|---|---|---|
| requests, gap = 0 | 48 | **192** (an 8-byte gap is never merged) |
| requests, gap ≥ 8 B (to 8 MiB) | 48 | **96** (one per tile row; rows are ~16 MB apart, so no gap setting merges them) |
| wire = payload bytes | 75.48 MB | 295.56 MB |
| planning time | 8 ms (72 assets) | **0.5 s for 48,000 assets × 2 windows** → 144,000 manifest rows, ~linear |

**Correctness:** the manifest, executed by a bare range fetcher and decoded with zlib + a numpy `cumsum`, is **bit-exact against GDAL: W_A 48/48, W_B 24/24 assets** (the W_B case exercises merged multi-tile ranges and a 4-block window). Decode of all 48 tiles takes 0.33 s serially (~7 ms per 1024² tile).

**Efficiency limits it cannot fix:** at tile granularity the useful pixels are **15%** (W_A) and **8.6%** (W_B) of the fetched tile bytes. That over-read is inherent to COG tiles; only overviews, partial-tile reads or a different format reduce it.

## 5. P — pixel retrieval head-to-head, [MEAS]

W_A, 24 scenes × B04+B08, 4 interleaved cold-process repetitions. `t_work` excludes interpreter start and import. "×floor" is the median ratio to the best floor configuration (C=32), bootstrap 95% interval in brackets, **n = 4: indicative only**.

| tool | t_work median [min–max] | requests | bytes | pixels vs GDAL | ×floor |
|---|---|---|---|---|---|
| GDAL **default**, 4 threads | **55.1 s** [49.7–62.7] | **600** | 77.07 MB | 48/48 | 19× (15–25) |
| GDAL tuned, 4 threads | 12.9 s [10.1–35.5] | 144 | 77.86 MB | 48/48 | 4.5× (3.3–12) |
| GDAL tuned, 16 threads | **4.0 s** [3.3–5.5] | 144 | 77.86 MB | 48/48 | 1.39× (1.09–1.98) |
| GDAL tuned, 32 threads | 4.8 s [4.0–5.1] | 144 | 77.86 MB | 48/48 | 1.66× |
| manifest floor, C = 16 / 32 / 64 | 3.0 / **2.9** / 5.8 s | **48** | 75.48 MB | 48/48 (decode check) | 1.04× / 1.00× / 2.0× |
| Rasteret 0.4.1, index persisted | 4.3 s [3.0–8.0] | **48** | 75.48 MB | 48/48 | 1.48× (0.90–2.81) |

W_B (12 scenes, 4-block corner, 3 reps): default GDAL 38.6 s / 324 requests; tuned GDAL T16 10.9 s / 96 requests; floor C16 9.6 s and C32 5.4 s / 48 requests; Rasteret 9.4 s / 48 requests. All 24/24 bit-exact.

What this shows:

1. **Untuned GDAL is the single biggest, most certain effect:** 12.5 requests per asset in the batch (13 for a single file; the directory listing is shared per scene) instead of 3. The ~10 extras are a directory-listing GET and nine sidecar probes (`.aux.xml`, `.aux`, `.AUX`, `.msk`, `.MSK`, …), all 404. `GDAL_DISABLE_READDIR_ON_OPEN=EMPTY_DIR` + `CPL_VSIL_CURL_ALLOWED_EXTENSIONS` remove them. **Your `chronozarr/examples/sentinel2_pc/mosaic.py` already sets both**; what it does not match is the worker count (4, my "tuned T4" row), which was 3.2× slower than 16 here (ratio 0.31×, interval 0.11–0.45). That was Earth Search over S3, not Planetary Computer over Azure.
2. **At the same concurrency, tuned GDAL is within ~1.1–1.4× of the manifest executor** (T16/C16: 1.34× [1.09–2.00] on W_A, 1.14× [0.85–1.72] on W_B). Consistent with GDAL's extra HEAD + header GET per asset, but not claim-grade.
3. **Rasteret's read = the manifest's read:** identical 48 requests and 75.48 MB; no detectable time difference from tuned GDAL or the floor. Its costs sit in the build (10 requests/asset, IFD0 only), a Python ≥ 3.12 and `rasterio<1.5` pin, and no overview support.
4. **Over-concurrency hurts on this link:** floor C64 was 2.0× slower than C32, GDAL T32 slower than T16, and Rasteret (default 50 in flight) 1.5–1.7× slower than floor C32. The optimum here is 16–32. Nothing adapts; this is the one place a measurement supports the prior study's "adaptive per-host concurrency" gap, **but the optimum is specific to this link.**
5. **Bytes:** GDAL requests 3.1% more than the manifest (16 KiB chunk rounding plus a 32 KiB header per asset); nobody reduces tile bytes.

## 6. duckdb-cog 0.3.0, [MEAS] (DuckDB 1.5.5 environment)

| task | t_work | requests | check |
|---|---|---|---|
| `read_cog` tile grid of one B04, all 5 levels (291 rows) | 0.7 s | **1 GET, 32 KiB**, no HEAD | – |
| `RS_BandAsArray`, 48 assets, W_A | **28.1 s** [28.1–122.7] | 96 (2 per asset) | 48/48 bit-exact |
| same, with `COG_IO_THREADS`/`COG_FETCH_CONCURRENCY` raised | 91.6 s | 96 | no help |
| `RS_ZonalStats` mean, one call per scene, 24 scenes | 13.9 s [13.7–15.4] | 48 | max error 0.0 vs numpy |
| `read_stac_search` for the AOI, 2025 | 4.3 s | – | 86 items (no cloud filter possible), columns `item_id, datetime, href` only |

- **No function mentions offsets, byte counts or ranges** (27 functions listed; [SRC] and [MEAS] agree), and `read_cog` returns tile *geometry* only. It cannot emit a manifest.
- Correct (bit-exact; zonal means exact) and lean on metadata (1 GET per COG). But ~7× slower than tuned GDAL on the same bytes, and the concurrency knobs do not change it on this link.
- **Built for DuckDB 1.5.5 only** (HTTP 404 for 1.5.3, 1.5.4, **1.5.6**, 1.6.0): anyone on current DuckDB cannot `INSTALL cog`. Single maintainer; v0.3.0 published 2026-07-30 (passes the 7-day quarantine).
- Its README benchmarks (95 ms zonal series) are local/warm; they say nothing about remote bulk acquisition, which it does not claim to do.

## 7. Discovery improvements vs pixel-retrieval improvements, separated

| lever | stage | measured effect | needs an engine? |
|---|---|---|---|
| `s3://` door instead of `https://` | D | cold 5.2 → 3.0 s | no |
| persist the slice / footers | D | 3.9–6.5 s + 48 MiB → 0.5 ms + 7 KiB (12 years) | no |
| `_tile` ∧ footprint, glob live tails, dedupe | D (correctness) | 68 → 36 scenes; 194 → 195 | no |
| `ST_Intersects_Extent` for AOI-only | D | 110.8 → 3.55 MiB, 829 → 23 GET | no |
| derive hrefs, skip `assets` | D | −6.4 MiB per query | no |
| STAC API for single tile-year | D | 0.46 s vs 3.0 s cold | no |
| 1 GET per COG, persisted by ETag | L | 72 assets in 1.5 s; Rasteret 10× the requests | no (one script) |
| manifest instead of per-open HEAD+header | L → P | 144 → 48 requests; time 1.1–1.4×, not claim-grade | no (SQL + fetcher) |
| fix untuned GDAL (EMPTY_DIR + allowed ext) | P | 55 → 12.9 s, 600 → 144 requests | no (config) |
| 4 → 16 workers | P | 12.9 → 4.0 s | no (config) |
| avoid over-concurrency | P | C64 2.0× slower than C32 (this link) | adaptive controller (untested) |
| fewer tile bytes | P | none available at tile granularity (15% / 8.6% useful) | overviews / partial tiles / other format |

Discovery is a **0.5–5 s** problem per job that caching reduces to milliseconds; pixels are **3–55 s** on this link and are dominated by bandwidth and worker count. In the W_A job (first use, no caches): D ≈ 0.5 s (API) + L ≈ 1 s + P ≈ 3–4 s. P dominates; D matters only for multi-tile or multi-year jobs without a cache.

## 8. What this changes in the earlier study

- **Gate G0** (tuned baseline within ~1.4× of the floor ⇒ no engine): on this laptop, tuned GDAL at equal concurrency is 1.1–1.4× the floor and Rasteret is indistinguishable. That leans NO-GO *for an engine in this regime*. **It does not settle the cloud-near-data regime**, which G0 also covered and which is still unrun.
- Verified, not just inferred: `ST_Intersects_Extent` pruning; the planner producing correct manifests; one GET per COG sufficing for the layout.
- Corrected: "tuned GDAL needs 1 GET before the tile" was 2 (HEAD + header GET). Rasteret's "GDAL makes 3–6 requests per file" is right for tuned GDAL (3) and understated for default GDAL (13).
- Newly found: antimeridian pollution of geo-stats pruning; live-tail omission; the https-vs-s3 door gap; duckdb-cog's single-DuckDB-version build.

## 9. Decisions for you

1. **Cloud-near-data run.** The only experiment that can reopen the engine question is a same-region (us-west-2) repeat with a fast NIC, where request count and concurrency matter more than a 100 Mbit/s link. It is a few dollars at most. This is open decision #3 of the earlier study; approve or decline.
2. **No-engine wins for chronozarr ingest now:** sweep worker count above 4 in `mosaic.py`; use the discovery recipe in §2.4 if you want an offline discovery path. Both are small and independent of the engine decision. (Re-measure on Planetary Computer before trusting the worker-count number there.)
3. **Upstream notes (not sent, outward-facing):** to Chris Holmes' repo (README examples miss live tails; `ST_Intersects_Extent` prunes and halves bytes; antimeridian-wrapped footprints pollute row-group extents) and to duckdb-cog (rebuild for current DuckDB). Say if you want drafts.

## 10. Unresolved

- Do the 1.1–1.4× tuned-GDAL/manifest gaps survive a second session and the cloud regime?
- Is the 16–32 concurrency optimum a property of this link or of S3 per-connection fairness?
- Does the antimeridian pollution also hit `sentinel-2-l2a` parts?
- Planetary Computer (512² blocks, no predictor) and CDSE (JP2) were not touched.

## 11. Reproduce

`cd study/experiments && uv sync` (main), `rasteret-env/` and `cog-env/` likewise; then `e0`…`e8` in order. Index and details: [`experiments/README.md`](experiments/README.md).
