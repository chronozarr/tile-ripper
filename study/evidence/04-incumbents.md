# 04 - Incumbents: GDAL / odc-stac / stackstac / others (research notes)

Date of research: 2026-10-09. Agent 4 of 8. READ-ONLY (cloned upstream into scratchpad/repos, no execution of tools; the only network probe I ran was `curl -I --http2` against public hosts to see which HTTP version they negotiate).

Evidence grades: [SRC] I read code/docs myself; [BENCH] reproducible benchmark (code+data+method public); [BENCH-partial] code/data public but config/versions missing; [CLAIM] authors' assertion, not reproducible; [INFER] my inference.

Pinned commits I read (all permalinks use these):
- GDAL master   5154115825852ee99091b939886d3f24bddb10bd (2026-10-09, = 3.14.0dev; 3.14.0 due 2026-11-08 per GitHub milestone). Also tags v3.12.0 / v3.12.4 (raw files) because owner has 3.12.4 and rasterio 1.5.2 wheels bundle GDAL 3.12.2 + curl 8.20.0 [SRC https://github.com/rasterio/rasterio/blob/main/CHANGES.txt].
- odc-stac  d3345ed21efddb7aa1962990b734009602334d3f (v0.5.3, 2026-07-29)
- odc-loader 38395b17e1647e9b86cb3257a576a2f8aaee1d01 (0.6.4 released 2026-01-14; commits to 2026-07-30)
- stackstac 38571909cd83d1f3da0c20e44450560e174ff845 (v0.5.1, 2024-08-10 = last commit)
- rioxarray c48a1a38c326d08727a448eace3a90e908801988; rio-tiler a45f2937f8ea90b185e8b5444b9ecc12ae24fe08 (9.4.12, 2026-10-08); Xee 0d7c6a3007cfe941f6bfc559cf6a45dce5758755 (v0.1.2)

Latest GDAL releases: 3.13.3 (2026-08-18), 3.13.0 (2026-05-08), 3.12.4 (2026-04-24).

====================================================================
## 1. GDAL: /vsicurl/, /vsis3/, /vsiaz/ and the COG read path
====================================================================

### 1.1 How a COG window read actually happens (code-level, [SRC])
GTiff `IRasterIO` (https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/frmts/gtiff/gtiffdataset.cpp#L470-L545) picks one of three paths:

A. `CacheMultiRange` path (default for /vsicurl/ & friends when GDAL_NUM_THREADS is NOT set): builds the list of compressed block ranges the window needs, sorts/merges contiguous or slightly-overlapping ranges, issues ONE `VSIFReadMultiRangeL` -> curl multi, ranges fetched in parallel (one HTTP request per merged range; HTTP/2 multiplex or multiple connections). Decode is then single-threaded in the calling thread. HARD CAP: `GDAL_MAX_RAW_BLOCK_CACHE_SIZE` default 10 MiB (10485760) of compressed data per RasterIO call; beyond that it stops adding blocks and the rest are fetched block-by-block (sequential round trips). [SRC gtiffdataset_read.cpp#L7129 https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/frmts/gtiff/gtiffdataset_read.cpp#L7120-L7135]. This option is NOT in configoptions.rst (I grepped). Independent evidence it matters: sentle PR #93 raised it to 256 MB; 4392 px window went 31 requests/56 MB -> 10 requests/36 MB [CLAIM-with-counting-proxy https://github.com/cmosig/sentle/pull/93].
B. `MultiThreadedRead` path (only if `NUM_THREADS` open option or `GDAL_NUM_THREADS` config is set; GDAL >= 3.6; window must intersect >1 block, no resampling in RasterIO): computes all block offsets, calls `VSIVirtualHandle::AdviseRead` (implemented for /vsicurl/ since 3.7) which fetches ALL ranges asynchronously in a background thread, up to `CPL_VSIL_CURL_ADVISE_READ_TOTAL_BYTES_LIMIT` = 100 MiB (default; if exceeded the RasterIO request is split in halves), while a worker pool decodes blocks as ranges land. Decode threads come from ONE process-global pool `GDALGetGlobalThreadPool` that only grows [SRC https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/gcore/gdal_thread_pool.cpp#L28-L50]. Docs: https://gdal.org/en/stable/drivers/raster/gtiff.html (oo NUM_THREADS: "Starting with GDAL 3.6 ... multi-threaded decoding").
C. Block-by-block (GDAL block cache, `GDAL_CACHEMAX`): fallback; each block = its own HTTP range (via vsicurl 16 KiB-granular regions that grow up to 128x on sequential access).

Other warp-level behaviour: `GDALWarpOperation` calls `AdviseRead` on the source only if the chunk list covers >=80% of the bounding source window [SRC alg/gdalwarpoperation.cpp#L1029-L1037]. For GTiff sources `AdviseRead` at dataset level is a no-op (no `IAdviseRead` in frmts/gtiff - rg returned nothing); real prefetch happens inside RasterIO (A/B above). `gdalwarp -multi` = 2 threads (I/O thread + compute thread); real parallel compute needs `-wo NUM_THREADS`; `-wm` warp memory (default 64 MB) is shared among threads [SRC https://gdal.org/en/stable/programs/gdalwarp.html].

There is NO `ReadBlocks`/`GDALDataset::ReadBlocks` API in GDAL (rg over gcore/*.h found nothing). The closest things: `GDALDataset::AdviseRead`, `VSIVirtualHandle::AdviseRead` (3.7), `GetAdviseReadTotalBytesLimit` (3.9), RFC 90 compressed-data access (3.7), RFC 101 thread-safe datasets (3.10).

### 1.2 Connection / concurrency architecture (this is the structural part) [SRC]
GDAL 3.12.x (owner's 3.12.4; rasterio 1.5.2 wheels 3.12.2):
- One libcurl multi handle PER THREAD PER FILESYSTEM HANDLER (`static thread_local g_tls_connectionCache`) used by ordinary reads and `ReadMultiRange`: https://github.com/OSGeo/gdal/blob/v3.12.4/port/cpl_vsil_curl.cpp#L3931-L3937 ; `GetCurlMultiHandleFor` at #L3993. => N threads = N separate connection pools; no connection sharing across threads.
- `AdviseRead` creates a DEDICATED multi handle per `VSICurlHandle` (i.e. per opened dataset) and joins its thread on next call/close: #L3517-L3540 and ~VSICurlHandle #L505-L525. => fresh TCP+TLS per opened file whenever the AdviseRead (GDAL_NUM_THREADS) path is used. [INFER on cost; mechanism is SRC]
- Code comment in GDAL itself: "For HTTP/1.1, parallel connections work better since you can get results out of order" (#L2609, #L3529).
- Global region (LRU) cache, file-prop cache and dir-list cache sit behind one `CPLMutexHolder oHolder(&hMutex)` per filesystem handler (#L4028+). `AddRegion` copies bytes into a std::string. => global lock on every cache hit/miss for all threads [SRC; contention impact not measured by anyone I found].
- Docs: GDAL multithreading page: datasets are never thread-safe; block-cache is fine for concurrent READS; heavy multithreading fragments glibc malloc -> use tcmalloc LD_PRELOAD [SRC https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/doc/source/user/multithreading.rst]. RFC 101 (3.10): `GDAL_OF_THREAD_SAFE` just opens one dataset per thread behind a wrapper (generic impl.), "there is a band block cache per thread, no sharing"; Python GIL becomes the bottleneck when many short GDAL calls are made [SRC https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/doc/source/development/rfc/rfc101_raster_dataset_threadsafety.rst]. LIBERTIFF (3.11) is a natively thread-safe read-only GeoTIFF driver that bypasses the block cache (must be requested explicitly: `-if LIBERTIFF` / allowed_drivers) [SRC .../drivers/raster/libertiff.rst]. rasterio's concurrency doc now shows a LIBERTIFF thread_safe=True example for GDAL>=3.11 [SRC https://rasterio.readthedocs.io/en/latest/topics/concurrency.html].

GDAL master (future 3.14, due 2026-11-08): PR #15007 (merged 2026-08-16, "Change execution behavior of curl requests", Andrew Bell) moves ALL curl work onto a dedicated per-filesystem "Run" thread with ONE shared multi handle (`m_runMutex`, `m_multi`, `Perform()`), explicitly "removed support for the CURLPIPE_MULTIPLEX option as it is the default" [SRC https://github.com/OSGeo/gdal/pull/15007 ; code https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/port/cpl_vsil_curl.cpp#L1095-L1130]. Backport to 3.13.4 marked "failed backport". Consequence: from 3.14, connections/HTTP2 streams are shared across threads (good); but a single run thread drives all transfers [INFER: a possible new serialization point; nobody has benchmarked it]. Nothing in 3.12.x/3.13.x has this.

### 1.3 Config options that matter (default per CODE vs per DOCS)
Docs = https://gdal.org/en/stable/user/configoptions.html (source: configoptions.rst, virtual_file_systems.rst at master SHA above). Code = port/cpl_vsil_curl.cpp (master) / v3.12.4.

| Option | Default (code) | What it does / notes |
|---|---|---|
| CPL_VSIL_CURL_CHUNK_SIZE | 16384 (min 1024, max 10 MiB) | granularity of range GETs; grows up to 128x (=2 MiB) on detected sequential reads. Docs say raise CACHE_SIZE to 128x CHUNK_SIZE if you raise CHUNK_SIZE |
| CPL_VSIL_CURL_CACHE_SIZE | 16,384,000 B (=1000 regions x 16 KiB) | GLOBAL LRU of downloaded regions shared by all handles; "regions" = cache_size/chunk_size, so raising CHUNK_SIZE without CACHE_SIZE shrinks region count (thrash with many files) |
| GDAL_INGESTED_BYTES_AT_OPEN | 1024 (clamped 1024..10 MiB) then rounded up to chunk size => effectively 16 KiB | first GET at open; raise for COGs with big headers (TiTiler example header 32,770 B). Docs give no default |
| GDAL_DISABLE_READDIR_ON_OPEN | FALSE | EMPTY_DIR = no dir listing AND drivers don't probe sidecars (.aux.xml/.msk/.ovr). This is THE big open-latency fix. |
| CPL_VSIL_CURL_ALLOWED_EXTENSIONS | unset | pretends other files don't exist (saves probes) |
| CPL_VSIL_CURL_USE_HEAD | YES | HEAD at open; can set NO to use ranged GET (also `use_head=no` in `/vsicurl?` syntax) |
| GDAL_HTTP_MULTIPLEX | YES (3.12.x reads it; REMOVED from code in master after PR #15007 though docs still list it) | only affects HTTP/2 connections. See 1.4: useless on S3/Azure Blob/CDSE |
| GDAL_HTTP_VERSION | not set => libcurl default (docs say "default 1.1 generally"; GCE VMs auto 2TLS) | 2 / 2TLS / 2PRIOR_KNOWLEDGE to force HTTP/2 [SRC cpl_http.cpp#L2151-L2210] |
| GDAL_HTTP_MULTIRANGE | PARALLEL (code metadata) vs "YES" (docs) | anything except SINGLE_GET/SERIAL is parallel; SINGLE_GET (one GET with multiple ranges) unsupported by S3/GCS |
| GDAL_HTTP_MERGE_CONSECUTIVE_RANGES | YES | merges adjacent ranges in ReadMultiRange/AdviseRead (AdviseRead also merges across gaps <= 8 bytes COG markers) |
| GDAL_HTTP_ENABLE_ADVISE_READ | TRUE | master switch for async AdviseRead prefetch (undocumented in configoptions.rst) |
| CPL_VSIL_CURL_ADVISE_READ_TOTAL_BYTES_LIMIT | 100 MiB | max bytes AdviseRead will fetch at once (undocumented in rst) |
| GDAL_MAX_RAW_BLOCK_CACHE_SIZE | 10 MiB | cap for CacheMultiRange prefetch per RasterIO (undocumented in rst) |
| VSI_CACHE / VSI_CACHE_SIZE | NO / 25 MB per file handle | per-handle RAM cache; discarded at close; no cross-open benefit (the 16 MB global cache already does that) |
| GDAL_CACHEMAX | 5% RAM | raster block cache (decoded blocks); useful when same block is touched repeatedly (chunk edges/overlap) |
| GDAL_BAND_BLOCK_CACHE | AUTO | HASHSET good for huge sparse rasters |
| GDAL_NUM_THREADS | unset | enables GTiff multithreaded decode + AdviseRead path (3.6+); ALL_CPUS or N; shared global pool |
| GDAL_HTTP_MAX_RETRY / RETRY_DELAY / RETRY_CODES | 0 / 30 s / 429,500,502,503,504 | retry: AdviseRead retry added 3.12; ReadMultiRange retry + range sorting added 3.13 (NEWS: https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/NEWS.md lines ~470-471) => on 3.12.x the CacheMultiRange path has NO retry on S3 503 SlowDown |
| GDAL_HTTP_MAX_CACHED_CONNECTIONS / GDAL_HTTP_MAX_TOTAL_CONNECTIONS | unset | added 3.11; applied to each multi handle => per-thread limit in 3.12 [INFER] |
| GDAL_HTTP_TCP_KEEPALIVE | NO | 3.6+ |
| CPL_VSIL_CURL_NON_CACHED | unset | defeats global cache for named paths (benchmarking!) |
| CPL_VSIL_NETWORK_STATS_ENABLED + VSINetworkStatsGetAsSerializedJSON | off | built-in request/byte counters per file/action -> use to measure requests & bytes for any benchmark [SRC cpl_vsil_curl.cpp#L6447, cpl_vsi.h#L528] |
| GDAL_HTTP_HEADERS / pc_url_signing=yes (vsicurl key) / VSICURL_PC_URL_SIGNING | - | Planetary Computer SAS signing built into /vsicurl (3.5.2+) |

Other things the docs state that matter: /vsis3 AWS_NO_SIGN_REQUEST, AWS_REGION (set region to avoid redirect penalty, per opendatacube/benchmark-rio-s3 [CLAIM]), CPL_VSIL_CURL_USE_S3_REDIRECT.

### 1.4 HTTP version actually negotiated by the S2 hosts (my own probe, 2026-10-09, `curl --http2 -I`; curl 8.7.1 w/ nghttp2) [SRC-probe, reproducible]
- sentinel-cogs.s3.us-west-2.amazonaws.com (AWS Open Data S2 COGs) -> HTTP/1.1
- e84-earth-search-sentinel-data.s3.us-west-2.amazonaws.com (Earth Search c1-l2a) -> HTTP/1.1
- sentinel2l2a01.blob.core.windows.net (Planetary Computer S2 L2A) -> HTTP/1.1
- eodata.dataspace.copernicus.eu (CDSE S3) -> HTTP/1.1
- storage.googleapis.com -> HTTP/2 ; planetarycomputer.microsoft.com (API) -> HTTP/2
So GDAL_HTTP_MULTIPLEX/VERSION=2 (recommended by TiTiler) have no effect for the main S2 archives; parallel ranges = parallel TCP connections. Corroborated by a user's `CPL_CURL_VERBOSE` trace showing "ALPN, server accepted to use http/1.1" for sentinel-cogs S3 [https://github.com/gjoseph92/stackstac/issues/199] and by TiTiler's own caveat.

### 1.5 GDAL features that already cover parts of "(AOI, time, bands) -> pixels" [SRC]
- STACIT driver (3.4): STAC ItemCollection JSON -> virtual mosaic (VRT), MAX_ITEMS default 1000 [https://gdal.org/en/stable/drivers/raster/stacit.html].
- GTI driver (3.9): tile-index mosaic; STAC-GeoParquet index since 3.10; `gdal raster index --profile STAC-GeoParquet` since 3.13 [gti.rst].
- `gdal raster pipeline/mosaic/stack/reproject/tile` (3.11+, `-j/--num-threads`) [https://gdal.org/en/stable/programs/gdal_raster_pipeline.html].
- Zarr driver: V2+V3, V3 sharding read/write (3.13), multithreaded chunk prefetch with GDAL_NUM_THREADS (3.13), default V3 from 3.14 [https://gdal.org/en/stable/drivers/raster/zarr.html]. Owner's 3.12.4 has NO sharding.
- COG driver, overviews auto-selected for decimated reads.

### 1.6 Known GDAL limitations (collected)
- Dataset not thread-safe (per-thread open or RFC 101 wrapper). Block cache per dataset instance; global per-process, not shared across processes (dask process workers re-fetch).
- No cross-file/cross-band request scheduler: concurrency is whatever threads the caller brings; prefetch is per RasterIO call (A/B above).
- Per-thread curl pools (3.12), per-handle AdviseRead pools; one mutex around caches.
- Undocumented-but-critical caps (10 MiB raw-block, 100 MiB AdviseRead).
- Python-level GIL when many short calls (RFC 101).
- Stability: GDAL issue #14997 "multithreaded Warp of tiled LERC BigTIFF fails since GDAL 3.13" (closed); #13528 intermittent cert error in GTI warps (open); #8041 JP2OpenJPEG multi-threaded RasterIO floods server with duplicate requests (closed).
- S2 JP2 (CDSE): TLM markers missing in existing products; ESA erratum plans TLM; reported 6x latency decrease for remote crops with baseline 05.12 [CLAIM, Pangeo thread https://discourse.pangeo.io/t/cloud-optimized-access-to-sentinel-2-jpeg2000/5072].
- Provider limits: CDSE general users: 4 concurrent connections, 20 MB/s each, 12 TB/30d, 2000 S3 req/min [SRC https://documentation.dataspace.copernicus.eu/Quotas.html]. => on CDSE the quota, not the engine, is the ceiling (80 MB/s max aggregate).

====================================================================
## 2. odc-stac (+ odc-loader + odc-geo)
====================================================================
- License Apache-2.0 (all three). Status: ACTIVE. odc-stac v0.5.3 2026-07-29, last push 2026-10-07; odc-loader 0.6.4 2026-01-14 (commits to 2026-07-30), 6 open issues; odc-geo push 2026-10-07. Sustainability caveat: maintainers say orgs funding odc-loader/odc-stac have no Zarr requirement and "progress is therefore likely to be slow" on eopf-zarr support [SRC https://github.com/opendatacube/odc-stac/issues/241 ; Kirill888: "prepared to finance this development?"]. odc-stac depends on dask[array], rasterio, pystac<2, odc-loader>=0.6.4 [pyproject].
- Reading: `odc.loader._rio` -> `rasterio.open(src.uri, "r", sharing=False)` per (source, chunk) read, then `rdr.read(window=...)` if paste-compatible, else `rasterio.warp.reproject(...)`; overview chosen via `pick_overview` when `use_overviews` (default True) [SRC https://github.com/opendatacube/odc-loader/blob/38395b17e1647e9b86cb3257a576a2f8aaee1d01/src/odc/loader/_rio.py#L601-L634]. The class comment says "TODO: open file handle cache goes here" (#L95) => every read re-opens the dataset. Reads are wrapped in `rio_env(VSI_CACHE=False)` (#L622, #L630).
- Plan: `odc.stac.load()` parses ALL items in Python (`list(items)`, `parse_items`), groups (`groupby=time|solar_day|id|prop`), builds output GeoBox (`crs, resolution, anchor, bbox/geobox/like`), bins each (time group, item) to chunk-grid tiles using the item's footprint geometry (`_tyx_bins`, `item.safe_geometry`) [SRC https://github.com/opendatacube/odc-stac/blob/d3345ed21efddb7aa1962990b734009602334d3f/odc/stac/_stac_load.py#L395-L525, #L615-L640]. Dask graph: one task per (band, t, y, x) chunk loading all contributing sources for that chunk then fusing "first valid pixel wins" (`_dask_loader_tyx`, `_dask_fuser`: np.full + copyto) [odc-loader _builder.py#L454-L520]. Empty chunks are cheap (no read).
- Without `chunks=`: `direct_chunked_load` with `pool=None` => SERIAL `map` (no threads!) unless `pool=int|ThreadPoolExecutor` [SRC odc-loader _utils.py#L35-L44; _builder.py#L880-L960]; default internal chunk 2048 (DEFAULT_CHUNK_FOR_LOAD). `progress=` only in this path. Docstring does not state the serial default (doc-vs-code gap).
- Concurrency config: `odc.stac.configure_rio(cloud_defaults=True, verbose=, aws=...)`, `configure_s3_access(aws_unsigned=..., requester_pays=..., cloud_defaults=True, **gdal_opts)` [SRC _rio.py#L296-L380]; applied on dask workers via env capture (`capture_rio_env`). If the user never configures, `capture_rio_env` returns GDAL_CLOUD_DEFAULTS = {GDAL_DISABLE_READDIR_ON_OPEN: EMPTY_DIR, GDAL_HTTP_MAX_RETRY: 10, GDAL_HTTP_RETRY_DELAY: 0.5} (#L82-L86, #L636-L651). Nothing else is set: no HTTP/2, no cache sizes, no GDAL_NUM_THREADS, no GDAL_MAX_RAW_BLOCK_CACHE_SIZE.
- Threads: determined entirely by Dask (scheduler/threads_per_worker) or `pool=`. No docs give tuning guidance; docs/stac-best-practice.rst is about metadata (proj:shape, raster ext), not performance. Only benchmarking doc: https://odc-stac.readthedocs.io/en/latest/benchmarking.html (`python -m odc.stac.bench`), whose sample output shows 16-19 Mpx/s overall (4 threads, 3 bands, 9 items, 80 m, 2022-01) = illustrative only [CLAIM].
- Options asked about: `resampling` (per band; `nearest` default; with `use_overviews` + non-nearest triggers warp), `fail_on_error` (default True; False => warn and return empty slice), `stac_cfg` (per-collection missing-metadata config), `anchor` (edge|center; wrong choice => half-pixel shift [Pangeo thread]), `progress` (non-dask only), `patch_url` (e.g. planetary_computer.sign), `fuse_func`.
- Zarr: odc-loader has a `_zarr.py` reader driver for in-memory xarray/zarr spec docs; eopf-zarr STAC (CDSE sample service) NOT supported natively (issue #241 closed without implementation; workaround = GDAL `ZARR:"..."` href).
- Cloud-optimised I/O limits: (a) no handle/IFD cache across chunk tasks (above); (b) VSI_CACHE forced off at read; (c) planning via footprint polygons (S2 footprints include nodata slivers => empty reads); (d) pure-Python planning cost: lazycogs notebook: odc-stac 10.44 s to load items + 45.47 s "initialize array" for 122 time steps x 3 bands, 13000x7100 px @100 m, 512 chunks (single laptop run) [CLAIM]; odc-stac vs stackstac report: graph submit 3.88 s (512 chunks) -> 0.27 s (2048) [BENCH-partial].

====================================================================
## 3. stackstac
====================================================================
- License MIT. Status: effectively UNMAINTAINED. Last release v0.5.1 2024-08-10; last commit same day; owner on 2026-03-25: "I am not actively maintaining stackstac, and have no plans to currently ... odc-stac is certainly under more active development" [SRC https://github.com/gjoseph92/stackstac/issues/270]; PR #271 adding a README pointer to odc-stac is open [https://github.com/gjoseph92/stackstac/pull/271]; 60 open issues / 12 open PRs. README disclaimer: "I haven't even written tests yet! Don't use this in production" [README].
- Architecture [SRC https://github.com/gjoseph92/stackstac/blob/38571909cd83d1f3da0c20e44450560e174ff845/stackstac/rio_reader.py]: Dask blockwise graph built lazily in constant time (to_dask.py, ArraySliceDep); each (item, asset) = one `AutoParallelRioReader`; on first read it opens the file once (`sharing=False`) then hands out `ThreadLocalRioDataset` (a separate dataset per thread, because GDAL datasets aren't thread-safe) when driver is GTiff, else `SingleThreadedRioDataset` with a lock (MULTITHREADED_DRIVER_ALLOWLIST={"GTiff"}); makes a `WarpedVRT` whenever the asset's grid != requested grid. One band per asset only ("Assets must have exactly 1 band"). No overview support (issue #196 open, 13 comments), float64 + scale/offset by default (`rescale=True`, `dtype=float64`), default `chunksize=1024`, `xy_coords='topleft'`.
- GDAL env (`DEFAULT_GDAL_ENV`, LayeredEnv, thread-local rio.Env copies): always: GDAL_HTTP_MULTIRANGE=YES ("unclear if this actually works"), GDAL_HTTP_MERGE_CONSECUTIVE_RANGES=YES; open: GDAL_DISABLE_READDIR_ON_OPEN=EMPTY_DIR, VSI_CACHE=True (so threads re-opening the same URL hit cache); read: VSI_CACHE=False [#L36-L55]. No retry settings by default (issue #18 open since 2021; #61 "occasional lockups during dask reads" - hangs with no timeout; Gabe suggested GDAL_HTTP_TIMEOUT/MAX_RETRY via LayeredEnv).
- Known pitfalls: wide mosaics create many no-op chunks => huge graph (33.4 s graph submit at chunk 512 vs 0.9 s odc) [BENCH-partial]; "stackstac.mosaic" memory (PC discussion #17); default dtype/rescale -> 4x memory; edge-vs-center coords; duplicated STAC items from PC `sentinel-2-l2a`.
- Gabe Joseph writeups: I searched Pangeo Discourse, GitHub, Coiled and general web; the substantive written record of his GDAL/Dask tuning is the in-code comments above, the stackstac README ("Mediate between Dask's parallelism and GDAL's aversion to it") and his 2022-04-20 reply in odc-stac #54 ("Manages the GDAL cache so multithreaded reading is as fast as possible; constructs the dask graph as efficiently as possible") https://github.com/opendatacube/odc-stac/issues/54 . I did NOT find a standalone benchmark writeup or a published GIL/thread-count/process-vs-thread study by him [absence-of-evidence, searched ~6 queries]. The "Coiled/Dask GIL" material found (Dask fine-performance-metrics, dask-pyspy demo) is generic.
- Cluster doc example: 8 workers x 4 threads on Coiled us-west-2; 292 S2 scenes @100 m, 9,344 chunks; stack() 295 ms; "run in the same data center" [https://stackstac.readthedocs.io/en/latest/examples/cluster.html] - no throughput given.

====================================================================
## 4. Other incumbents (one paragraph each)
====================================================================
- rioxarray (Apache-2.0, active, last push 2026-07-27): per-file `open_rasterio`; default `lock=None -> RASTERIO_LOCK` (global SerializableLock => one read at a time per process) unless `lock=False` (then a file handle per thread) [SRC https://github.com/corteva/rioxarray/blob/c48a1a38c326d08727a448eace3a90e908801988/rioxarray/_io.py#L1053-L1064,#L1124-L1127]. Not a STAC planner; no AOI/time -> pixels. Good for local/one-file work; for fast multi-COG reads you'd pass lock=False yourself.
- rio-tiler (BSD-3, very active) / TiTiler (MIT): tile/window readers: `rasterio.open` per request, `WarpedVRT` for reprojection, mosaic via ThreadPoolExecutor with `RIO_TILER_MAX_THREADS` default cpu_count*5 [SRC rio_tiler/constants.py#L11-L12, tasks.py]. Built for ~256-px web tiles at request latency, not bulk cubes. NEW: rio-tiler has an experimental async reader on async-geotiff (`rio_tiler/experimental/geotiff.py`) and zarr readers. TiTiler's performance-tuning page is the best vendor-published GDAL env block (see 6) but gives no measured numbers [https://developmentseed.org/titiler/advanced/performance_tuning/].
- cogeo-mosaic (MIT, 9.2.0 2026-06-18): MosaicJSON = precomputed tile->asset index, read via rio-tiler; solves "which COGs cover this tile" fast, not bulk acquisition/time-stacks.
- xarray-spatial (MIT, active): numba/dask raster ANALYSIS (slope, zonal, NDVI...), not acquisition. Irrelevant to the I/O question.
- gdalcubes (R + C++; MIT; v0.7.5 2026-09-15): GDAL-based on-the-fly cubes from image collections (STAC via rstac), `gdalcubes_options(parallel=N)` multiprocess; GDAL warp per chunk; no published throughput numbers (tutorial notes slow outside AWS region). Same GDAL ceiling as Python stacks; real prior art for the "lazy cube + chunked reproject" design (2019-).
- openEO (client Apache-2.0): API standard; execution is back-end specific. CDSE back-end: free tier 10,000 credits/month, synchronous request flat 7 credits, 2 concurrent sync + 2 concurrent batch jobs, "tested up to 100x100 km at 10 m" [https://documentation.dataspace.copernicus.eu/APIs/openEO/openEO.html ; credit_usage page]. Solves (AOI,time,bands)->pixels as a service, but lock-in + quotas + queueing; not an engine you control.
- Xee (Apache-2.0, v0.1.2 2026-07-14; v0.1.0 breaking refactor): xarray backend over Earth Engine `computePixels`; request cap 48 MB (`REQUEST_BYTE_LIMIT`, xee/ext.py#L75-L77), ThreadPoolExecutor via `executor_kwargs`. EE project quotas (Community tier default): 40 concurrent requests, 100 req/s, 10 MB request payload [https://developers.google.com/earth-engine/guides/usage, updated 2026-07-27]. Solves it fast IF the data is in EE (you pay with quotas/EECU, ToS, no raw-bytes control, output grid resampled by EE); lock-in is total.
- Sentinel Hub Process API (now Planet Insights Platform; docs moving to docs.planet.com): AOI+time+bands via evalscript, server-side mosaicking/resampling; sync output <= 2500x2500 px, async <= 10,000 px, batch needs enterprise, PU-metered with per-minute request/PU limits (trial 10 req/min) [CLAIM: community forum posts, older; official page I fetched states none of these numbers]. Pure SaaS lock-in.
- GEE `getPixels/computePixels` raw: same limits as Xee (above).
- lazycogs (MIT, created 2026-04-13, 55 stars, Development Seed) / async-geotiff / async-tiff / Rasteret (Terrafloww): the Rust/async-native competitors; evidence in section 5. lazycogs = stac-geoparquet (rustac/DuckDB) + async-geotiff + obstore + pyproj warping, "first valid pixel" mosaic with early stop, nearest-only resampling. Directly overlaps the Tile-Ripper idea (without a Zarr writer / without an engine-level scheduler).

====================================================================
## 5. Best-available speed evidence for tuned incumbents on S2 L2A
====================================================================
HEADLINE: no rigorous, independently reproduced, fully-specified public benchmark of a tuned odc-stac/stackstac/GDAL pipeline on Sentinel-2 L2A exists that I could find. The best are partial. Details:

1. odc-stac vs stackstac report (odc-stac maintainer "Kirill888", 2022, per odc-stac issue #54; https://benchmark-odc-stac-vs-stackstac.netlify.app/) [BENCH-partial]
   - Planetary Computer notebook env, single node, 4 cores / 32 GiB, S2 L2A COGs from Azure blob, 3 bands (red,green,blue) 10 m. Best of 3-5 runs reported. Raw CSV public; tool `odc.stac.bench` public. NOT stated: GDAL env, thread count, versions, Azure/compute region.
   - Deep (13 slices of one tile, 10,980x10,980, 8.76 GiB decoded): odc-stac 105.0 Mpix/s, stackstac 121.8 Mpix/s (both best at chunk 2048).
   - Wide (9 tiles mosaic, 1 day, 10,980x90,978, 5.58 GiB): odc-stac 74.7 Mpix/s (40.1 s elapsed), stackstac 30.1 Mpix/s at chunk 7168 (18.1 Mpix/s, 165.9 s at chunk 2048). Report attributes odc-stac wide-vs-deep loss to 8% pixel overlap + 22% fusing cost.
   - Derived by me: 105 Mpix/s x 2 B = ~210 MB/s of DECODED uint16 on 4 cores (~26 Mpix/s/core ~ 52 MB/s/core). Compressed bytes/s unknown (not reported).
2. opendatacube/benchmark-rio-s3 (opendatacube org, repo archived 2026-02-03; https://github.com/opendatacube/benchmark-rio-s3/blob/master/report.md) [BENCH-partial; Landsat-8, old GDAL]: m5.xlarge us-west-2, 1,278 Landsat-8 COG tiles (512x512, DEFLATE, median 404 KB), GDAL env EMPTY_DIR + ALLOWED_EXTENSIONS=tif + VSI_CACHE=TRUE; threads 1->49: 13.7 -> 371.7 tiles/s (derived ~5.5 -> ~150 MB/s); at 1 thread median open 30.9 ms = 59.8% of per-tile time; per-thread efficiency drops 13.7 -> 7.6 tiles/s; authors: latency-dominated, "blocking library design prevents issuing many opens and reads from one thread ... async would be ideal". Not S2, no HTTP/2, no GDAL_NUM_THREADS.
3. pyasyncio-benchmark (repo owner geospatial-jeff, cited by Development Seed; https://github.com/geospatial-jeff/pyasyncio-benchmark, results 2025-04-23 m5.4xlarge, 300 s runs) [BENCH, headers only]: "cog_header" = open one S2 COG (sentinel-cogs S3) with rasterio (GDAL_INGESTED_BYTES_AT_OPEN=16384, EMPTY_DIR, NO_SIGN, CPL_VSIL_CURL_NON_CACHED so cache defeated, run via anyio threads -> likely 40-thread default limiter [INFER]): rasterio 285 opens/s; async-tiff 6,989 header parses/s (24.5x); raw obstore 16 KB GETs 11,087/s; aiohttp 7,727/s. Measures ONLY open/header, not pixel decode or end-to-end.
4. Pangeo thread (2024-03/04) https://discourse.pangeo.io/t/comparing-odc-stac-load-and-stackstac-for-raster-composite-workflow/4097 [CLAIM]: rabernat: odc 68 s vs stackstac 66 s (monthly median, 0.5x0.5 deg, 52 duplicated scenes from `sentinel-2-l2a`, AWS us-east-1 VM = cross-region, no GDAL config); scottyhq: with `odc.stac.configure_rio(cloud_defaults=True, aws={"aws_unsigned": True})` ~3.75 s wall (6.17 s CPU) for the same job on 26 `sentinel-2-c1-l2a` scenes (104 GeoTIFFs) on "a modest machine in us-west-2" - claims "order of magnitude" but baseline not like-for-like.
5. lazycogs vs odc-stac (https://developmentseed.org/lazycogs/performance/ + notebook .../notebooks/lazycogs-odc-stac/) [CLAIM; single run, laptop, unknown network]: odc-stac 0.5.2 vs lazycogs: init array 45.47 s vs 0.77 s; point extract (30 d) 17.13 s vs 4.96 s; spatial subset (3 time steps) 28.81 s vs 13.55 s; load items 10.44 s (odc only). Setup: Earth Search c1-l2a, 2025-06..09, 13000x7100 px @100 m EPSG:5070, 122 steps, odc `chunks={"time":1,"x":512,"y":512}` use_overviews=True nearest; lazycogs time-chunked only (not identical). No `configure_rio` or dask config in the notebook (odc gets the GDAL_CLOUD_DEFAULTS automatically). Reproducibility: notebook is public but results depend on laptop+network; no variance. => Not a tuned-baseline comparison; mostly measures planning cost + small-chunk overhead at 100 m.
6. sentle PR #93 (https://github.com/cmosig/sentle/pull/93) [BENCH-partial; request-counting proxy script in repo]: on PC, 30 km chunk wall 58-100 s -> 22-23 s after (a) one STAC search/run, (b) reading each band once per tile+chunk, (c) GDAL_MAX_RAW_BLOCK_CACHE_SIZE=256 MB, (d) 4 concurrent band-window reads/worker. Requests/MB e.g. 568/278 -> 240/199. Shows how much app-level read planning + one undocumented GDAL cap moves the needle (not a GDAL-vs-X comparison).
7. Element 84 (vendor [CLAIM]): JP2K needs ~2x data and ~25x more GETs for partial reads; COG tile read >=3x faster than JP2K https://element84.com/cloud-architecture/aws/sentinel-2-cloud-optimized-geotiffs-now-available-on-aws-registry-of-open-data/
8. Cloud-Native Geo blog [CLAIM]: STAC-GeoParquet vs API: 267,880 Jan-2020 S2 items, ~160 s (parallelised API) vs <5 s (parquet, 260 MB) https://cloudnativegeo.org/blog/2024/08/introduction-to-stac-geoparquet/
9. Rasteret (Terrafloww blog) [CLAIM, unverifiable]: no numbers in text (results are images), baseline = plain sequential rasterio with no env; no repo linked. Ignore as evidence.
10. rasterio docs: 1 vs 4 threads 4.28 s -> 1.25 s on a local windowed-read example (illustrative) https://rasterio.readthedocs.io/en/latest/topics/concurrency.html
11. Cloud-optimized geospatial formats guide (https://guide.cloudnativegeo.org/cloud-optimized-geotiffs/intro.html): qualitative only (block 256/512, overviews); no benchmarks.
12. DevSeed warp-resample-profiling (https://developmentseed.org/warp-resample-profiling/pages/ecosystem.html): reprojection tools on MUR SST/IMERG, not S2 I/O; says I/O choice (virtualisation, overviews) beats algorithm tuning "by factors of 2-20" [CLAIM]; explicitly excludes async-tiff/lazycogs pipelines; GDAL-backed rioxarray/odc-geo lead on speed+memory.

Provider ceilings: CDSE general 4 conn x 20 MB/s (above); EE 40 concurrent; AWS S3/PC blob: no published per-user caps found (S2 on PC does rate-limit STAC + SAS endpoints per sentle PR #93: "hitting Planetary Computer's rate limit").

====================================================================
## 6. Fair "tuned baseline" configurations
====================================================================
Protocol (must-haves, [INFER] from the above): compute in the data's region (PC: West Europe; Earth Search c1-l2a: us-west-2); report cold + warm cache; N>=5 runs, report median+min; record GDAL/rasterio/curl versions (rasterio 1.5.2 wheel = GDAL 3.12.2 / curl 8.20; also try conda GDAL 3.13.3 for ReadMultiRange retry); count requests/bytes with CPL_VSIL_NETWORK_STATS_ENABLED + VSINetworkStatsGetAsSerializedJSON or a counting proxy (sentle approach); sweep rather than pick: dask threads {4,8,16,32,64}, chunk {1024,2048,4096} (aligned to COG block size UNVERIFIED for E84 c1-l2a; PC = 512 per sentle PR claim only), GDAL_NUM_THREADS {unset, 4, ALL_CPUS}; pin same output grid/dtype/resampling; measure time-to-first-chunk AND total; include STAC search+plan time (stac-geoparquet vs API) as a separate line.

A. odc-stac / GDAL baseline
```python
import odc.stac, rasterio
odc.stac.configure_rio(
    cloud_defaults=True,                       # EMPTY_DIR, MAX_RETRY=10, RETRY_DELAY=0.5   [odc-loader _rio.py L82-86]
    aws={"aws_unsigned": True, "region_name": "us-west-2"},   # Earth Search; set region to avoid redirects [benchmark-rio-s3 CLAIM]
    CPL_VSIL_CURL_ALLOWED_EXTENSIONS=".tif",   # [TiTiler, GDAL docs]
    GDAL_INGESTED_BYTES_AT_OPEN=32768,         # [TiTiler: raise on high latency; example header 32,770 B]
    CPL_VSIL_CURL_CACHE_SIZE=200_000_000,      # [TiTiler; GDAL default 16 MB thrashes with many files]
    GDAL_CACHEMAX=1024,                        # MB; TiTiler uses 200
    GDAL_BAND_BLOCK_CACHE="HASHSET",           # [TiTiler]
    GDAL_MAX_RAW_BLOCK_CACHE_SIZE=268_435_456, # [GDAL code default 10 MiB; sentle PR #93 uses 256 MB]
    GDAL_NUM_THREADS="ALL_CPUS",               # [GDAL>=3.6 MT decode + AdviseRead path] - sweep it, global pool shared w/ dask threads
    GDAL_HTTP_MERGE_CONSECUTIVE_RANGES="YES",  # already default
    GDAL_HTTP_TCP_KEEPALIVE="YES",             # [3.6+; INFER]
)
ds = odc.stac.load(items, bands=[...], bbox=..., crs=..., resolution=10, groupby="solar_day",
                   chunks={"x": 2048, "y": 2048}, resampling="nearest", fail_on_error=False)
# dask: threaded scheduler with many threads (I/O bound: benchmark-rio-s3 peaked at 40-50 threads on a 4-vCPU box);
# e.g. LocalCluster(n_workers=1, threads_per_worker=16..64, processes=False) -> sweep.
```
Caveats: (1) odc-loader forces VSI_CACHE=False inside reads (so TiTiler's VSI_CACHE=TRUE advice is overridden for data reads; header caching relies on the global CPL_VSIL_CURL_CACHE_SIZE). (2) HTTP/2 vars (GDAL_HTTP_VERSION=2, GDAL_HTTP_MULTIPLEX=YES) only help on GCS/CloudFront - harmless but useless on S3/Azure/CDSE. (3) Non-dask path: pass `pool=N` or it is serial. (4) `chunks=2048` was best for both libs in the only public S2 sweep (benchmark-rio-s3 / odc report), but only on a 4-core node.
Optional upgrade: GDAL 3.13.x (retries for ReadMultiRange); GDAL master 3.14 (shared curl multi) once released 2026-11.

B. stackstac baseline
```python
import stackstac, rasterio as rio
env = stackstac.DEFAULT_GDAL_ENV.updated(
    always=dict(GDAL_HTTP_MAX_RETRY=10, GDAL_HTTP_RETRY_DELAY=0.5, GDAL_HTTP_TIMEOUT=45,       # [stackstac issue #61 advice]
                CPL_VSIL_CURL_ALLOWED_EXTENSIONS=".tif", CPL_VSIL_CURL_CACHE_SIZE=200_000_000,
                GDAL_CACHEMAX=1024, GDAL_MAX_RAW_BLOCK_CACHE_SIZE=268_435_456,
                AWS_NO_SIGN_REQUEST="YES", AWS_REGION="us-west-2"),
    open=dict(GDAL_INGESTED_BYTES_AT_OPEN=32768),
    read=dict(GDAL_NUM_THREADS="ALL_CPUS"))
da = stackstac.stack(items, assets=[...], epsg=..., resolution=10, bounds=...,
                     chunksize=(1, 1, 2048, 2048), dtype="uint16", fill_value=0, rescale=False,  # avoid float64 + scale (4x memory)
                     resampling=rio.enums.Resampling.nearest, xy_coords="center", gdal_env=env,
                     errors_as_nodata=(rio.errors.RasterioIOError("HTTP response code: 404"),))
```
Notes: defaults it ships: GDAL_HTTP_MULTIRANGE=YES (no-op = default), MERGE_CONSECUTIVE_RANGES=YES, EMPTY_DIR + VSI_CACHE=True on open only, VSI_CACHE=False on read. No overview reads (#196) => for decimated output stackstac reads full-res (the 2022 report shows 4x+ slower at 20-80 m); upstream is unmaintained so this will not be fixed. For Dask threads same sweep as A. Use stackstac only as a *historical* baseline; odc-stac is the live incumbent.

====================================================================
## 7. Structural inefficiencies a purpose-built engine could remove (vs what GDAL already does well)
====================================================================
REAL inefficiencies (ranked by evidence):
1. Per-read open + header parse, no cross-task IFD/tile-offset cache. [SRC] odc-loader opens a fresh dataset per (source,chunk) with a TODO to cache; stackstac re-opens per thread. [BENCH-partial] benchmark-rio-s3: open = ~60% of per-tile time for ~400 KB tiles at 1 thread; pyasyncio-benchmark: 285 rasterio opens/s vs ~7,000 async-tiff header parses/s on a 16-vCPU box (cache deliberately defeated). A planner that precomputes (url, offset, length, codec, dtype, predictor) per tile (stac-geoparquet + sidecar index; Rasteret/lazycogs idea) removes most header GETs entirely. Caveat: for large 2048-px chunk reads of S2 (several MB each) open share is small; gain is biggest for small windows / many chunks / many scenes.
2. No global I/O scheduler; concurrency = OS threads. [SRC] GDAL 3.12: per-thread curl pools, per-handle AdviseRead pools, global-lock caches; hosts are HTTP/1.1 so each in-flight range = a TCP+TLS connection [probe]. [BENCH-partial] needs 40-50 threads to saturate a 4-vCPU instance on 400 KB reads; per-thread efficiency halves. An async engine with a shared connection pool (and thousands of in-flight range GETs across files/bands/time) removes thread cost and handshakes - but NOT the connection count itself on HTTP/1.1, and not provider quotas (CDSE: 4 conns).
3. Python planning/graph cost scaling with items x chunks. [BENCH-partial/CLAIM] 10.4 s item load + 45.5 s array init (odc-stac, 122 steps x 3 bands at 512 chunks) vs 0.77 s lazycogs; stackstac 33 s graph at 512 chunks. [SRC] odc bins by footprint polygon in Python. GDAL's own GTI (STAC-GeoParquet) would also fix this - i.e. it's removable without a new engine for the plan stage.
4. Decode -> intermediate numpy -> fuse -> dask -> (later) rechunk -> Zarr: extra copies and no end-to-end pipeline coupling to the sink. [SRC] `_dask_loader_tyx` allocs chunk, `_dask_fuser` np.full + copyto; [BENCH-partial] report attributes 22% (of 30%) wide-vs-deep loss to fusing. A direct "decode into output shard buffer -> compress -> write shard" pipeline (ChronoZarr) removes copies and Dask scheduling; this is where a sink-aware engine differs from every incumbent (none of odc-stac/stackstac writes Zarr itself). [INFER on magnitude].
5. Read amplification / redundant reads from chunking + undocumented caps: GDAL's 10 MiB raw-block cap, 16 KiB region granularity/16 MB cache, block cache not shared across processes/threads-with-RFC101, per-chunk windows straddling blocks, nodata-sliver footprints, duplicate STAC items. [CLAIM-counting proxy] sentle: 568 req/278 MB -> 240 req/199 MB for the same output by reading each band once per tile. An engine planning at COG-block granularity across the whole AOI (dedup, valid-footprint pruning, QA-first skipping) avoids these. Mostly achievable by better tuning/planning in Python too -> weaker "novelty" claim.

NOT inefficiencies (GDAL does it well - don't re-implement without a measured reason):
- Range coalescing & parallel range fetch within a read (ReadMultiRange / CacheMultiRange / AdviseRead; merges adjacent and COG-marker-gapped ranges) [SRC].
- Multithreaded block decode in-process (GTiff >= 3.6, global pool, zstd/deflate/lzw/lerc/jpeg/webp/jxl) [SRC]; rasterio releases the GIL around RasterIO [rasterio docs].
- Overview selection / decimated reads (odc `use_overviews`; GDAL RasterIO picks overviews) [SRC].
- COG leader/trailer tricks to avoid reading TileOffsets/ByteCounts (OptimizedRetrievalOfOffsetSize) [SRC gtiffdataset_read.cpp].
- Retry/auth/signing: S3 SigV4, Azure SAS, GCS, Planetary Computer URL signing built in; 429/5xx retries (improving in 3.12/3.13) [SRC].
- Warp/resampling kernels + PROJ transforms, many formats (JP2 TLM via OpenJPEG, NetCDF, Zarr v2/v3 incl. sharding in 3.13) [SRC]. Reproducing quality parity is expensive; DevSeed's own profiling says GDAL-backed tools lead on speed+memory for resampling [CLAIM].
- Raw bandwidth: once threads/connections are enough, S2 reads are bandwidth/decode bound on any engine; HTTP/2 won't help (hosts are 1.1).
- GDAL already ships STAC (STACIT), stac-geoparquet mosaics (GTI), multi-step pipelines (`gdal raster pipeline`) and Zarr v3 writing - a Tile-Ripper must beat `GTI + gdal raster reproject/tile` too, not just Python tools.

====================================================================
## 8. Contradictions / doc-vs-code gaps found
====================================================================
1. configoptions.rst lists GDAL_HTTP_MULTIPLEX "Defaults to YES"; on GDAL master the option is only declared in the option XML (cpl_vsil_curl.cpp#L4667) and no longer read (PR #15007 removed CURLPIPE_MULTIPLEX); in 3.12.4 it is read (v3.12.4 cpl_vsil_curl.cpp#L2611, #L3531). GitHub code search confirms only rst + that XML mention it on master.
2. GDAL_HTTP_MULTIRANGE: docs say choices SINGLE_GET/SERIAL/YES default YES; option metadata in code says PARALLEL/SERIAL default PARALLEL; code just tests for SINGLE_GET/SERIAL. stackstac sets "YES" with comment "unclear if this actually works" (it's the default).
3. GDAL_HTTP_VERSION docs: "default 1.1 generally"; code leaves libcurl default unless set (or GCE). TiTiler tells users to set 2 + MULTIPLEX; for S3/Azure/CDSE it's moot (probe).
4. GDAL_INGESTED_BYTES_AT_OPEN: docs no default; code default 1024 -> 16 KiB after chunk rounding; TiTiler says 16 KB.
5. GDAL_MAX_RAW_BLOCK_CACHE_SIZE, GDAL_HTTP_ENABLE_ADVISE_READ, CPL_VSIL_CURL_ADVISE_READ_TOTAL_BYTES_LIMIT: critical to COG reads, absent from configoptions.rst.
6. VSI_CACHE advice conflict: TiTiler recommends VSI_CACHE=TRUE (5 MB/handle); odc-loader forces it FALSE during reads; stackstac TRUE only at open, FALSE at read (rationale: don't evict header cache).
7. Folk advice "configure_rio(cloud_defaults=True) gives an order-of-magnitude speedup" (Pangeo 2024) vs current odc-loader code, which applies the same defaults automatically when nothing is configured -> advice may be stale for >=0.5; the "order of magnitude" had no like-for-like baseline.
8. odc.stac.load docstring: `pool` "Use thread pool to perform load locally" - code default `pool=None` = serial.
9. stackstac README "Don't use this in production" and no maintenance vs ongoing heavy use (issue #270 comments); PyPI/README still the top STAC->xarray hit.
10. odc-stac benchmark report conclusion "stackstac faster in deep case" (121.8 vs 105 Mpix/s) contradicts the later folk consensus "use odc-stac"; the advantage claimed for odc-stac is wide-mosaic graph size/fusion + overviews, not raw read speed.

====================================================================
## 9. Open questions
====================================================================
1. What is the actual compressed bytes/s and decode CPU share in the odc/stackstac 105-122 Mpix/s runs (needs `CPL_VSIL_NETWORK_STATS` rerun on PC)? Without it we can't tell if the ceiling is NIC, decode, or latency.
2. What is the end-to-end gain from GDAL_NUM_THREADS + GDAL_MAX_RAW_BLOCK_CACHE_SIZE + high dask threads over odc defaults on S2 (nobody published a sweep)? This is the baseline gap that decides novelty.
3. Does GDAL master's single-run-thread curl design (3.14) help or hurt many-thread COG reads? Unbenchmarked.
4. Do S2 COGs on PC (512 block?) vs Earth Search c1 (1024?) vs legacy AWS COGs differ enough in block layout to change the best chunk? (I only have a secondary claim for PC = 512.)
5. GIL/thread scaling of rasterio+dask beyond 16-32 threads on S2-size reads: no public data.
6. Is there any independent reproduction of lazycogs-vs-odc-stac, or of async-tiff pixel (not header) throughput vs GDAL? I found none.
7. CDSE (JP2/TLM, 4-connection cap) is a quota-bound case where engine choice barely matters - is it in scope for the study?

====================================================================
## 10. URLs (all)
====================================================================
GDAL docs/code:
- https://gdal.org/en/stable/user/configoptions.html  (src: https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/doc/source/user/configoptions.rst)
- https://gdal.org/en/stable/user/virtual_file_systems.html
- https://gdal.org/en/stable/user/multithreading.html
- https://gdal.org/en/stable/development/rfc/rfc101_raster_dataset_threadsafety.html ; rfc90 ; rfc26
- https://gdal.org/en/stable/drivers/raster/gtiff.html ; libertiff ; stacit ; gti ; zarr ; cog
- https://gdal.org/en/stable/programs/gdalwarp.html ; gdal_raster_pipeline
- https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/port/cpl_vsil_curl.cpp
- https://github.com/OSGeo/gdal/blob/v3.12.4/port/cpl_vsil_curl.cpp
- https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/frmts/gtiff/gtiffdataset.cpp
- https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/frmts/gtiff/gtiffdataset_read.cpp
- https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/gcore/gdal_thread_pool.cpp
- https://github.com/OSGeo/gdal/blob/5154115825852ee99091b939886d3f24bddb10bd/NEWS.md
- https://github.com/OSGeo/gdal/pull/15007 (curl run thread) ; issues 14997, 13528, 8041, 12426
odc / stackstac / others:
- https://github.com/opendatacube/odc-stac (d3345ed) ; https://github.com/opendatacube/odc-loader (38395b1) ; https://github.com/opendatacube/odc-geo
- https://github.com/opendatacube/odc-loader/blob/38395b17e1647e9b86cb3257a576a2f8aaee1d01/src/odc/loader/_rio.py
- https://github.com/opendatacube/odc-stac/issues/54 , /241 ; https://odc-stac.readthedocs.io/en/latest/benchmarking.html
- https://benchmark-odc-stac-vs-stackstac.netlify.app/
- https://github.com/gjoseph92/stackstac (3857190) ; issues 270, 271 (PR), 61, 199, 196, 18
- https://github.com/corteva/rioxarray/blob/c48a1a38c326d08727a448eace3a90e908801988/rioxarray/_io.py
- https://github.com/cogeotiff/rio-tiler ; https://developmentseed.org/titiler/advanced/performance_tuning/
- https://github.com/developmentseed/cogeo-mosaic ; https://github.com/appelmar/gdalcubes_R ; https://github.com/google/Xee ; https://github.com/Open-EO/openeo-python-client
- https://documentation.dataspace.copernicus.eu/APIs/openEO/openEO.html ; https://documentation.dataspace.copernicus.eu/Quotas.html
- https://developers.google.com/earth-engine/guides/usage
- https://docs.sentinel-hub.com/api/latest/api/process/ (moved to docs.planet.com) ; community.planet.com threads on limits
Benchmarks / evidence:
- https://github.com/opendatacube/benchmark-rio-s3/blob/master/report.md
- https://github.com/geospatial-jeff/pyasyncio-benchmark (results/20250423_results/ec2_m5_300seconds/aggregated_results.csv)
- https://developmentseed.org/lazycogs/performance/ ; https://developmentseed.org/lazycogs/notebooks/lazycogs-odc-stac/
- https://developmentseed.org/async-geotiff/latest/blog/2026/02/03/introducing-async-geotiff/
- https://developmentseed.org/warp-resample-profiling/pages/ecosystem.html
- https://github.com/cmosig/sentle/pull/93
- https://discourse.pangeo.io/t/comparing-odc-stac-load-and-stackstac-for-raster-composite-workflow/4097
- https://discourse.pangeo.io/t/cloud-optimized-access-to-sentinel-2-jpeg2000/5072
- https://discourse.pangeo.io/t/cloud-optimized-geotiffs-pangeo-best-practices/999
- https://blog.terrafloww.com/efficient-cloud-native-raster-data-access-an-alternative-to-rasterio-gdal/
- https://element84.com/cloud-architecture/aws/sentinel-2-cloud-optimized-geotiffs-now-available-on-aws-registry-of-open-data/
- https://cloudnativegeo.org/blog/2024/08/introduction-to-stac-geoparquet/ ; https://guide.cloudnativegeo.org/cloud-optimized-geotiffs/intro.html
- https://rasterio.readthedocs.io/en/latest/topics/concurrency.html ; https://github.com/rasterio/rasterio/blob/main/CHANGES.txt
- https://stackstac.readthedocs.io/en/latest/examples/cluster.html
