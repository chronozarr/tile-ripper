# 02 - STAC / metadata-discovery layer (agent 2 of 8)

Date of study: 2026-10-09. Evidence grades: [SRC] read code/docs myself, [BENCH] reproducible benchmark (code+data+method), [CLAIM] authors' assertion, [INFER] my inference, [MEAS] my own one-off single-request curl measurement (not a benchmark; one sample, from an unknown-region host, 2026-10-09 ~19:40-20:00 UTC).

Clones (read-only, untrusted) live in scratchpad/repos/: rustac, rustac-py, stac-geoparquet, stac-geoparquet-spec, pystac-client, s2-stac-geoparquet (TGE mirror), pc-apis, stac-server, rasteret/odc-stac/async-tiff/gdal (shared with sibling agents). Raw API responses I fetched are in scratchpad/data/.

## 0. Bottom line

1. Discovery is a SOLVED problem for Sentinel-2 L2A at the "list items for AOI+time+cloud" level. One STAC API page returns a year of items for a small AOI in ~0.3-3 s [MEAS]. Nothing about Tile-Ripper's novelty lives here; it should adopt, not build.
2. The ecosystem does NOT provide what an acquisition planner needs from the discovery layer: (a) COG block size / tile offsets / byte counts are in no STAC catalog (Earth Search, PC, CDSE) and in no STAC extension (raster v2.0.0 has no tile fields); (b) concurrent/windowed paging; (c) catalog normalisation (grid:code vs s2:mgrs_tile, proj:epsg vs proj:code, raster:bands vs flattened raster:*, COG vs JP2, SAS signing); (d) spatial row-group pruning in DuckDB via stac-duckdb (it does not use the bbox covering columns).
3. The one existing project that closes gap (a) is **rasteret** (Python, Apache-2.0): stores parsed COG header (tile_offsets, tile_byte_counts, tile size, compression...) per band in a Parquet collection. Its headline speedups are [CLAIM]-with-notebook and EXCLUDE the one-time header-parse build cost.
4. Rust STAC stack (stac-rs, now "rustac") is real and active but single-maintainer, pre-1.0, and its API client pages strictly sequentially. Good enough to adopt for types + a thin search client; do not expect throughput features.

## 1. rustac / stac-rs

