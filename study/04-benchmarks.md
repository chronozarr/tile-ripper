# 04 — Benchmark specification (Deliverable 5)

Status: spec for review. **Nothing here has been run.** No performance claim is made anywhere in this study; every number marked `[model]` or `[INFER]` is a hypothesis to be tested by this spec.
Evidence tags `[N0x]` → `evidence/0x-*.md`.

---

## 0. What the benchmark must be able to conclude

Three questions, in this order. A "no" to the first ends the project as an engine (see 06, Gate G0).

1. **Is there headroom?** How far is the best tuned existing pipeline from a measured *floor* (the time to fetch exactly the needed bytes and decode them, with nothing else)?
2. **Where does headroom come from?** Scheduling, header round trips, dedupe, over-read, memory/streaming, decode, or concurrency cost?
3. **Does Tile-Ripper close it** without losing pixel correctness or bounded memory?

Two facts shape the design:
- **No rigorous public benchmark of a tuned GDAL/odc-stac pipeline on Sentinel-2 L2A exists.** The closest (odc-stac vs stackstac on PC) leaves GDAL env, threads and versions unstated; lazycogs-vs-odc-stac is a single laptop run with mismatched chunking/dtype; obstore's CSV shows parity above ~64 KiB per request and an invalid fsspec baseline [N04 §4, N01 §4, N03]. So the baseline *is* part of the work.
- **Workstation regime is bandwidth-bound.** At ~1 Gbps a 1.8 GiB large-AOI job is bytes-bound (~15.6 s) and scheduling is irrelevant [N08 §B.7]. Headroom, if any, lives in cloud-near-data runs with big NICs, under-concurrent baselines, and tail latency. The matrix therefore weights cloud runs.

---

## 1. Floors and efficiency metric

For every (workload, environment, cache state) the harness produces two floor times with **independent, minimal** code paths:

| Floor | Definition | How measured |
|---|---|---|
| `T_io_floor` | Fetch exactly the byte ranges the oracle says are needed (tile spans, ideal coalescing allowed), no decode, discard bytes. Concurrency swept 1…512; take the best. | Small instrumented fetcher (raw HTTP/`object_store`), same host/region as the tool runs |
| `T_dec_floor` | Decode all needed tiles from RAM on all cores, no I/O. | Offline: tiles cached locally, libdeflate/OpenJPEG, P cores |

`T_floor = max(T_io_floor, T_dec_floor)` (perfect pipelining). **Efficiency = T_floor / T_tool.** Reporting efficiency, not just wall time, makes results comparable across environments and shows where each tool sits relative to physics. The oracle's `ideal_wire_bytes` (compressed size of tiles strictly intersecting the planned windows, computed from headers offline) gives `wire_amp = wire_bytes / ideal_wire_bytes` for every tool.

---

## 2. Workloads

Sizing arithmetic is from [N05 §6] (STAC counts, parsed block sizes, measured compressed sizes), all `[INFER]` from those inputs. All use B02/B03/B04/B08 (uint16) **plus SCL** as mask, Earth Search C1 L2A as primary host (1024² blocks, Deflate + predictor 2, anonymous).

| ID | Shape | Useful decoded | Scenes / items | GETs (ES, 1 per block) | Compressed read (ES / PC) | Regime |
|---|---|---|---|---|---|---|
| **W1 dense-small** | 25 km AOI (Ucayali, 2765×2759 px), **all** acquisitions, ~22 months | 62.9 MB/date × 191 dates = **12 GB** | 385 tile-items (2 tiles/date) | ~15.4k (+~2k headers/SCL) | 20.1 / 19.8 GB (1.65–1.7× useful: block rounding + 9.8 km tile overlap) | many scenes, moderate AOI |
| **W2 sparse-large** | 1 full MGRS tile, 1 scene per month × 12 | 995 MB/scene × 12 = **11.9 GB** | 12 | ~7.3k | 7.6 / 9.5 GB (0.63× — compressed) | bytes/decode-bound |
| **W3 many-chips** | 500 chips of 512² px, 12 monthly dates | **12.6 GB** | 6,000 scene-bands | ~54k (+~24k headers) | 70 GB (5.6× useful) / 39 GB (3.1×) | request- and over-read-bound |
| **W4 multi-tile mosaic** | ~3×3 tiles (≈ 300×300 km), 1–2 dates, one UTM zone | ~9–18 GB | 9–18 | ~9k–18k | ~7–14 GB | cross-tile paste/dedupe, large writes |
| **W5 overlap-heavy** *(held-out)* | 24×28 km at 52°N (NL), ~11 months, all dates | 30.7 MB/item × 391 = **12 GB** | 391 | ~11.7k | ~15.2 GB | orbit overlap, duplicates |
| **W6 zone-straddle** *(held-out)* | AOI crossing a UTM zone boundary | ~12 GB | — | — | — | forces `Warp`/`PerZone` |

