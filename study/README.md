# Tile-Ripper feasibility study

Date: 2026-10-09 · Status: **read-only research and design, for review** · No production code written, nothing forked, nothing built, no optimisation started.

## Verdict

**Do not start building the engine. Spend 2–3 weeks proving there is something to build.** Then decide at Gate G0.

1. **Existing software already solves most of the intended problem.** GDAL already does range merging, parallel range fetch, multithreaded decode, overview selection, signing and retries. Rasteret (Python, active) already ships the "plan all byte ranges up front from a Parquet tile-offset index" idea. sentle already goes (AOI, time) → Zarr with overlap dedupe. Virtual-Zarr/Icechunk and ESA's native Zarr are a substitution path for the Sentinel-2 case.
2. **Rust is not the differentiator.** C beats Rust for deflate, zstd and JP2 decode; no Rust crate implements a GDAL-style warp; async-tiff decode was ~1.7× slower than GDAL's at 16 threads in its maintainers' own benchmark. Nothing in the "Rust-native" ecosystem is an acquisition engine: lazycogs and async-geotiff are 100% Python; only async-tiff has Rust, and it is a reader.
3. **Nobody has measured a *tuned* baseline.** No rigorous public benchmark of tuned GDAL/odc-stac on Sentinel-2 exists; the published speedups I found are against untuned or differently-shaped baselines. Whether a purpose-built engine beats tuned GDAL by a margin that matters is **unknown**. That is the novelty question, and it is a measurement, not a design question.
4. **The real gaps are orchestration, not primitives:** cross-asset global read planning with a persistent layout index; adaptive per-host concurrency and hedged range reads (nobody ships either); QA-first byte skipping; bounded-memory streaming from planner to sink; a zero-warp native-grid path; a reproducible benchmark harness.
5. **Where it can and can't help (model, unmeasured):** on a ~1 Gbps workstation everything is bytes-bound and scheduling is irrelevant. Headroom, if any, is in cloud-near-data runs with big NICs (hundreds of in-flight HTTP/1.1 connections), under-concurrent baselines, header round trips, overlap dedupe and tail latency. Small-AOI over-read is inherent to COG tile granularity (≈ 67× for a 1 km chip) and is not a scheduling problem.

