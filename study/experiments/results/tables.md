## D1. Documented queries, reproduced (DuckDB v1.5.6, fresh connection per cold run)

| query | cold wall s (median, min-max) | HTTP GET | MiB in | waves | rows | warm s |
|---|---|---|---|---|---|---|
| readme_q1_tile_window_cloud | 4.1 (3.7-4.7) | [8] | [6.17] | [6, 7, 8] | [2] | 0.47 |
| readme_q2_assets_one_scene | 4.3 (3.1-4.7) | [6] | [12.26] | [5, 6] | [1] | 0.30 |
| readme_q3_glob_year_prune | 1.6 (1.5-1.9) | [10] | [0.19] | [3, 4] | [2] | 0.31 |
| readme_q4_stats_monthly | 3.2 (3.0-4.7) | [57] | [0.32] | [4, 5, 6] | [12] | 0.25 |
| readme_q5_stats_timeline | 0.7 (0.6-0.9) | [1] | [0.0] | [1, 2] | [1] | 0.42 |
| tile3mo | 5.1 (3.7-5.5) | [37] | [1.48] | [3, 4, 5] | [21] | - |
| assets_id | 2.8 (2.0-3.3) | [37] | [1.96] | [2, 3] | [1] | - |
| bbox_month5 | 3.3 (2.3-3.6) | [39] | [3.83] | [3, 5] | [20] | - |
| bbox_month5_extent | 3.6 (1.7-3.8) | [15] | [1.83] | [3, 5, 6] | [20] | - |
| bbox_nomonth | 5.0 (4.7-6.2) | [138] | [20.84] | [2, 3] | [155] | - |
| bbox_nomonth_extent | 4.1 (4.1-6.8) | [51] | [5.93] | [4] | [155] | - |
| year_tile | 3.7 (3.4-4.3) | [43] | [2.32] | [2, 3, 4] | [227] | - |

## D2. C1 `year=2025/items.parquet` structure

- 6.31 GB, 825 row groups, 5,065,361 rows, rows/group min-median-max [2705, 6144.0, 6144], footer 7.30 MiB
- sorted by `_tile`: True; row groups whose geometry lon-extent exceeds 180 degrees: 20 of 825
- bytes by column (MB): assets 5374.6, geometry 116.2, earthsearch:payload_id 88.5, links, list, element, href 54.4, bbox, list, element 37.9, view:sun_elevation 36.8

| predicate | outcome | stats-admitted rg | rg fetched | fetched outside admit set | GET | data MiB | footer MiB | rows |
|---|---|---|---|---|---|---|---|---|
| tile_eq | pruned | 1 | 1 | 0 | 6 | 0.04 | 7.55 | 174 |
| tile_eq_and_3mo | pruned | 1 | 1 | 0 | 7 | 0.07 | 7.55 | 45 |
| tile_range_between | pruned | 1 | 1 | 0 | 5 | 0.04 | 7.55 | 348 |
| datetime_3mo_only | interrupted at budget | 825 | 71 | 0 | 215 | 5.29 | 7.55 | - |
| st_intersects_geom | NOT pruned | 19 | 825 | 806 | 829 | 110.84 | 7.55 | 86 |
| st_intersects_extent | pruned | 19 | 19 | 0 | 23 | 3.55 | 7.55 | 86 |
| st_intersects_ocean_control | NOT pruned | 20 | 825 | 805 | 827 | 110.80 | 7.55 | 0 |
| st_intersects_extent_ocean_control | pruned | 20 | 20 | 0 | 26 | 3.84 | 7.55 | 220 |
| extent_and_exact | pruned | 19 | 19 | 0 | 23 | 3.55 | 7.55 | 86 |
| centroid_within_1deg | pruned | 21 | 21 | 0 | 48 | 0.98 | 7.55 | 866 |

## D3. Discovery for one tile + AOI footprint, 2025, cloud<=20 (36 scenes)

| path | wall s median (cold) | requests | MiB | notes |
|---|---|---|---|---|
| STAC API (full) | 0.46 | 2 pages | 0.61 | |
| STAC API (trimmed) | 0.50 | 2 pages | 0.49 | |
| GeoParquet https | 5.19 | 12 | 7.76 | warm repeat 0.43 s; waves [9, 10, 12] |
| GeoParquet https+assets | 6.17 | 13 | 14.15 | warm repeat 0.51 s; waves [10, 11, 12] |
| GeoParquet s3 | 2.99 | 12 | 7.76 | warm repeat 0.36 s; waves [8, 9, 10] |
| GeoParquet s3+assets | 4.08 | 13 | 14.15 | warm repeat 0.39 s; waves [7, 8, 10, 11, 12] |

Tile-only selection returns 68 rows; tile + footprint returns 36 = API's 36 (set-equal: only_in_api=0, only_in_gp=0).
Asset hrefs derived from id match the stored hrefs for 36/36 rows; distinct B04 grids across scenes: 1.

All years 2015-2026, GeoParquet cold, 12 files in parallel: wall ['6.5', '3.9', '6.1'] s, 94 GET, 48.3 MiB, 194 rows. Persisted slice: 7 KiB, query 0.5 ms. STAC API all years: ['1.14', '1.02', '1.23'] s, 2 pages, 3.27 MiB, 195 items. Set difference: only_api=1 (['S2A_T17SPV_20261001T161236_L2A']), only_gp=0.

## L. Layout index (one 32 KiB range GET per COG)

- 72 assets: 72 GETs, 2.36 MB, wall 1.52 s at concurrency 16; extra fetches needed: 0
- header bytes actually required (min/median/max): red 4204/4206/4208, nir 4204/4206/4208, scl 4044/4046/4050