Notes:
- "~12 GB useful, monthly time series" is the headline (W1 and W2 both hit it from opposite sides: many scenes/moderate AOI vs few scenes/large AOI). W3 is the only honest way to test "small AOIs intersecting many scenes" at 12 GB (one 5 km AOI × 10 yr is only ~3 GB with 10 bands).
- Output cadence is a separate axis: `PerScene` (no temporal reduce; full 12 GB written) vs `MonthlyMedian` (W1 output ≈ 1.37 GB). They stress different stages (write vs reduce).
- Duplicates: same-tile-same-date S2A+S2B at high overlap (~18% at high latitude), reprocessing sequences `_0_/_1_/_3` (1–2% tropics) — the oracle fixes the dedupe rule so tools are compared on identical selected scene sets.
- Provider differences matter: PC uses 512² blocks and Deflate without predictor (≈3× the GETs, 15-bit packed). PC is a *secondary* host (Azure West Europe), never mixed with ES numbers.

### 2.1 Axes (and which cells are actually run)

Full factorial is not run. Axes:

| Axis | Levels |
|---|---|
| AOI size | W3 chips (5 km) · W1 (25 km) · W2/W4 (110–300 km) |
| Temporal | dense (all dates) · sparse (1/month, least cloudy per STAC `eo:cloud_cover`) |
| Output grid | Native (same UTM zone) · integer-shift mosaic · 20→10 m `Scale` nearest · true warp (EPSG:4326 and a polar/other-zone) |
| Reduction | PerScene · MonthlyMedian · FirstValid(SCL) |
| Environment | E-WS workstation · E-CLOUD (AWS us-west-2, S3 gateway endpoint) · E-PC (Azure West Europe, secondary) |
| Cache | Cold · Warm (§5) |
| I/O mode | Selective ranges · whole-object download + local decode |

**Core matrix (run in full, ≥ N reps):** {W1, W2, W3} × {Native} × {PerScene} × {E-WS, E-CLOUD} × {cold, warm} × all tools. **Extended:** reproject/aggregation variants on W1; W4 on E-CLOUD; E-PC on W1 only. **Held-out (not used for tuning):** W5, W6, a different continent's tile, PC host, a second instance type, a different month (§7 and 05).

---

## 3. Environments

| ID | Spec | Why |
|---|---|---|
| **E-WS** | Owner's laptop (record chip, RAM, OS), wired if possible; `iperf3`/speed baseline to a same-region host recorded per session | Real-user regime; expected bandwidth-bound |
| **E-CLOUD** | AWS **us-west-2** (same region as `sentinel-cogs`), network-optimised instance (e.g. c6in.8xlarge 50 Gbps or c7gn.8xlarge 100 Gbps — confirm NIC baseline in AWS docs at run time), **S3 gateway VPC endpoint** | In-region traffic is free (sponsored data, S3→EC2 same region $0). Without a gateway endpoint the IGW path is capped at 5 Gbps for <32 vCPU, and NAT adds ~$0.045/GB; endpoint-not-capped is `[INFER]`, verify with `E2` below [N05 §4–5] |
| **E-CLOUD-S** | Same region, small instance (≈ 4 vCPU) | Shows when a *tool* (not the NIC) is the limit; tests under-provisioned honesty |
| **E-PC** | Azure West Europe Dds_v5 (e.g. D32/D64 16–30 Gbps) | Only place PC limits relax; SAS refresh path; not comparable to ES numbers |
| **E-LAB** | Local object server (MinIO or nginx range server) with a **frozen mirror** of the workload's objects + `tc netem` (RTT, jitter, loss, bandwidth cap) | Deterministic, exact request counts, infinite repetition; used for tuning loops (05). Never used for headline claims |

