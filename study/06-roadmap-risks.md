# 06 — Implementation roadmap, go/no-go gates, and risks (Deliverables 6 and 7)

Status: proposal for review. **Default at every gate is STOP unless the criteria are met.** Thresholds below are my proposals, not established values; adjust them before Phase 0 starts and then freeze them (pre-registration, [04 §8](04-benchmarks.md)).
Effort figures are `[INFER]` guesses for one developer with coding-agent help, in calendar weeks. They are the least reliable numbers in this study.

---

## 1. Roadmap

### Phase 0 — Evidence, no engine (≈ 2–3 weeks)
Nothing here is Tile-Ripper code. It produces the facts every later decision depends on.

| Step | Output |
|---|---|
| **0a Tune what exists** (≈ 2–3 days) | The current ingest (`/Users/jakegearon/projects/chronozarr/examples/sentinel2_pc/mosaic.py`) is rasterio `reproject` per band per scene with `ThreadPool(4)` over scenes, `GDAL_HTTP_MULTIPLEX=YES` (no effect on HTTP/1.1 Azure), no `GDAL_NUM_THREADS`, no `GDAL_MAX_RAW_BLOCK_CACHE_SIZE`, and `pc.sign(href)` per read (PC signing returns 429 under load; verify whether the library caches tokens). Sweep workers/GDAL settings per [04 §4.1](04-benchmarks.md). Measure 6.7 s/month → ? This is the cheapest possible test of "is the problem concurrency, not language?" |
| **0b Harness skeleton** | Frozen scene lists (JSON + SHA-256), oracle (GDAL window/warp), floor probes (`T_io_floor` fetch-only with concurrency sweep, `T_dec_floor` offline), trace + NIC capture, run manifest |
| **0c E0 baselines** | S1 tuned GDAL pool, S2 tuned odc-stac, S3 lazycogs, S4 Rasteret, S5 sentle, S7 whole-object download on W1–W3; E-WS and E-CLOUD; cold/warm |
| **0d E2/E3 probes** | Concurrency–throughput curves per host; **scheduling isolation** with throwaway fetch-only scripts (same reader, per-asset vs global queue). These scripts are measurement tools, not the engine |
| **0e Review & freeze** | Pre-registered thresholds for G0/G1/G2 |

**Gate G0 — Is there an engine-shaped opportunity?**

| Outcome | Criterion (proposed) | Decision |
|---|---|---|
| **GO** | On E-CLOUD, for ≥ 2 of {W1, W2, W3}, the *best tuned* existing tool has efficiency `T_floor/T_tool ≤ 0.5` (≥ 2× headroom) in both cold and warm **and** E3 shows ≥ 1.3× from a global queue + adaptive controller over per-asset scheduling on the same reader | Proceed to Phase 1 |
| **NARROW** | Efficiency 0.5–0.7, or headroom only for tail latency / memory / streaming rather than throughput | Continue only as a **planner/scheduler library** feeding the existing GDAL path; reassess scope with the owner |
| **NO-GO** | Best tuned tool ≥ 0.7 efficiency on all core workloads, or all headroom is in regimes the owner doesn't use | **Stop the engine.** Deliver: the tuned recipe, the benchmark harness (no rigorous public one exists), upstream contributions (async-tiff decode-into-buffer, `object_store` connector/metrics hooks, a published tile-offset layout index), and a faster `chronozarr` ingest example |

A NO-GO is a successful outcome of this study: it costs 2–3 weeks instead of 3–6 months.

### Phase 1 — Plan + fetch only (≈ 4–6 weeks)
Crates: `tr-core`, `tr-discovery` (STAC API; ES v1/v2, PC, CDSE normalisers), `tr-layout` (header fetch, persistent index), `tr-planner`, `tr-io` (custom `HttpConnector`, scheduler, `Fixed` + `Aimd`), `tr-bench` integration. No decode.