## Planner (SQL), W_A = 400x400 px in one block, W_B = 600x600 px over a 4-block corner

| gap (bytes) | W_A requests | W_A wire MB | W_B requests (24 scenes) | W_B wire MB | W_B payload MB |
|---|---|---|---|---|---|
| 0 | 48 | 75.48 | 192 | 295.56 | 295.56 |
| 8 | 48 | 75.48 | 96 | 295.56 | 295.56 |
| 1024 | 48 | 75.48 | 96 | 295.56 | 295.56 |
| 65536 | 48 | 75.48 | 96 | 295.56 | 295.56 |
| 1048576 | 48 | 75.48 | 96 | 295.56 | 295.56 |
| 8388608 | 48 | 75.48 | 96 | 295.56 | 295.56 |

Planning time, 72 assets x 2 windows: median 7.4 ms. Scale-up: 4800 assets -> 14400 manifest rows in 77 ms; 48000 assets -> 144000 manifest rows in 513 ms

## P. W_A_chip400, 24 scenes x B04+B08 (4 interleaved reps, fresh process each)

| config | t_work median s [min-max] | requests | bytes requested MB | NIC min MB (all runs) | pixels vs GDAL oracle | ratio to best floor (95% boot CI) |
|---|---|---|---|---|---|---|
| gdal-default-T4 | 55.1 [49.7-62.7] | [600] | [77.07] | 84.1 | ['48/48'] | 19.03x (15.11-24.89) |
| gdal-tuned-T4 | 12.9 [10.1-35.5] | [144] | [77.86] | 81.4 | ['48/48'] | 4.45x (3.33-12.27) |
| gdal-tuned-T16 | 4.0 [3.3-5.5] | [144] | [77.86] | 81.8 | ['48/48'] | 1.39x (1.09-1.98) |
| gdal-tuned-T32 | 4.8 [4.0-5.1] | [144] | [77.86] | 81.5 | ['48/48'] | 1.66x (1.28-2.07) |
| floor-C16 | 3.0 [2.4-3.3] | [48] | [75.48] | 79.4 | n/a (separate decode check) | 1.04x (0.77-1.35) |
| floor-C32 | 2.9 [2.4-3.4] | [48] | [75.48] | 78.8 | n/a (separate decode check) | 1.00x (0.73-1.36) |
| floor-C64 | 5.8 [4.8-9.4] | [48] | [75.48] | 79.2 | n/a (separate decode check) | 2.01x (1.48-3.28) |
| rasteret-read | 4.3 [3.0-8.0] | [48] | [75.48] | 79.3 | ['48/48'] | 1.48x (0.90-2.81) |

Like-for-like (same concurrency) ratios of median t_work, bootstrap 95% interval (n is small: indicative):
- gdal-tuned-T16 / floor-C16: 1.34x (1.09-2.00)
- gdal-tuned-T32 / floor-C32: 1.66x (1.28-2.07)
- rasteret-read / floor-C32: 1.48x (0.90-2.81)
- gdal-tuned-T16 / gdal-tuned-T4: 0.31x (0.11-0.45)

Best floor config = floor-C32. NIC counter includes TLS/HTTP framing and other machine traffic; the minimum across runs is the cleanest bound. `*` see e6b.

## P. W_B_corner600, 12 scenes x B04+B08 (3 interleaved reps, fresh process each)

| config | t_work median s [min-max] | requests | bytes requested MB | NIC min MB (all runs) | pixels vs GDAL oracle | ratio to best floor (95% boot CI) |
|---|---|---|---|---|---|---|
| gdal-default-T4 | 38.6 [28.5-40.3] | [324] | [151.18] | 158.4 | ['24/24'] | 7.15x (3.87-7.89) |
| gdal-tuned-T16 | 10.9 [10.6-11.5] | ['96*'] | [151.57] | 160.1 | ['24/24'] | 2.02x (1.44-2.24) |
| floor-C16 | 9.6 [6.7-12.4] | [48] | [150.79] | 157.3 | n/a (separate decode check) | 1.77x (0.90-2.42) |
| floor-C32 | 5.4 [5.1-7.4] | [48] | [150.79] | 158.7 | n/a (separate decode check) | 1.00x (0.69-1.44) |
| rasteret-read | 9.4 [8.0-9.5] | [48] | [150.79] | 157.9 | ['24/24'] | 1.74x (1.09-1.86) |

Like-for-like (same concurrency) ratios of median t_work, bootstrap 95% interval (n is small: indicative):
- gdal-tuned-T16 / floor-C16: 1.14x (0.85-1.72)
- rasteret-read / floor-C32: 1.74x (1.09-1.86)

Best floor config = floor-C32. NIC counter includes TLS/HTTP framing and other machine traffic; the minimum across runs is the cleanest bound. `*` see e6b.

## duckdb-cog 0.3.0 (DuckDB 1.5.5), same assets and window

| task | t_work s median [min-max] | requests | MB | check |
|---|---|---|---|---|
| meta/default | 0.7 [0.6-0.8] | 1 (HEAD 0) | 0.03 | tile rows 291 |
| window/default | 28.1 [28.1-122.7] | 96 (HEAD 0) | 77.05 | 48/48 |
| window/knobs | 91.6 [91.6-91.6] | 96 (HEAD 0) | 77.05 | 48/48 |
| zonal/default | 13.9 [13.7-15.4] | 48 (HEAD 0) | 36.48 | max err 0.0 |
| zonal/knobs | 17.1 [17.1-17.1] | 48 (HEAD 0) | 36.48 | max err 0.0 |
| search/default | 4.3 [4.1-4.9] | 0 (HEAD 0) | 0.00 | rows 1978 items 86 |
