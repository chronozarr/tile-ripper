# experiments/ — measurement probes for study §07

Throwaway scripts that produced the numbers in [`../07-discovery-and-planner-experiments.md`](../07-discovery-and-planner-experiments.md). **Not an engine.** Everything runs against public data with an identifying `User-Agent`, bounded volume (~6 GB total from public S3), no credentials.

## Environments (three, all locked; the uv 7-day quarantine applies, nothing overridden)

| dir | python deps | why separate |
|---|---|---|
| `.` | duckdb 1.5.6, rasterio 1.5.2 (GDAL 3.12.2), pyarrow, numpy, httpx | main probes, GDAL baseline, floor executor |
| `rasteret-env/` | rasteret 0.4.1 (pins `rasterio<1.5`, GDAL 3.10.3) | Rasteret cannot share an env with current GDAL |
| `cog-env/` | duckdb 1.5.5 | the `cog` community extension exists only for DuckDB 1.5.5 |

Set up with `uv sync` in each directory. Scripts are trusted code run as `.venv/bin/python script.py` (not `-I`: they import sibling modules); downloaded data lives outside this directory.

## Order and what each answers

| script | answers | main output |
|---|---|---|
| `e0_network.py` | which regime is this machine in | `results/e0_network.json` |
| `e1_documented_queries.py`, `e1b_doc_method.py` | do the README / AGENTS / `query-performance.md` queries reproduce | `e1*.json` |
| `e2_rowgroups.py` | footer structure; which predicates prune (HTTP ranges mapped to column chunks) | `e2_rowgroups.json`, `meta_c1_2025.parquet` |
| `e3_discovery.py`, `e3b_multiyear.py` | API vs GeoParquet, set equality, href derivation, frozen scene list | `e3*.json`, `scenes_17SPV_2025.json` |
| `e4_layout.py` + `tiffscan.py` | one-GET layout index (offsets/byte counts) | `layout_17SPV_2025.parquet` |
| `e5_planner.py` + `planwin.py` | SQL manifest planner, gap sweep, scale-up | `manifest_*_gap8.parquet`, `e5_planner.json` |
| `e6_pixels.py` (+ `w_gdal.py`, `w_floor.py`, `w_rasteret.py`), `e6b_gdal_accounting.py` | pixel head-to-head, bit-exact vs GDAL, request accounting control | `e6_pixels_*.json`, `oracle_*.json` |
| `e7_cog_install.py`, `e7_cog_runs.py` (+ `w_cog.py`) | duckdb-cog surface and tasks | `e7*.json` |
| `e8_tables.py` | all tables of §07, generated from the JSON | `results/tables.md` |

Pixel runs: `python e6_pixels.py W_A_chip400 24 4` and `python e6_pixels.py W_B_corner600 12 3 gdal-default-T4,gdal-tuned-T16,floor-C16,floor-C32,rasteret-read`. Rasteret needs its index first: `rasteret-env/.venv/bin/python w_rasteret.py --phase build --workspace <dir> --frozen results/scenes_17SPV_2025.json` and `RASTERET_WS` in `e6_pixels.py` pointed at that directory.

## Gotchas learned the hard way

- Pipe a long Python job through `tail` and you see nothing until it ends (block buffering): use `python -u` and a file.
- `pkill` on an orchestrator leaves its child workers running; kill the children or the process group.
- GDAL's debug log (what rasterio exposes) loses lines emitted from GDAL's own decode threads; count requests with `GDAL_NUM_THREADS=1` or at the proxy.
- `fetch_arrow_table()` avoids DuckDB's `pytz` requirement for `TIMESTAMPTZ`.
- Everything here is one network, one session; treat sub-1.5× wall-time differences as noise.