**Gate G1**
- Fetch-only TR within **15%** of `T_io_floor` on E-CLOUD for W1–W3.
- ≥ **1.3×** faster than the best tuned tool's fetch phase on ≥ 2 workloads (else explain why G0's isolation result didn't transfer; default stop).
- W2 (bandwidth-bound) no worse than **5%** vs floor.
- Provider-safety gates pass on ES and PC (429/503 < 0.1%, no ceiling violations).
- Plan hash deterministic; `explain()` estimates within 5% of actual requests/bytes.
- Peak memory ≤ budget × 1.1 at three budgets.

### Phase 2 — Decode + native-grid extraction + streaming sink (≈ 4–6 weeks)
`tr-decode` (libdeflate, fused predictor/15-bit unpack/crop, zstd, buffer pool, rayon pool), `tr-format` COG, `tr-transform` Identity/IntegerShift/Scale (nearest), FirstValid/Min/Max/Mean/Median/Count accumulators, `tr-sink` local + **Arrow-IPC/NPY stream into the existing Python `chronozarr` encoder** (no Rust ChronoZarr sink yet). Decision point: async-tiff (upstream PRs) vs own ~500-line COG-subset parser.

**Gate G2**
- **Bit-exact** vs GDAL oracle on all native/shift/scale cases across W1–W3 and on a PC tile (15-bit) and an ES tile; radiometry per-provider pixel checks pass.
- End-to-end `T_total` ≤ **0.67×** (≥ 1.5× faster) than the best tuned tool on ≥ 2 core workloads on E-CLOUD, and **not slower** (CI includes 1) on E-WS.
- CPU time per useful MB within **20%** of the GDAL baseline; otherwise wrap GDAL decode rather than defend a slower Rust decoder (C1).
- Peak RSS ≤ budget × 1.1 under shuffled arrival and cell-major/time-major orders; no sink-stall deadlocks.
- ChronoZarr output validates (`chronozarr validate`, independent reader checksum).

### Phase 3 — Transform, JP2, breadth (≈ 4–6 weeks)
`GridMapper::Warp` **delegated to GDAL** (`gdal-sys`), bilinear/cubic via GDAL, temporal grouping/reduction, `ZonePolicy`, ES v1/v2 + PC normalisation hardened, JP2 via OpenJPEG with TLM front-end (feature-gated).

**Gate G3**
- Warp nearest bit-exact with `gdalwarp -r near`; bilinear/cubic within declared tolerance.
- No regression vs tuned GDAL on reprojected W1.
- **Do not write a Rust warp or JP2 decoder unless** GDAL warp > 30% of `T_total` (or JP2 decode is the measured bottleneck) *and* a parity corpus exists. Otherwise the answer stays "delegate".
- JP2 path decodes real baseline-N05.12 S2 files bit-exactly vs OpenJPEG with TLM range assembly validated on baselines with and without TLM.

### Phase 4 — Hardening: adaptive scheduling, hedging, multi-provider (≈ 3–4 weeks)
AIMD tuning, optional hedging, SAS single-flight refresh, CDSE quota compliance, retry budgets, connection warm-up, E7/E9 experiments, observability polish, harness frozen and the hidden test set created.