**Recommendation:** Option B (standalone Rust engine composed from existing crates, C codecs and GDAL where Rust isn't faster), **gated**, with upstream contributions as the parallel path; **reject A** (lazycogs has no Rust to extract); C is not an alternative (no Rust project is an engine). Default at every gate is STOP. A NO-GO at G0 is a good outcome: it ships the benchmark harness, a tuned recipe, and upstream fixes at ~1/10 of the cost.

## Cheapest first move (days, not weeks)
Tune the pipeline you already run (`/Users/jakegearon/projects/chronozarr/examples/sentinel2_pc/mosaic.py`): it uses a 4-worker pool, `GDAL_HTTP_MULTIPLEX=YES` (no effect on HTTP/1.1 Azure), no `GDAL_NUM_THREADS`, no raw-block-cache raise, and `pc.sign()` per read. Measure the 6.7 s/month baseline against a swept configuration. If most of the gap closes, the problem was concurrency, not language.

## Follow-up measurements (2026-10-09/10): [07-discovery-and-planner-experiments.md](07-discovery-and-planner-experiments.md)
Hands-on, on a laptop link: the s2-stac-geoparquet queries reproduce, and its pruning and traps were measured; a DuckDB SQL planner produced bit-exact manifests from a one-GET-per-COG layout index; Rasteret, tuned GDAL and the manifest executor were compared on identical frozen scenes. Result: **discovery and pixel retrieval separate cleanly**; on this link tuned GDAL is within ~1.1–1.4× of the manifest floor (not claim-grade), untuned GDAL is 14× slower, and duckdb-cog is not a component. The cloud-near-data regime (Gate G0's other half) is still unmeasured. Scripts and raw results: [experiments/](experiments/).

## Deliverables

| # | Deliverable | File |
|---|---|---|
| 1 | Evidence-backed ecosystem comparison (capability matrix, project cards, benchmark credibility, provider ground truth, decode/transform) | [01-ecosystem.md](01-ecosystem.md) |
| 2 | Fork / build / extend recommendation with repository evidence | [02-strategy.md](02-strategy.md) §1–2 |
| 3 | Proposed Rust crate/module architecture, interfaces, global-planner analysis | [03-architecture.md](03-architecture.md) |
| 4 | Differentiation vs existing tools (and what is *not* differentiated) | [02-strategy.md](02-strategy.md) §3 |
| 5 | Benchmark specification (Sentinel-2, ~12 GB workloads, floors, oracles, statistics) | [04-benchmarks.md](04-benchmarks.md) |
| — | Agent-driven optimization framework (Phase 5) | [05-agent-optimization.md](05-agent-optimization.md) |
| 6 | Prioritised roadmap with go/no-go gates | [06-roadmap-risks.md](06-roadmap-risks.md) §1 |
| 7 | Unresolved technical risks (21) and open decisions | [06-roadmap-risks.md](06-roadmap-risks.md) §2–3 |
| — | Raw research notes with all URLs and pinned commit SHAs | [evidence/](evidence/) |
| 8 | Measured follow-up: stac-geoparquet reproduction, row-group pruning, DuckDB planner, Rasteret / tuned GDAL / duckdb-cog comparison | [07-discovery-and-planner-experiments.md](07-discovery-and-planner-experiments.md), [experiments/](experiments/) |

## How to read the evidence
- Grades: **[SRC]** read the code/docs · **[BENCH]** reproducible benchmark (code + method available) · **[CLAIM]** assertion without method · **[INFER]** inference · **[MEAS]** one-off probe, not a benchmark. `[N0x]` points to `evidence/0x-*.md`.
- **How this was gathered:** nine parallel research agents read upstream repos and docs on 2026-10-09 and wrote notes; I synthesised them. I spot-checked two load-bearing claims myself (lazycogs and async-geotiff are 100% Python by GitHub language stats; ChronoZarr v0.3.0 removed star-delta) and both held. I did **not** re-run any benchmark. Several figures are single-sample probes or inferences and are labelled.
- **Probing done by agents:** anonymous GET/HEAD/Range requests of ≤ 64 KB against public hosts, plus one anonymous Planetary Computer SAS token (free token endpoint, used only to list blob names, then deleted) by two agents. No imagery was downloaded or decoded; no accounts created.

## Things that changed what I would have assumed
- **ChronoZarr no longer has star-delta** (spec v0.3.0, 2026-10-03). Every (timestep, cell) chunk is independent, so a streaming writer can take level-0 chunks in any order. Your project memory notes still describe v0.2.
- **The sink isn't the bottleneck.** The Python encoder runs ~110–170 MB/s; fetch + warp is the cost. Phase 2 therefore streams into the existing Python encoder instead of writing a Rust sink.
- **Provider layouts differ**, so request counts don't transfer: Earth Search 1024² + Deflate + predictor 2; Planetary Computer 512² + Deflate, no predictor, 15-bit packed (async-tiff can't read it); CDSE/AWS/GCS are JP2. Offsets (+1000 DN, baseline ≥ 04.00) are a correctness trap on every provider.
- **S2 hosts are HTTP/1.1** (S3, Azure Blob, CDSE): concurrency = TCP connections; HTTP/2 tuning does nothing there.

## Decisions I need from you (before Phase 0)
Full list in [06 §2](06-roadmap-risks.md). The four that matter most:
1. **Laptop or cloud-near-data (or HPC)?** The evidence says the laptop regime is bandwidth-bound.
2. **Is this an ingest accelerator for chronozarr, or a general OSS tool?** That decides how G0 is judged.
3. **Cloud spend** for same-region benchmark runs, and whether you're comfortable running high-concurrency tests against Planetary Computer/CDSE (shared public hosts with limits).
4. **A cargo supply-chain policy.** Your quarantine rules cover npm and uv; several key crates (`zarrs_filesystem`, `icechunk`, `proj4rs`) published inside a 7-day window at audit time.