CDSE (JP2, 4 connections × 20 MB/s, 2000 req/min, 12 TB/30 d) is **not** a throughput benchmark host: the quota is the ceiling. Use it only for JP2 correctness (raw-DN ground truth) and a quota-compliance test [N05 §5].

Before any session: record `uname`, CPU model, cores, RAM, governor, kernel, instance type/NIC speed, region/AZ, time, tool versions + commit SHAs, Rust/Python/GDAL versions, GDAL env, rustc flags, `Cargo.lock`/`uv.lock` hashes. Dedicated (non-burstable) instances; nothing else running; fixed CPU governor.

---

## 4. Systems under test (SUT)

All are given the **same frozen scene list** (STAC snapshot, §6) so discovery variance does not leak into pixel I/O. Discovery is also measured separately (§6).

| SUT | Configuration | Source |
|---|---|---|
| **S0 status quo** | `examples/sentinel2_pc` as shipped (rasterio `reproject`, ThreadPool(4) over scenes, `GDAL_HTTP_MULTIPLEX=YES`, PC `sign` per read) — only for PC/W1 | `/Users/jakegearon/projects/chronozarr/examples/sentinel2_pc/mosaic.py` |
| **S1 tuned GDAL/rasterio pool** | Hand-written minimal loop: one process, `rasterio.Env` with tuned settings, N worker threads (sweep), windowed reads (+`WarpedVRT`/`reproject` only for non-native). Settings below | [N04 tuned baseline] |
| **S2 tuned odc-stac** | `configure_rio(cloud_defaults=True, aws={"aws_unsigned": True, "region_name": "us-west-2"}, ...)`, `chunks={"x":2048,"y":2048}`, `groupby="solar_day"`, Dask threads swept 4–64 and/or `pool=N` | [N04] |
| **S3 lazycogs** | Pinned commit (reviewed: ca203a9); local stac-geoparquet; `max_concurrent_reads` swept; `chunks=` set to bound memory | [N01] |
| **S4 Rasteret** | v0.4.1; index built (build cost reported separately and also *included* in a "first-use" row — its own benchmark excludes it) | [N02 §3, N08 §A.1] |
| **S5 sentle** | Latest tagged; for W1/W4 only (end-to-end AOI,time→Zarr) | [N08 §A.1] |
| **S6 stackstac** | Historical reference only (unmaintained since 2024-08) | [N04 §7] |
| **S7 download-then-decode** (a *set*; report defaults and best-of-sweep for each) | Whole-object parallel download, then local GDAL decode. S3 headline: aws-cli with explicit CRT (`target_bandwidth` = NIC, `--no-sign-request`, tmpfs); secondary: s5cmd 2.3.0 sweeping `--numworkers` 16–256 (stale project, flagged); cross-host: rclone; Azure: AzCopy with SAS. None of these can read many spans, so S7 is a full-object comparator and bandwidth-ceiling probe, **not** an AOI baseline (over-fetches ~150× for a small chip). Ceiling probes: `aws-crt-s3-benchmarks`, warp | [N09](evidence/09-transfer-tools.md), [01 §6](01-ecosystem.md) |
| **S8 virtual-Zarr probe** | VirtualiZarr + virtual-tiff + Icechunk reading the same windows (substitution check); only if it can read the host's COGs (PC 15-bit may block) | [N08 §A.3] |
| **TR** | Tile-Ripper at each roadmap phase (Phase 1: fetch-only; Phase 2: + decode + native; …) | this study |