### 1.1 Repo facts
- Repo renamed from stac-rs on 2025-04-17 [SRC https://github.com/stac-utils/rustac/blob/4095a77bb46602d87c4a9067313e209990c6f3d8/docs/history.md].
- License: dual MIT OR Apache-2.0 [SRC Cargo.toml `license = "MIT OR Apache-2.0"`, files LICENSE-MIT + LICENSE-APACHE in https://github.com/stac-utils/rustac/tree/4095a77bb46602d87c4a9067313e209990c6f3d8]. (GitHub API reports only Apache-2.0.)
- Activity [gh api]: pushed 2026-10-08; 147 stars; 29 open issues; created 2021-03-29. Contributors: gadomski 1046 contributions, then dependabot 155, release-bot 17, bradh 4, jjcfrancisco 4, bitner 3 [gh api contributors]. 2026 commits since Jan 1 (first 100 pages): 41 commits by Pete Gadomski vs ~10 commits from 5 other humans (rest bots). => bus factor ~1 [INFER].
- Release cadence: release-please automated; `stac` v0.17.3 2026-07-31, .4 2026-08-06, .5 2026-08-27, .6 2026-09-04 [gh api releases]. All crates 0.x (no stability promise) [INFER from semver].
- Workspace crates (HEAD 4095a77): cli (published as `rustac`), core (`stac`), derive, duckdb, extensions, io, server, validate, wasm [SRC Cargo.toml].
- crates.io [curl crates.io API]: stac 0.17.6 (85k dl), stac-io 0.3.4 (6.0k), stac-duckdb 0.3.13 (8.2k), stac-extensions 0.1.16, stac-server 0.5.8, rustac (CLI) 0.2.16 (2.7k), stac-validate 0.6.14. **`stac-api` 0.8.2 last published 2025-11-14 and is NOT in the workspace anymore: the api module was moved into `stac` (crates/core/src/api) and the client into `stac-io`** [SRC crates/io/CHANGELOG.md#L117-L135 "move stac_api crate into stac crate (#869)", "move api client to stac-io crate (#864)"]. **No `stac-geoparquet` crate exists**; GeoParquet lives in `stac::geoparquet` behind the `geoparquet` feature (crates/core/Cargo.toml). Anyone depending on stac-api 0.8.x is on a frozen crate.
- Python: `rustac` on PyPI 0.9.17 (2026-08-18), repo stac-utils/rustac-py (Apache-2.0 per GH; Cargo says MIT OR Apache-2.0; 54 stars; 12 open issues; pushed 2026-10-06; 8 contributors) [gh api, PyPI JSON]. pyo3 bindings wrapping rustac + obstore-style stores; `rustac-duckdb-extensions` optional pkg bundles DuckDB extensions [SRC https://github.com/stac-utils/rustac-py/blob/ebcff1a611cb3fb32548a92f248222cce1e84e6a/python/rustac/duckdb.py].
- Other Rust STAC crates (crates.io search, tiny adoption): `stac-client` 0.3.0 (658 dl, Wayfinder-Foundry), `oxigdal-stac`/`oxigeo-stac` (cool-japan OxiGDAL, ~300-450 dl), `superstac-*` (spatialnode, federated search, 0.3.0, Oct 2026), `ogcapi-client` (georust). stac-incubator-rs (gadomski) last touched 2023. None is a competitor to stac-rs. (OxiGDAL as a whole is another agent's problem.)

### 1.2 STAC API client (stac-io)
- Search = POST /search with JSON body; follows `next` link (method/headers/body from the link) one page at a time [SRC https://github.com/stac-utils/rustac/blob/4095a77bb46602d87c4a9067313e209990c6f3d8/crates/io/src/api.rs#L419-L470 (stream, stream_pages), #L281 request_from_link].
- "Streaming": a spawned tokio task fetches pages and pushes them into an mpsc channel of capacity 4 (DEFAULT_CHANNEL_BUFFER = 4, api.rs#L23). Because each page's URL/token comes from the previous page's `next` link, requests are strictly serial: prefetch depth is up to 4 *completed* pages, but never >1 request in flight [SRC; INFER on the consequence]. No time-window splitting, no parallel pagination, no `numberMatched` pre-flight.
- Retries: not in the Rust client itself (it was removed: "remove our ApiClientBuilder, use reqwest's (#1072)" CHANGELOG). Python layer exposes reqwest retry knobs: max_retries_per_request, retry_status_codes, max_extra_load [SRC https://github.com/stac-utils/rustac-py/blob/ebcff1a611cb3fb32548a92f248222cce1e84e6a/src/api.rs#L26-L60].
- Python `rustac.search(...)` returns a full list; `iter_search` is the async-iterator form; `search_to(outfile, ...)` collects the whole ItemCollection in memory, converts to serde_json::Value, then writes JSON/NDJSON/GeoParquet [SRC rustac-py src/search.rs#L214-L305]. Not memory-flat for 10^6 items.
- `Search` supports: collections, ids, bbox, intersects, datetime, limit, sortby, fields, query, filter(CQL2 via cql2-rs) [SRC crates/core/src/api/search.rs, items.rs]. Aggregations not implemented (open issue #1121).
- Typed extensions crate `stac-extensions`: only authentication, electro_optical, projection (IDENTIFIER v2.0.0 only), raster (IDENTIFIER v1.1.0 only) [SRC crates/extensions/src/{projection,raster}.rs]. Earth Search v1 items are projection v1.1.0 (proj:epsg) and v2 items raster 2.0.0 (flattened). => do not rely on typed accessors; read `serde_json::Value` maps.

### 1.3 GeoParquet read/write
- Write: `stac::geoparquet` writer, default max_row_group_row_count 150,000 (DEFAULT_STAC_MAX_ROW_GROUP_ROW_COUNT) and, since PR #1107, writes GeoParquet bbox *covering* metadata pointing at the `bbox` struct column [SRC https://github.com/stac-utils/rustac/blob/4095a77bb46602d87c4a9067313e209990c6f3d8/crates/core/src/geoparquet.rs#L29 and #L411-L430; crates/core/CHANGELOG.md#L23]. 150k-row row groups are too coarse for tile-level pruning (TGE found ~6k rows/group needed, see 2.3).
- Read without DuckDB: `ItemCollection::from_geoparquet_path` reads the entire file into a Vec<u8> (`read_to_end`) then decodes; no projection/predicate/row-group pruning [SRC crates/io/src/geoparquet.rs#L9-L25]. Only suitable for small files.
- Partitioned stac-geoparquet writes: open issue #774 (unimplemented). Flexible schema #548 open.

### 1.4 stac-duckdb (DuckDB-backed search over stac-geoparquet)
- Library crate `stac-duckdb` 0.3.13; `Client::new()` runs `INSTALL spatial/icu/httpfs; LOAD ...` at construction (network fetch unless pre-installed) [SRC https://github.com/stac-utils/rustac/blob/4095a77bb46602d87c4a9067313e209990c6f3d8/crates/duckdb/src/client.rs#L79-L90]. duckdb crate = "1.10501.0" (DuckDB 1.5.x line) [SRC Cargo.toml]. Optional `bundled` feature compiles DuckDB (slow build) [crates/duckdb/Cargo.toml].
- Query building [SRC client.rs#L278-L440]: `SELECT <cols> FROM read_parquet('<href>', hive_partitioning=..., union_by_name=...) WHERE ...`.
  - `bbox` and `intersects` both become `ST_Intersects(geometry, ST_GeomFromGeoJSON(?))` (L359-L382). **No bbox-covering column predicate is generated.**
  - `datetime` -> `?::TIMESTAMPTZ <= coalesce(end_datetime, datetime)` / `?::TIMESTAMPTZ >= coalesce(start_datetime, datetime)` when start/end columns exist (L383-L407). [INFER] a `coalesce()` wrapper is not a column-vs-constant comparison, so DuckDB's Parquet zone-map pruning probably cannot use it; plain `datetime` comparison (Earth Search S2, no start/end) can. Not measured.
  - `filter` (CQL2) -> `cql2::ToDuckSQL` (L408-L415), only if all properties exist as columns, otherwise returns zero results (`Ok(None)`).
  - `query` extension -> hard error `QueryNotImplemented` (L281-L283, error.rs#L37).
  - Pagination = `LIMIT` / `OFFSET` (L424-L430); sortby = ORDER BY. `fields` = column projection. `DESCRIBE SELECT *` is executed first on the same href (L287, extra metadata read) [SRC].
  - Streaming: `search_to_arrow` returns an Arrow RecordBatch reader (true streaming from DuckDB); the async `StreamItemsClient` impl collects everything then streams (client.rs#L681-L700, comment "DuckDB queries run synchronously and return the full result in one go").
  - Iceberg source supported via `iceberg_scan` since 0.3.12.
- So: "search a local/remote stac-geoparquet file with spatial+temporal predicate pushdown": temporal yes (modulo coalesce), spatial *no* by stac-duckdb's SQL; see 2.2 for why plain ST_Intersects does not prune in DuckDB 1.5.x.

### 1.5 CQL2
- cql2-rs dependency (`cql2 = 0.6.0`), used for client-side filter handling and DuckDB SQL translation [SRC Cargo.toml]. Server-side support is per catalog (3.x).

### 1.6 Benchmarks
- No benchmark suite in rustac or rustac-py (`rg bench` finds only CHANGELOG text). All performance claims about rustac are absent, not just unverified [SRC].

## 2. stac-geoparquet spec + library + public datasets

### 2.1 Spec (moved to radiantearth/stac-geoparquet-spec; the stac-utils copy is a 3-line stub) [SRC https://github.com/stac-utils/stac-geoparquet/blob/a3825a3426b43eb420b9c435ba3b679bb3340eec/spec/stac-geoparquet-spec.md]
Spec @ ef0df4f (MIT, 23 stars, pushed 2026-09-29) https://github.com/radiantearth/stac-geoparquet-spec/blob/ef0df4f5e54b478f3e393a82e1c0aaadb329e986/stac-geoparquet-spec.md :
- GUARANTEES: one row per item; `bbox` is a REQUIRED struct (4 or 6 floats) conforming to GeoParquet 1.1 "Bounding Box Columns"; `geometry` WKB or GeoArrow; datetime columns are native timestamps; properties lifted to top-level columns; `assets` is a struct of per-asset structs (href required; extra fields like proj:*, raster:bands "may be added"); Collection JSON(s) in file metadata key `stac-geoparquet`, `version` 1.0.0/1.1.0; collection-mirror asset role for pointing a Collection at its parquet mirror.
- DOES NOT GUARANTEE: any sort order, any partitioning scheme, row-group size, or spatial index. It says only that the collection-mirror asset *description* "should ideally include details about the spatial partitioning method". So pruning efficiency is entirely up to the publisher.
- Python lib stac-geoparquet 0.8.2 (MIT, 2026-08-12; 155 stars; 21 open issues): conversion only (ndjson/items/pgstac/Delta Lake <-> Arrow/Parquet). No search, no sorting/partitioning helpers [SRC https://github.com/stac-utils/stac-geoparquet/blob/a3825a3426b43eb420b9c435ba3b679bb3340eec/CHANGELOG.md, docs/usage.md]. README claim of "much, much faster" for analytic queries is [CLAIM] with a single blog benchmark (2.4).

### 2.2 DuckDB / Parquet predicate pushdown on bbox
- DuckDB 1.5.x does NOT prune row groups from a plain `ST_Intersects(geometry, envelope)`; it shows the filter pushed into the scan but still reads every row group's geometry column [BENCH https://github.com/taylor-geospatial/s2-stac-geoparquet/blob/1c7dd4ee6c44e8a42317f7fc39bc9aae12437081/docs/query-performance.md "Geometry statistics are not used for pruning", DuckDB 1.5.3: 109/109 row groups read, 20.4 MB in 111 GETs]. Independent corroboration on DuckDB 1.5.6 (Iceberg path, not plain Parquet): plain ST_Intersects reads 10/10 files, `ST_Intersects_Extent`/`&&` reads 1/10 [BENCH https://github.com/jatorre/iceberg-geo-testbed/pull/1]. Dunnington (2025) states DuckDB needs explicit hard-coded `bbox.xmin <= ... AND ...` struct predicates to prune on a GeoParquet 1.1 covering column, while SedonaDB prunes automatically [SRC https://dewey.dunnington.ca/post/2025/lazy-geoparquet-reading-in-sedonadb-duckdb-geopandas-and-gdal/; no numbers given].
- Workaround: add `ST_Intersects_Extent(geometry, env)` or explicit bbox-struct predicates alongside ST_Intersects. stac-duckdb does not do this (1.4).
- Note TGE's mirror has `bbox` as list<double>, not the spec's struct, so covering-column stats pruning is unavailable there regardless (columns listed in https://data.source.coop/tge-labs/s2-stac-geoparquet/sentinel-2-c1-l2a/collection.json [MEAS]).

### 2.3 Public stac-geoparquet datasets for Sentinel-2 L2A
(a) **Microsoft Planetary Computer** `sentinel-2-l2a` `geoparquet-items` asset: `abfs://items/sentinel-2-l2a.parquet`, account `pcstacitems`, role stac-items, `msft:partition_info {is_partitioned: true, partition_frequency: "W-MON"}` [MEAS https://planetarycomputer.microsoft.com/api/stac/v1/collections/sentinel-2-l2a]. Docs: "produced with a lag relative to what's available through the STAC API"; recommend API for recent data [SRC https://github.com/microsoft/PlanetaryComputerExamples/blob/main/quickstarts/stac-geoparquet.ipynb ; web https://planetarycomputer.microsoft.com/docs/quickstarts/stac-geoparquet/].
  - **Not anonymous**: unauthenticated container list -> HTTP 409 PublicAccessNotPermitted [MEAS]; requires an SAS token (free anonymous token endpoint https://planetarycomputer.microsoft.com/api/sas/v1/token/pcstacitems/items works; I used it only to LIST blob names, no data downloaded, token deleted).
  - Listing at 2026-10-09 ~19:55 UTC [MEAS via SAS list]: 159 blobs, 136 distinct part numbers, ~68 GB total (61.7 GB if only newest file per part). **Partitions are MONTHLY by observed names** (`part-0133_2026-08-01..2026-08-31`, `part-0134_2026-09-01..`), contradicting the collection asset metadata "W-MON" and the docs ("partitioned by week"). Current-month parts are rewritten under a NEW filename each export (e.g. part-0134 appears 9 times with end timestamps 09-07 ... 09-30; 4 parts have >1 file), so a naive `*.parquet` glob double-counts items. Export cadence for the live months ~every 3-7 days (Last-Modified Sep 7, 14, 16, 17, 22, 24, 28, Oct 5). Newest data on Oct 9: items through 2026-10-03T15:41Z (part-0135, modified Oct 5) => ~6 days behind the API. Typical recent monthly part 0.3-0.56 GB; median part 449 MB; max 996 MB.
  - Assets carry hrefs (https blob URLs, need per-asset or container SAS signing); `assets` is a nested column (notebook uses json_normalize on it; exact Arrow type not inspected [INFER: struct]); items have proj:shape/transform/bbox per asset but no raster:bands (see 4.3).
(b) **Taylor Geospatial Engine "s2-stac-geoparquet" mirror** (Source Cooperative, public, no key): https://github.com/taylor-geospatial/s2-stac-geoparquet (Apache-2.0; created 2026-09-15; 45 stars; 15 open issues; pushed 2026-10-08), catalog https://data.source.coop/tge-labs/s2-stac-geoparquet/catalog.json, browse https://source.coop/tge-labs/s2-stac-geoparquet. (Also reachable as portolan-mirrors/sentinel-2-catalog; a GH issue says data prefix may still be portolan-mirrors during a migration - https://github.com/taylor-geospatial/s2-stac-geoparquet/issues/19 - but tge-labs collection.json served 200 for me.) [SRC README @1c7dd4e]
  - Two item indexes of Earth Search v1: `sentinel-2-c1-l2a` 30,652,620 rows (collection.json `table:row_count`, 2015-10-22..2026-10-09T01:52Z, updated 2026-10-09T04:40Z) and `sentinel-2-l2a` 51,603,974 rows (2016-11-01.., updated 2026-10-09T04:43Z) [MEAS collection.json]. Daily refresh 03:42 UTC + monthly consolidation [CLAIM README, corroborated by the `updated` stamps I read hours before my check].
  - Parts 0.1-2.8 GB each [CLAIM README]; C1: one `items.parquet`/year, ~6,000-row row groups, sorted (_tile, datetime); L2A original: year x UTM-zone parts, sorted (_month, _hilbert), 2026: (_month, s2:mgrs_tile, _hilbert). GeoParquet 2.0 native geometry => DuckDB >= 1.4 required.
  - Carries the complete asset hrefs, BUT `assets` is a JSON *string* column (not the spec's struct) and `bbox` is list<double> (not the spec's struct); so the file is not stac-geoparquet-spec conformant [MEAS collection.json table:columns vs spec]. Plus helper columns `_tile`, `_month`, `_hilbert`. Use `json_extract_string(assets,'$.red.href')` (README). proj:shape/transform live only inside the JSON blob.
  - Also publishes MGRS-tile x month aggregate stats (17-21 MB) with best_item_id per month - a ready-made cloud-free-month finder.
  - Measured latency in docs/query-performance.md (see 2.4).
(c) **Element84 Earth Search**: no official geoparquet export found (collection JSON has no table/geoparquet asset; Element84/earth-search repo/issues search found nothing). Official bulk alternative is the S3 Inventory parquet in `e84-earth-search-sentinel-data-inventory` (lists keys+sizes, not STAC items) [web search result of https://registry.opendata.aws/sentinel-2-l2a-cogs/ ; not verified by me]. Earth Search v1 states "best effort"; no SLA [SRC https://github.com/Element84/earth-search/blob/main/README.md#L176].
(d) Nothing found from Radiant/others for S2 L2A. geoparquet.org lists PC as the provider.

### 2.4 Measured latency: STAC API vs GeoParquet
- GeoParquet remote cold (TGE doc, DuckDB 1.5.3, Copenhagen laptop, Cloudflare-served bucket with TTFB 0.3-0.9 s per request): tile + 3 months + cloud<=20 LIMIT 30: median 10.8-15.1 s cold (min 3.9-8.4 s, max 76 s), 21-100 GETs, 1.4-7.8 MiB; warm same-connection 0.3-0.8 s with 0 GETs. Depth of sequential round trips (HEAD -> footer -> filter columns+bloom -> remaining columns) is 6-8, ~5-7 s regardless of layout; footer is 0.8-1.1 MB for 6k-row groups (about half the bytes of a small query) [BENCH https://github.com/taylor-geospatial/s2-stac-geoparquet/blob/1c7dd4ee6c44e8a42317f7fc39bc9aae12437081/docs/query-performance.md - includes full Python script; I did not rerun].
- STAC API, my single GETs (not a benchmark; different network): Earth Search v1 /search limit=1000, ~9-month AOI bbox 0.1 deg: 64 items, 1.19 MB, 1.0 s; same with `fields.include=[id,properties,geometry,bbox]` + `fields.exclude=[assets,links]` for 1 year: 153 items, 333 KB, 0.6 s; Earth Search v2 limit=1: 1.18 s; Planetary Computer limit=1000, same ~9-month AOI: 59 items, 897 KB, 2.9 s; CDSE limit=2: 143 KB, 2.5 s [MEAS].
- Bulk: Cloud Native Geo blog: 267,880 PC S2 items (Jan 2020) took ~162 s through the API with time-sliced asyncio.gather vs ~5 s to read the 260 MB local parquet (not apples-to-apples; machine unspecified) [CLAIM-with-code https://cloudnativegeo.org/blog/2024/08/introduction-to-stac-geoparquet/].
- [INFER] For the Tile-Ripper target query (one AOI, <=12 months, a few dozen to a few hundred items) one STAC API page (0.3-3 s) beats cold remote GeoParquet (5-15 s). GeoParquet wins for (i) >~10^4 candidate items (page caps), (ii) avoiding API throttling, (iii) repeated queries on a long-lived DuckDB connection (0.3-0.8 s warm), (iv) tile-level queries when the file is sorted by tile.

## 3. STAC API servers for Sentinel-2 L2A

| | Earth Search v1 (Element84) | Earth Search v2 (beta) | Planetary Computer | CDSE STAC | GEE |
|---|---|---|---|---|---|
| URL | https://earth-search.aws.element84.com/v1 | .../v2 | https://planetarycomputer.microsoft.com/api/stac/v1 | https://stac.dataspace.copernicus.eu/v1 | https://storage.googleapis.com/earthengine-stac/catalog/catalog.json |
| Server | stac-server (Node, Lambda) | same family, STAC 1.1.0 | stac-fastapi-pgstac fork (pcstac) | ? (eoAPI-like) | static JSON |
| S2 L2A collections | `sentinel-2-l2a` (~51.6M items, all baselines, JP2+COG), `sentinel-2-c1-l2a` (~30.6M, COG, baseline>=5.0), `-pre-c1-l2a`, `-l1c` | `sentinel-2-l2a` = old c1 (~26.2M), c1 id removed | `sentinel-2-l2a` | `sentinel-2-l2a` | dataset descriptions only |
| Page size | default 10; max deployment-configurable (ITEMS_MAX_LIMIT); live clamp = 10,000 (limit=100000 -> context.limit 10000) [MEAS]; **6 MB Lambda response cap** (issue below) | ? | default 250, **max 1000** (400 error, SRC search.py L35) | **max 200 for S2 L2A; >100 requires `fields`** (400 LimitValidationError, MEAS) | n/a |
| Extensions (conformsTo) | fields, sort, query, aggregation; **NO filter/CQL2** [MEAS] | + CQL2 filter, basic-spatial-functions, aggregation | query, sort, fields, **CQL2 filter (basic/json/text)** [MEAS] | filter (basic CQL2, S_INTERSECTS), query (eq,neq,lt..), fields, sort, collection-search [MEAS, docs] | none |
| Rate limits | undocumented; "best effort"; maintainers: "some throttling and concurrency limits in place" (Forbidden under ~300 burst searches) [CLAIM issue #19]; 502 on too-large responses (issue #17) | undocumented | STAC search: maintainer (2023) says no rate limit per se, 503/504 under load, retry w/ backoff [CLAIM discussion #246]; BUT source defaults per-IP: search 100/s, items 100/s, item/collection 500/s -> 429, back-pressure after 50 req/s (+10 ms per excess) [SRC pc-apis config.py L54-L66, redis.py L217-L250]; deployed values unknown. Asset SAS signing API is rate-limited (API key recommended); 429s reported on asset signing (issue #506) | quotas page: S3 4 concurrent connections, 20 MB/s/conn, 2,000 req/min (S3 only), 12 TB/month; direct HTTP COG 50k req/month [CLAIM docs Quotas.html]; STAC search no auth | n/a |
| Auth | none for search or COG GET (public bucket sentinel-cogs / e84-earth-search-sentinel-data) | same (items use s3:// href + storage:schemes) | search none; **assets need SAS** (planetary_computer.sign) | search none (my GET worked); **assets are s3://eodata JP2 inside .SAFE, need CDSE S3 keys / OIDC** | EE account |
| Asset format | COG (+JP2 requester-pays-ish in s3://sentinel-s2-l2a) | COG | COG on Azure blob (westeurope) | **JP2 only (SAFE)**, not COG | n/a |
| Tile props | `grid:code` ("MGRS-17SPV"), `mgrs:utm_zone/latitude_band/grid_square`; NO `s2:mgrs_tile` on items I fetched from 2021 and 2026 [MEAS], though TGE README says original index has it | `grid:code`, `mgrs:*`, `proj:code` | `s2:mgrs_tile` (queryable) [MEAS queryables]; no grid:code | `grid:code` | - |
| Queryables | only `eo:cloud_cover` listed [MEAS] | ? | ~35 incl. eo:cloud_cover, s2:mgrs_tile, s2:nodata_pixel_percentage, s2:processing_baseline, proj:epsg | per-collection | - |
Observed GETs: ES v1 limit=1 items GET 0.48 s; PC root 1.35 s; CDSE root 0.78 s [MEAS].
- Earth Search `fields`: `include` alone does NOT prune assets/links (same 18.7 KB/item); `exclude:[assets,links]` combined with an include list works (2.2 KB/item) [MEAS].
- Earth Search duplicates: pre-2022 original index mostly has two items per scene/tile (`_0_`/`_1_` ids) [CLAIM TGE README]; I saw 153 items for 1 year on one AOI [MEAS].
- Google Earth Engine "STAC": static collection-level catalog only (dataset JSON with no `assets`, no items/search link) [MEAS COPERNICUS_S2_SR_HARMONIZED.json]. Not a discovery source.
- pgstac (PC's backend) docs say narrow datetime + simple geometry + single collection are fastest [CLAIM discussion #246]. stac-fastapi-pgstac 7.1.0 (2026-10-06), pgstac v0.10.0 (2026-10-01), titiler-pgstac 3.2.0 (2026-09-16) are all MIT and active; titiler-pgstac is a tile server (not an acquisition engine).

## 4. Other discovery-layer projects
- **pystac-client** 0.9.0 (2025-07-18; PyPI Apache-2.0; LICENSE file Apache 2.0 [SRC]; GH API says NOASSERTION): sequential `pages()/items()` generators, urllib3 Retry max_retries=5 default; no parallel paging; no client-side default limit since 0.7.4. Main branch has unreleased changes since July 2025 (transaction endpoints etc.) [SRC https://github.com/stac-utils/pystac-client/blob/f76c1e952b67fe0d5daaca570fa73ee8430be318/CHANGELOG.md; pystac_client/stac_api_io.py#L51].
- **odc-stac** 0.5.3 (2026-07-30, Apache-2.0, 204 stars, pushed 2026-10-07): builds the lazy datacube from `proj:shape` + `proj:transform` + `proj:code/epsg` (+ `raster:bands`/`bands` for dtype/nodata) with NO file opening when the Item has the projection extension and every data asset has shape+transform; otherwise probes files [SRC https://github.com/opendatacube/odc-stac/blob/d3345ed21efddb7aa1962990b734009602334d3f/odc/stac/_mdtools.py#L165-L310 (has_proj_ext, has_proj_data, asset_geobox)]. Depends on pystac.
- **stackstac** 0.5.1: last release/push 2024-08-10, MIT; effectively unmaintained [PyPI/GH]; uses proj:* when present, rasterio for IO.
- **rasteret** (exists): https://github.com/terrafloww/rasteret v0.4.1 (2026-07-22/26), Apache-2.0, 137 stars, 1 open issue, 3 contributors, pushed 2026-08-02. "Index-first reader": `rasteret.build("earthsearch/sentinel-2-l2a", bbox, date_range)` searches STAC, parses each COG header once, stores per-band struct `{image_width, image_height, tile_width, tile_height, dtype, transform, predictor, compression, tile_offsets[], tile_byte_counts[], pixel_scale, tiepoint, nodata, ...}` in a Parquet/Arrow collection; later reads go straight to pixel byte ranges (asyncio.gather) [SRC https://github.com/terrafloww/rasteret/blob/c91eb1b789f2c7bbd2934b559bd3b5697158b4c8/src/rasteret/constants.py#L112-L135 ; README.md "Why Rasteret"]. Header parser issues 16 B read -> 2 B IFD count -> IFD entries -> separate range reads for external tag arrays (>=4 sequential round trips per band per scene) [SRC src/rasteret/fetch/header_parser.py#L321-L360, #L636]. Benchmarks (single AOI 15 scenes: 9.08 s rasterio vs 1.14 s; multi-AOI 30 scenes 42.05 vs 2.25 s; cross-CRS 12 scenes 12.47 vs 0.59 s; us-west-2 EC2) [CLAIM; notebook docs/tutorials/05_... exists; docs/explanation/benchmark.md states the Rasteret "index/header" time is "time to read the pre-built collection index from local storage", i.e. build cost excluded]. Docs concede the index is a snapshot of metadata at build time (docs/explanation/design-decisions.md#L142). Python 3.12+, so a Python-dependency not a Rust building block, but the *schema* is exactly the discovery->acquisition hand-off artifact.
- **eodag** 4.9.2 (2026-10-07, Apache-2.0, 437 stars): Python multi-provider search+download gateway; not performance oriented; includes CDSE/PC/Earth Search providers [GH/PyPI; not read in depth].
- stac-asset (Python, auth+download), stac-task, stactools: not relevant. stac-incubator-rs: dead (2023).

### 4.3 What the S2 L2A items actually carry (decide whether HEAD / IFD reads are avoidable)
Per-asset fields observed [MEAS; raw JSON in scratchpad/data/*.json]:
- Earth Search v1 `sentinel-2-l2a` COG asset (e.g. `red` B04.tif): href (https://sentinel-cogs.s3.us-west-2.amazonaws.com/...), type "image/tiff; application=geotiff; profile=cloud-optimized", `proj:shape` [10980,10980], `proj:transform` [10,0,600000,0,-10,4000020], `gsd`, `raster:bands[{nodata:0, data_type:uint16, spatial_resolution:10, scale:0.0001, offset:-0.1}]`, `eo:bands`. Item-level `proj:epsg`, `proj:centroid`, `earthsearch:s3_path`, `earthsearch:boa_offset_applied: true`, `s2:processing_baseline`. **No `file:size`, no `proj:bbox`, no block size, no compression.** Also `-jp2` twin assets under s3://sentinel-s2-l2a (bucket access terms not verified by me).
- Earth Search v1 `sentinel-2-c1-l2a`: same + `file:size` (e.g. 202,674,140 B for B04) and `file:checksum` (multihash) + `storage:*` [MEAS scratchpad/stac/es_c1_item.json, sibling file].
- Earth Search v2: STAC 1.1: flattened `data_type`, `nodata`, `raster:scale`, `raster:offset`, `raster:spatial_resolution`, `proj:code`, `file:size`, `bands`/`eo:common_name`, `storage:refs`, `s3://e84-earth-search-sentinel-data/...` hrefs [MEAS].
- Planetary Computer: `proj:bbox`, `proj:shape`, `proj:transform`, `eo:bands`, `gsd`; **no raster:bands (no scale/offset/nodata), no file:size** [MEAS]. Scale/offset must come from `s2:processing_baseline` knowledge (baseline >= 04.00 has offset -1000 DN) [INFER].
- CDSE: JP2 (not COG); `proj:code`, `proj:bbox/shape/transform`, `data_type`, `nodata`, `raster:scale/offset`, `file:size`, `file:checksum`, `alternate.https` (OData, OIDC) [MEAS].
- STAC raster ext v2.0.0 fields: bits_per_sample, sampling, spatial_resolution, scale, offset, histogram, unit, nodata, data_type - **no tile/block/overview/compression fields** [SRC https://github.com/stac-extensions/raster (README)]; projection ext latest v2.1.0 (2026-09-29). I found no STAC extension or proposal for COG tile layout.

**Can a planner compute exact tile byte ranges from STAC alone? No.** `proj:shape` + `proj:transform` + AOI => exact pixel window (row/col rectangle) in each asset grid, per band resolution. Mapping the window to tile *indices* additionally needs the block size (1024 or 512; absent from STAC; but extremely regular per asset key + resolution, see below), and mapping indices to bytes needs TileOffsets/TileByteCounts (data-dependent because of deflate; cannot be predicted). Only ONE ranged GET per file is needed to get all of it for these COGs:
- From a 64 KiB prefix of Earth Search v1 B04 (sibling agent's file scratchpad/stac/hdr/es_B04.hdr; parsed by my own script scratchpad/s02/ifdscan.py, `python3 -I`): classic TIFF, IFD0 10980x10980 tile **1024x1024**, deflate (comp 8) + horizontal predictor (2), 121 tiles; overviews IFD1..IFD4 5490/2745/1373/687 with **512x512** tiles; every IFD and every TileOffsets/TileByteCounts array (484 B each at 121 tiles) lies within the first ~3.6 KB. SCL (20 m): IFD0 512 tiles, overviews 256 tiles. Another NC tile (17SPV) identical. [SRC-derived, provenance of the prefix file = sibling agent, URL assumed from file name]. => 1 round trip (>=4 KB; async-tiff reads ahead 32 KiB by default: https://github.com/developmentseed/async-tiff/blob/cf4bc5943a4d25f56e80c3be8e2827e828bb96b7/src/metadata/cache.rs#L115 [SRC]) per file, fully parallel across files; vs. rasteret's parser >=4 sequential reads per file; vs GDAL, which fetches a configurable first chunk (GDAL_INGESTED_BYTES_AT_OPEN, "bytes read in one GET call at file opening") then more on demand.
- Cost scale: 12 bands x 100 items = 1,200 header GETs x ~1 RTT; at 50-100 ms TTFB and 64-way concurrency ~1-2 s; at TGE-style 0.8 s TTFB it is ~15 s. Caching these in a local Parquet/sidecar removes it for repeat reads, but the cache is keyed on (href, ETag/size) since processing may overwrite objects.
- Practical consequence: tile size 1024 at 10 m means each 2 MB-raw tile (~1.7 MB compressed: 202 MB / 121) is the minimum fetch; AOI smaller than 10 km on a side suffers ~read amplification of 1024^2 / AOI-pixels. (A COG-layer issue, noted here because the plan hand-off must carry tile size.)

## 5. MGRS pruning
- Earth Search: `grid:code` = "MGRS-<utm><band><square>" (e.g. MGRS-17SPV) + `mgrs:utm_zone/latitude_band/grid_square` [MEAS]; item ids contain the tile (`S2C_17SPV_20260909_0_L2A`; C1: `S2C_T17SPV_20260830T160924_L2A`).
- Planetary Computer: `s2:mgrs_tile` (queryable).
- CDSE: `grid:code`.
- S2 tiles are 109.8 km squares with 4.9 km overlap, native UTM grid (proj:transform origin multiples of 10 m: 600000 / 4000020 for 17SPV) => AOI -> tile list is deterministic (no STAC call needed) and tile+date identifies items; any pixel in a 4.9 km overlap strip exists in 2 (or 4 in corners) tiles; the planner must choose/dedupe (not the discovery layer's job but the hand-off must carry tile id and nodata-pixel %).
- Parquet pruning: sort/partition by tile (TGE C1: one tile-year = 1-2 row groups; L2A 2026: one tile-month = 1 row group + 2 boundary groups). Hive `year=` partitioning gives file skipping; DuckDB cannot glob over plain HTTPS ("Globs (*) for generic HTTP file are not supported") - need s3:// listing or explicit part names [SRC TGE README].
- Static-href alternative [INFER]: Earth Search COG keys are deterministic `sentinel-s2-l2a-cogs/{utm}/{band}/{sq}/{yyyy}/{m}/{id}/B04.tif` with id = {platform}_{tile}_{date}_{seq}_L2A; platform letter (A/B/C) and sequence digit are unknown a priori, so one S3 LIST per tile-month could replace a STAC call if cloud/nodata filtering is not needed.

## 6. Adopt / gaps / hand-off

### Adopt unchanged
- `stac` crate types (Item, ItemCollection, api::Search) and `stac-io` api client for plain "search + follow next" - but treat items as `serde_json::Value` maps because typed extensions are version-pinned. License fine (MIT/Apache).
- STAC API search endpoints as the default discovery source (ES v1/v2, PC, CDSE all answer in 0.3-3 s for the target query). Use `fields` (exclude assets/links) for candidate listing then fetch assets only for chosen items.
- stac-geoparquet *spec* as the on-disk index format for any cached/exported item set (but write it with a sorted/partitioned layout and the bbox struct + covering metadata; TGE's JSON-string `assets` + list bbox shows publishers deviate).
- TGE mirror (Source Coop, public) as an optional bulk/offline discovery source for S2 L2A; PC geoparquet only if an SAS flow is already needed.
- DuckDB (directly, own SQL with `ST_Intersects_Extent` or bbox-struct predicates) rather than stac-duckdb for parquet-side search if that path is used.

### Gaps (nothing existing fills them)
1. Block size / compression / overviews / tile offsets+byte counts not in any catalog; rasteret is the only prior art (Python, header-chatty build).
2. Concurrent paging / time-window fan-out in Rust (stac-io is serial); handling PC 1000 cap, CDSE 200 cap, ES 6 MB lambda cap, 429/502/403 backoff across catalogs.
3. Catalog normalisation: tile id (grid:code vs s2:mgrs_tile), CRS (proj:epsg vs proj:code), band metadata (raster:bands vs flattened vs absent), scale/offset by processing baseline, hrefs (https/s3/abfs; signed vs unsigned; PC SAS container tokens; CDSE S3 keys), JP2 vs COG, `_0_/_1_` duplicates, baseline changes, STAC 1.0 vs 1.1 (Earth Search v2 beta reshapes everything).
4. stac-duckdb: no covering-column pruning, `query` unsupported, coalesce datetime predicates, extension INSTALL at construct time, no true async streaming.
5. Freshness reconciliation between mirrors (PC ~6 days lag; TGE daily) and API.

### What the discovery -> acquisition plan hand-off should contain
Per scene (item): id, collection, catalog + search params + timestamp (provenance), datetime, tile id (normalised), EPSG/CRS, footprint geometry + bbox, eo:cloud_cover and nodata %, processing baseline (for offset), STAC version/extension versions.
Per (scene, band/asset): normalised href + scheme + auth ref (none | SAS container | S3 keys), media type/driver (COG vs JP2), file size (from `file:size` when present, else from the header GET's Content-Range), ETag/version for cache validity, `proj:shape`, `proj:transform`, dtype, nodata, scale/offset (resolved), then (from header cache or first-read) block size, compression/predictor/planar config, overview list (shape + tile size per IFD), TileOffsets/TileByteCounts (or pointer to a cached sidecar), header length actually needed.
Per AOI x asset: precomputed pixel window (row/col range) in the asset grid and, once headers are known, the list of (tile idx, offset, length) ranges; selection/dedupe groups (same-date adjacent tiles, overlap strips).
Output envelope: Arrow/Parquet table (stac-geoparquet-compatible columns + `{band}_layout` struct à la rasteret) so the acquisition stage and ChronoZarr writer consume the same artifact.

## 7. Contradictions found
- PC docs/asset metadata say weekly partitions (W-MON) but the actual blob names are monthly parts, with multiple re-exported files per live month.
- PC maintainers (2023) say STAC search has no rate limiting; pc-apis source ships per-IP 100 req/s search limits (429) and back-pressure.
- TGE README says original Earth Search index includes `s2:mgrs_tile`; live Earth Search v1 items from 2021 and 2026 that I fetched carry `grid:code` and no `s2:mgrs_tile`.
- PC asset metadata advertises `application/x-parquet`, spec says `application/vnd.apache.parquet`.
- rustac README/docs advertise stac-api as a crate; it is frozen at 0.8.2 and its content now lives in `stac` + `stac-io`.
- GitHub license field for rustac (Apache-2.0) vs Cargo/LICENSE files (MIT OR Apache-2.0); pystac-client GH NOASSERTION vs Apache-2.0 LICENSE.
- Rasteret "20x faster than rasterio/GDAL" headline vs its own benchmark page: header time for Rasteret = reading a pre-built local index (build cost excluded).
- stac-geoparquet spec requires `bbox` struct + `assets` struct; the largest public S2 mirror (TGE) uses list<double> and a JSON string.

## 8. Open questions
- PC geoparquet row-group size, sort order, and whether `datetime`/`s2:mgrs_tile` stats prune (needs a footer read; not done).
- Earth Search v2 production date and whether v1 `sentinel-2-l2a` will be frozen.
- Real PC STAC per-IP limits as deployed; Earth Search throttle thresholds (undocumented).
- Whether DuckDB >= 1.5.x spatial will add an ST_Intersects -> bbox rewrite (no issue exists per iceberg-geo-testbed PR).
- CDSE STAC max page for fields-restricted queries (>100 allowed, upper bound unknown).
- Per-band block sizes across baselines (only one 17SPV 2026 sample parsed; sibling agent has others).
- AOI-driven MGRS enumeration: an existing Rust crate? (not investigated.)

## 9. URL index
- https://github.com/stac-utils/rustac (HEAD 4095a77bb46602d87c4a9067313e209990c6f3d8)
- https://github.com/stac-utils/rustac-py (HEAD ebcff1a611cb3fb32548a92f248222cce1e84e6a)
- https://crates.io/crates/stac , /stac-io , /stac-duckdb , /stac-api , /rustac
- https://pypi.org/project/rustac/ , /pystac-client/ , /stac-geoparquet/ , /odc-stac/ , /rasteret/ , /eodag/
- https://github.com/stac-utils/stac-geoparquet (a3825a3426b43eb420b9c435ba3b679bb3340eec)
- https://github.com/radiantearth/stac-geoparquet-spec (ef0df4f5e54b478f3e393a82e1c0aaadb329e986)
- https://github.com/stac-utils/pystac-client (f76c1e952b67fe0d5daaca570fa73ee8430be318)
- https://github.com/microsoft/planetary-computer-apis (3644b7bc3e239fa6adf3cb2c2152b2f84beb0a40)
- https://github.com/stac-utils/stac-server (8015fdf1aad722bc477e2dfbdc4360411dcfd7c7)
- https://github.com/taylor-geospatial/s2-stac-geoparquet (1c7dd4ee6c44e8a42317f7fc39bc9aae12437081)
- https://github.com/terrafloww/rasteret (c91eb1b789f2c7bbd2934b559bd3b5697158b4c8)
- https://github.com/opendatacube/odc-stac (d3345ed21efddb7aa1962990b734009602334d3f)
- https://github.com/developmentseed/async-tiff (cf4bc5943a4d25f56e80c3be8e2827e828bb96b7)
- https://github.com/jatorre/iceberg-geo-testbed/pull/1
- https://dewey.dunnington.ca/post/2025/lazy-geoparquet-reading-in-sedonadb-duckdb-geopandas-and-gdal/
- https://cloudnativegeo.org/blog/2024/08/introduction-to-stac-geoparquet/
- https://planetarycomputer.microsoft.com/docs/quickstarts/stac-geoparquet/
- https://github.com/microsoft/PlanetaryComputerExamples/blob/main/quickstarts/stac-geoparquet.ipynb
- https://github.com/microsoft/PlanetaryComputer/discussions/246 ; /issues/506
- https://github.com/Element84/earth-search (README, docs/collections/sentinel-2-l2a.md) ; issues #17, #19
- https://earth-search.aws.element84.com/v1 , /v2 ; https://stac.dataspace.copernicus.eu/v1 ; https://documentation.dataspace.copernicus.eu/APIs/STAC.html ; https://documentation.dataspace.copernicus.eu/Quotas.html
- https://data.source.coop/tge-labs/s2-stac-geoparquet/catalog.json ; https://source.coop/tge-labs/s2-stac-geoparquet
- https://github.com/stac-extensions/raster ; https://github.com/stac-extensions/projection
- https://registry.opendata.aws/sentinel-2-l2a-cogs/