**Gate G4** (entry to the agent loop)
- Field validation on ES and PC over ≥ 3 days: 429/503 < 0.1%; hedging either shows p99 improvement with ≤ 5% byte overhead or stays off.
- Harness, oracle and test set are path-protected and reviewed; E-LAB profiles reproduce field ordering of SUTs (sanity check that the lab predicts the field).
- Manual sensitivity round shows ≥ 3 knobs with > 10% effect (otherwise the agent loop isn't worth building).

### Phase 5 — Agent-driven optimization ([05](05-agent-optimization.md)) (≈ 3–4 weeks, then continuous)
**Gate G5:** held-out (test) gain ≥ **50%** of the validation gain; all hard gates hold; ledger complete; defaults safe without tuning.

### Phase 6 — Sinks and stabilisation (as justified)
Rust ChronoZarr sink only if, on sink-bound runs, the Python encoder is > **25%** of `T_total` (`zarrs` pinned behind one boundary, bit-exact `downsample_2x` fixtures). Other sinks, docs, API stabilisation. **Gate G6:** a third party reproduces a headline benchmark from the repo and the manifest.

### Standing rules between gates
- One logical change at a time; each phase ends with a written result against its gate, including failures.
- Re-run the baseline when **GDAL 3.14** ships (master is due 2026-11-08; shared curl multi handle, 3.13 `ReadMultiRange` retry) — the bar may move [N04 §5].
- Re-check the substitution threats ([02 §3](02-strategy.md)) at each gate: EOPF operational status, VirtualiZarr/Icechunk S2 archives, odc-stac/lazycogs merger (lazycogs#89), Rasteret updates.

---

## 2. Unresolved decisions for the owner (answer before Phase 0)

1. **Primary target environment:** laptop/workstation, or cloud-near-data (or HPC)? The evidence says the workstation regime is bandwidth-bound and headroom is mostly cloud/high-NIC. This decides whether an engine can matter to you.
2. **What is Tile-Ripper for?** An internal ingest accelerator for chronozarr (then G0 should be judged on your workloads W1/W3, not W2), or a general OSS acquisition tool (then breadth and provider coverage dominate cost)? And does it reuse the "TileRipper" name per your earlier note?
3. **Semantics to guarantee:** stack vs mosaic, resampling kernels, duplicate-scene policy, radiometric harmonisation default. These decide how much dedupe/overview benefit a planner can bank.
4. **Spend authority:** cloud budget for E-CLOUD/E-PC runs (order of tens to low hundreds of dollars `[INFER]`; verify with the AWS/Azure calculators) and whether River/HPC can host the same-region runs.
5. **Rust supply-chain policy:** your policy covers npm and uv. Several key crates move fast (`zarrs_filesystem` 0.3.13 and `icechunk` 2.3.0 were inside a 7-day window at audit time; `proj4rs` 0.2.1 shipped the day of the audit). What is the cargo equivalent (lockfile + `--locked` + `cargo-deny`/`vet`, or a registry cooldown)?
6. **Sensor scope:** Sentinel-2 only, or also Landsat/HLS/SWOT rasters (affects the format abstraction and what "native grid" means)?
7. **Acceptable-use stance:** are you comfortable running ≥ 100-connection tests against Planetary Computer and CDSE (shared public infrastructure, undocumented/quota limits)? Cloud-region tests against `sentinel-cogs` are the safer default.
8. **Time box:** how many weeks is this allowed to take before it competes with the postdoc and with chronozarr/dgov/scilint/quarry work (scope-creep flag)?
9. **ChronoZarr spec coupling:** v0.3.0 is Draft and lost star-delta five days before this study; should the engine target a frozen sink contract, or track the draft?

---

## 3. Risk register (Deliverable 7)

L = likelihood, I = impact (H/M/L), both my judgement. Evidence tags → `evidence/`.

| # | Risk | Evidence | L | I | Mitigation / detection |
|---|---|---|---|---|---|
| R1 | **No headroom**: a tuned GDAL/odc-stac is already near the floor; the engine duplicates it | No rigorous tuned-baseline benchmark exists; GDAL does range merge, parallel fetch, multithreaded decode, overview choice [N04 §2]; Rasteret/sentle already ship index-first and dedupe [N08] | **M–H** | **H** | Phase 0 / G0 before any engine code |
| R2 | **Substitution / moving target**: virtual-Zarr + Icechunk, ESA EOPF Zarr (operational "late 2026/early 2027"), GDAL 3.13/3.14 (shared curl handle, `ReadMultiRange` retry, GTI over STAC-GeoParquet, Zarr v3 sharding), odc-stac absorbing lazycogs (#89) | [N08 §A.3], [N04 §2,5], [N05 §8], [N01] | M | H | Re-check each gate; keep scope on planner/scheduler/sink coupling that those do not address; track GDAL 3.14 |
| R3 | **Provider throttling caps the win**: PC limits unpublished, SAS-signing 429s, CDSE 4 conn × 20 MB/s + 2000 req/min + 12 TB/30 d, S3 SlowDown ramp | [N05 §5], [N03 §limits] | H | M | Per-host ceilings as non-agent-editable config; benchmark honestly reports quota-bound runs; E9 gentle ramp |
| R4 | **Etiquette/ban risk** from aggressive concurrency on free shared hosts | [N05], [N02 §8] | M | M | Identifying UA, documented caps, kill-switch on sustained 429/503, cloud-region tests preferred |
| R5 | **HTTP/1.1 connection explosion** (200–250 connections to fill 100 Gbps; FD/ephemeral-port limits; TLS CPU; idle-timeout mismatch S3 ~5 s vs pool 90 s, unverified) | [N03 §3], [N08 §B.4], [N03 contradictions] | M | M | Measure in E2; pool idle < server idle; test OS limits; warm-up connections |
| R6 | **`object_store` gaps**: per-store clients, no metrics/`Retry-After`/budget, `HttpStore` needs 206, 1 MiB fixed coalesce gap, fixed 10-way parallelism, h2 pooling issues, DNS failures at scale (obstore#273, anecdotal), credential mutex fixed in #648 | [N03] | M | M | Custom `HttpConnector`; own scheduler above the trait; upstream contributions; consider bespoke hyper only if profiling demands |
| R7 | **async-tiff dependency**: single maintainer, pre-1.0 (breaking minors), `.expect()` panics, tiled-only, no 15-bit (PC), planar+predictor bug (#346, fix PR #348), decode ~1.7× slower than GDAL at 16 threads, owned-`Vec` decode, MIT vs "MIT OR Apache-2.0" mismatch | [N01 §3–4] | H | M | Contribute upstream or own the ~500-line COG-subset parser; fuzz corpus; decode via libdeflate |
| R8 | **`zarrs` dependency**: bus factor 1 (1852 vs 15 commits), 28–43 breaking bullets per minor, no bounded-memory shard writer, no partial set on object stores, `store_chunk` of an all-fill chunk **deletes** the key, `zarrs_filesystem` 0.3.13 now `fsync`s every write (macOS F_FULLFSYNC cost) | [N06 §4–6] | M | M | Pin exact version; one `put_chunk` boundary; fallback ~200-line writer (zstd + serde_json + crc32c); default to the Python encoder in Phase 2 |
| R9 | **JP2**: ~20–40× deflate CPU per tile `[INFER]`; `hayro-jpeg2000` whole-image/f32/bit-exactness fixed 2026-07; TLM only on recent baselines; AI-written "native" decoders failed on real S2 | [N07 §4] | M | M | OpenJPEG ≥ 2.5.3 behind a feature; TLM front-end; parity tests; treat CDSE as correctness/ground-truth host |
| R10 | **Correctness hazards**: +1000 DN offset (not in PC STAC; wrong flag on ES legacy), `_0_/_1_` duplicates, PixelIsPoint, tiepoint (I,J), 4-corner source window (edge pixels in non-conformal CRS), nodata-only vs SCL masking, tie-break determinism, block layout may vary by processing baseline (unverified) | [N05 §3], [N01 §2, contradictions 6], [N07] | H | **H** | Oracle + hard gate; explicit `RadiometryPolicy`; per-provider pixel checks; property tests; never guess offsets |
| R11 | **Benchmark validity**: noisy networks, provider drift/reprocessing, PC vs ES catalogue gaps (25 vs 30 items for one tile/two months), GDAL version changes, hidden caches, report-only-the-win bias | [N05 §10], [N04] | H | H | Frozen scene lists + ETags, interleaved randomised runs ≥ 10, CI-based claim rule, tuned baselines swept not picked, publish nulls |
| R12 | **Planner gains are bounded**: bandwidth-bound regimes; small-AOI over-read is inherent to tile granularity (~67× for a 1 km chip; ES 5.6×/PC 3.1× at 512² chips); coalescing can be slower on some layouts (cptkirk) | [N08 §B.4–5, B.7], [N05 §6] | H | M | Lead with byte-reduction levers (QA-first, dedupe, overview choice, whole-object mode); consider partial-tile prefix decode only after measurement (idea, no prior art) |
| R13 | **Backpressure/memory design bugs** (deadlock, unbounded queues, admission starvation) | design risk | M | H | Out-chunk-working-set admission rule, reserved permit, property tests with adversarial arrival orders, runtime assertions |
| R14 | **Runtime oversubscription**: tokio + rayon + GDAL/OpenJPEG threads; GDAL curl handles thread-local; `proj` `!Send`; GDAL block-cache/global locks | [N04 §5], [N07 §8] | M | M | Single-owner parallelism (set library thread pools to 1); one `Proj` per worker; separate I/O and CPU runtimes; CPU/NIC utilisation gate |
| R15 | **Scope and maintenance burden** (single owner, multi-month; engine invites feature pressure; competes with chronozarr/dgov/scilint/quarry/research) | owner context; lazycogs shows a single-maintainer project stalled after 13 releases in 9 weeks | **H** | H | Explicit non-goals (03 §12), gates default to stop, NO-GO is a valid outcome, keep the engine's contract with ChronoZarr thin |
| R16 | **ChronoZarr coupling**: spec still Draft (v0.3.0 dated 2026-10-03 removed star-delta); `coverage` semantics inconsistent (spec count vs 0/1 flag); §8.3 working-copy rule vs streaming; consolidated metadata not in Zarr spec (zarr-specs #309); pyramid must be bit-exact | [N06 §3, contradictions] | M | M | Phase-2 sink = stream to Python encoder; golden fixtures before any Rust port; resolve spec ambiguities upstream |
| R17 | **Agent-loop failure modes**: overfitting, benchmark gaming, winner's curse, noise-chasing, provider abuse, runaway cost | [05] | M | H | Hidden test set, harness isolation, hard gates, confirmation runs, complexity penalty, budgets |
| R18 | **Licensing/provenance**: permissive stack (MIT/Apache/BSD: object_store, libdeflate, OpenJPEG, PROJ, GDAL, zarrs, rustac) is fine; avoid Grok (AGPL), Kakadu (proprietary), `eorst` (LGPL); async-tiff `Cargo.toml` MIT-or-Apache vs MIT-only LICENSE; TGE mirror is a young Apache-2.0 repo and not spec-conformant; Sentinel attribution required | [N01], [N07 §4], [N02 §7], [N05 §9] | L | M | Use MIT where ambiguous; license audit in CI; attribution string in outputs |
| R19 | **Supply chain / cargo quarantine** (see decision 5); fast-moving crates | audit-time observation | M | M | `Cargo.lock`, `--locked`, exact pins, `cargo-deny`; decide mechanism |
| R20 | **Catalog churn**: Earth Search v1→v2 (STAC 1.1 reshaping, `proj:code`), legacy vs C1 collections, PC parquet lag (~6 days, duplicate part files in live months), STAC API paging strictly serial in `rustac`, `stac-api` crate frozen | [N02 §4,7,8] | M | M | Normaliser tests against both ES versions; parse raw JSON; time-window concurrent paging; reconcile mirrors |
| R21 | **Unverified inputs in this study**: many figures are single-sample agent probes or `[INFER]` (e.g. S3 gateway endpoint not IGW-capped, PC 45-min SAS, tile-offset gaps, JP2 CPU multiple, 15-bit universality) | marked throughout | M | M | Each is an explicit E-series question; nothing is promoted to a claim without a measurement |

---

## 4. What would change this recommendation
- G0 NO-GO → stop the engine, ship harness + tuned recipe + upstream work.
- A rigorous independent benchmark showing a tuned GDAL/odc-stac far from the floor in a regime the owner uses → strengthens GO.
- Rasteret/odc-stac/GDAL 3.14 closing the measured gap → NO-GO.
- EOPF Zarr operational with global coverage → the Sentinel-2 "acquire pixels" problem shrinks; re-scope toward other collections.