### 4.1 Tuned baseline for S1/S2 (concrete values, from [N04])
```
CPL_VSIL_CURL_ALLOWED_EXTENSIONS=.tif   GDAL_DISABLE_READDIR_ON_OPEN=EMPTY_DIR
GDAL_INGESTED_BYTES_AT_OPEN=32768       CPL_VSIL_CURL_CACHE_SIZE=200000000
GDAL_CACHEMAX=1024                      GDAL_BAND_BLOCK_CACHE=HASHSET
GDAL_MAX_RAW_BLOCK_CACHE_SIZE=268435456 (undocumented 10 MiB default; larger windows fall back to sequential reads)
GDAL_NUM_THREADS=ALL_CPUS               GDAL_HTTP_MAX_RETRY=3, GDAL_HTTP_TIMEOUT=45
GDAL_HTTP_MULTIPLEX / GDAL_HTTP_VERSION=2: NO effect on S3 / Azure / CDSE (HTTP/1.1 hosts) — leave default
```
**Sweep, don't pick:** worker threads {4, 8, 16, 32, 64, 128, 256}, chunk/window {512, 1024, 2048, 4096}, `GDAL_NUM_THREADS` {1, 4, ALL}. Report the **best configuration per (workload, environment)** as the baseline, and the *default* configuration as a separate row. GDAL 3.12.4 is the owner's local version; rasterio 1.5.2 wheels bundle 3.12.2; master (3.14, due 2026-11-08) changes curl handling (shared multi handle) and 3.13 adds `ReadMultiRange` retry — **record the exact GDAL version and re-run the baseline when GDAL 3.14 ships** [N04 §5].

---

## 5. Cold, warm, and what each layer means

Define cache layers explicitly; a run's row records which are cold.

| Layer | Cold | Warm |
|---|---|---|
| OS page cache | `purge` (macOS) / `echo 3 > /proc/sys/vm/drop_caches` (Linux), or fresh instance | not purged |
| Tool caches (GDAL block/VSI cache, async-tiff header cache, tile LRU) | new process, empty cache dir | second run in a **new process** with only persisted caches present |
| **Persisted layout index** (Rasteret parquet, TR `LayoutCache`) | absent | present — reported separately because it is a *feature*, with its build cost shown as a first-use row |
| DNS/TLS/connections | new process (no reuse) | `warm-conn`: repeated runs inside one long-lived process |
| Provider-side | S3/Azure have no CDN; STAC APIs (Lambda/CDN) may cache — discovery measured separately | — |

---

## 6. Measurements (per run)

| Metric | How | Notes |
|---|---|---|
| **Discovery latency** | Live API: request → complete scene list; N ≥ 20 on separate days; per provider (ES v1/v2, PC, CDSE) and vs. cached stac-geoparquet. Report pages, bytes, throttling | Measured separately from pixel I/O. Earth Search `fields` must `exclude` assets/links; ES caps `limit` at 10,000, PC 1,000, CDSE 200 [N02 §8] |
| T_plan, T_layout | Engine timers; for GDAL tools, time to first byte of pixel read | Layout = header GETs |
| **Time to first chunk / time to usable** | Time to first durable sink chunk; total to manifest | "Usable" = all requested chunks durable and checksummed |
| **T_total (wall)** | Process start → exit, `hyperfine`-style, outside the tool | Primary metric |
| **Bytes transferred** | (a) per-request logs where available (GDAL `CPL_VSIL_NETWORK_STATS_ENABLED` + `VSINetworkStatsGetAsSerializedJSON`; TR trace; obstore via wrapping the connector/proxy), (b) **independent**: NIC counters (`ip -s link`/`sar -n DEV`) or a pcap byte count; they must agree ±5% or the run is flagged | Includes headers, retries, hedge duplicates. Also `wire_amp` vs oracle ideal |
| **Useful pixel throughput** | useful decoded bytes (valid pixels inside the requested windows × dtype) / T_total; also Mpx/s. Prior art for orientation: odc-stac ≈105 Mpix/s deep, ≈75 wide; stackstac ≈122/30 on 4 cores with unstated GDAL env [N04] | |
| **Requests** | Count by type (STAC, HEAD, header GET, ranged GET, whole GET); **distinct TCP connections** (`ss -tn` sampling or pcap); retries; 429/503 counts; hedges | Per host |
| **Request latency** | p50/p95/p99 TTFB and total, by size class | |
| **Peak memory** | Linux cgroup `memory.peak` / `/usr/bin/time -v` (max RSS), **summed over child processes/Dask workers**; macOS `time -l` | Hard gate for TR: ≤ configured `memory_bytes` × 1.1 |
| **CPU utilisation** | cgroup `cpu.stat` usage / (wall × cores); user vs sys; per-stage if the SUT exposes it | |
| **NIC utilisation** | NIC bytes/s time series, % of instance NIC | Shows whether the *network* or the *tool* is the limit |
| **Output correctness** | §7 | Hard gate |
| **Cost** | VM-hours × rate + S3/Azure request/egress fees (usually $0 for sponsored public data) + egress if cross-region; formulas in [N05 §4] | Account for it even when $0 |
| **Rate-limit events** | 429/503, SAS-sign 429 ("Try again in 23 seconds"), CDSE quota hits | Runs that trigger throttling are reported as such, not discarded |

Auth/limits to respect (all from [N05]): ES anonymous; PC anonymous SAS ≈ 45 min per container (fetch once, refresh early; signing endpoint 429s); CDSE OIDC 10 min, S3 keys, 4 connections / 2000 req/min / 12 TB per 30 d; S3 5,500 GET/s per prefix with 503 SlowDown ramp. **Etiquette:** identifying `User-Agent`; per-host concurrency caps written down in the run config; stop on sustained 429/503. Public hosts are shared infrastructure; the benchmark itself must not become a DoS.

---

## 7. Correctness oracle (hard gate for every accepted result)

Oracle = GDAL (pinned version) reading the same windows from the same objects, **independent of every SUT**.

| Mode | Check |
|---|---|
| Native / identity | **Bit-exact** vs GDAL windowed read of every band; compare per-scene and for mosaics |
| Integer shift / mosaic | Bit-exact vs GDAL `-r near` onto the aligned grid (same shifted grid) |
| `Scale` 20→10 m nearest | Bit-exact vs GDAL nearest |
| True warp, nearest | Bit-exact vs `gdalwarp -r near` with the same error threshold (0.125 px default) — if delegated to GDAL it must match; for any non-GDAL warp report mismatch count/location and bound |
| Bilinear/cubic | Max absolute DN difference ≤ declared tolerance (set per kernel from a GDAL-vs-GDAL repeatability run), mismatch fraction reported |
| Mosaic/reduce | Order-independent: result identical across 3 shuffled arrival orders; ties broken by `(datetime, scene id)`; `MonthlyMedian` vs numpy median with the same mask |
| Radiometry | Per-provider **pixel check on one tile each** (do not trust flags): PC raw DN (baseline ≥ 04.00 → +1000 offset not in STAC), ES legacy `earthsearch:boa_offset_applied` (wrong on some 2025 items), ES C1/v2 raw DN with `raster:offset` −0.1, CDSE/AWS-JP2 raw DN [N05 §3] |
| Geometry conventions | PixelIsArea vs PixelIsPoint half-pixel shift; raster tiepoint (I,J) ≠ 0 (async-geotiff ignores it, geotoolz#414); non-conformal CRS edge pixels (lazycogs 4-corner window risk) [N01] |
| Masking | SCL class handling matches the declared policy; nodata-only masking differs from SCL masking — tools compared only under the same policy |
| Format edges | Striped/sparse tiles, planar + predictor 2/3 (async-tiff #346 bug), PC 15-bit packed tiles, JP2 lossless 15-bit |
| Stored output | Zarr/ChronoZarr: `chronozarr validate`, decode through an independent reader (zarrita/zarr-python), checksum of decoded arrays vs oracle; pyramid levels vs `downsample_2x` golden fixtures |

A run with any correctness failure is **recorded and excluded from performance comparison** (reported as a failure), never silently dropped.

---

## 8. Statistical protocol

- **Pre-registration:** hypotheses, primary metric, N, exclusion rules, and success thresholds are committed to the repo *before* data collection.
- **Replication:** lab ≥ 5 runs/cell; field (real hosts) ≥ 10 runs/cell, **interleaved and order-randomised** across SUTs (ABBA/randomised blocks), spread over ≥ 3 days and ≥ 2 times of day (provider load varies; PC is documented as throttled).
- **Statistics:** report median and IQR; effect = ratio of medians with a bootstrap 95% CI. No means of heavy-tailed data without the median beside it. Tail metrics (p99 completion) reported with their sample counts.
- **Claim rule:** "X is faster than Y" is allowed only if the 95% CI of the ratio excludes 1, the effect is ≥ 10%, and it reproduces in ≥ 2 independent sessions/days. Otherwise the report says "no detectable difference".
- **Negative and null results are published** with the same prominence.
- **Frozen inputs:** the STAC item list is snapshotted (JSON + SHA-256) and fed to all SUTs; objects are verified by ETag at run start; any provider-side change (reprocessing) invalidates the run.
- **Confounders recorded:** time of day, concurrent load, cold/warm state, instance placement, DNS, ISP route (E-WS), GDAL/driver versions.
- **No headline until the baseline is tuned:** results against the default-configured baseline are reported but labelled "untuned".

---

## 9. Required controlled experiments (map to architecture §7.3)

| ID | Question | Design |
|---|---|---|
| **E0 baseline sweep** | What does a *properly tuned* GDAL/odc-stac achieve on W1–W3, cold/warm, E-CLOUD and E-WS, and where is it on the efficiency scale? | S1/S2 sweep per §4.1 |
| **E1 header elimination** | Value of a warm layout index | Same reader, cold vs persisted index |
| **E2 concurrency/saturation curve** | b_conn, C*, per-host ceilings; does an S3 gateway endpoint lift the 5 Gbps IGW cap? | Fetch-only, C ∈ {1…512}, sizes {16 KiB … 8 MiB}, per provider/env |
| **E3 scheduling isolation** | Per-asset pools vs global queue + adaptive controller, **same reader/decoder/plan** | Fetch-only then full pipeline |
| **E4 over-read/dedupe** | Coalescing gap sweep {0, 1 KiB, 64 KiB, 1 MiB, 4 MiB}; tile-aligned vs naive ordering; overlap dedupe | W1, W3, W5 |
| **E5 ranged vs whole-object** | AOI-fraction crossover θ | Sweep 1%→100% of a tile on W2-type scenes |
| **E6 decode** | libdeflate vs zlib-rs vs flate2 on S2-like u16 + predictor on x86 *and* aarch64; JP2: OpenJPEG vs hayro on real S2 15-bit files | Offline microbench, per-core MB/s |
| **E7 hedging/tail** | p99 completion with/without hedging at ≥ 1000 requests | Fetch-only, real hosts |
| **E8 memory** | Peak RSS vs `memory_bytes` at several budgets; cell-major vs time-major | TR only |
| **E9 provider limits** | Where each host starts throttling at the concurrency a planner implies (>1000 GET/s) | Gentle ramp, stop at first sustained 429/503 |

E0 and E2 are executed **before any engine code exists** (Phase 0).

---

## 10. Full-object vs selective ranged reads (explicit)

Compare on identical scene sets: (a) selective ranged COG reads (each SUT's default), (b) `whole-object` parallel download then local decode (S7), (c) TR `ReadMode::Auto{θ}`. Measure the crossover as a function of AOI fraction of an object (E5). Expectations to test, not assume: ranged reads win when useful fraction is small (W1/W3); whole-object ties or wins as the AOI approaches the full object (W2), because per-request overhead and over-read vanish — Earth Search band files are ≈ 160–280 MB so a whole-object GET is just ~13 × 16 MB requests [N05].

## 11. Reporting template

One row per (SUT, workload, env, cache, mode): `T_total, T_ttfc, T_plan, T_layout, T_floor, efficiency, wire_bytes, wire_amp, requests{type}, connections, retries, p50/p95/p99, useful_Mpx_s, peak_RSS, cpu%, nic%, correctness, cost, run_ids, config_hash`. A report is invalid without the environment manifest, the config hash, the frozen scene-list hash, and the raw per-run trace files.
