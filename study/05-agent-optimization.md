# 05 — Agent-driven optimization framework (Phase 5)

Status: design for review. Nothing here starts until the benchmark plan ([04](04-benchmarks.md)) and architecture ([03](03-architecture.md)) are reviewed and Gate G4 in [06](06-roadmap-risks.md) passes.

Goal: let a coding agent improve Tile-Ripper by repeated, measured experiments **without** overfitting the benchmark, breaking pixel correctness, blowing the memory budget, or producing results nobody can reproduce.

Core idea: the agent proposes changes; a **harness the agent cannot edit** decides whether they are accepted. Acceptance = hard gates (correctness, memory, reproducibility, provider-safety) → then performance on workloads the agent has never seen.

---

## 1. Roles and trust boundaries

| Component | Who owns it | Agent may |
|---|---|---|
| Engine source (`tr-*` except harness) | agent edits via branches/worktrees | read, modify, propose |
| Tunables file (`tunables.toml`) | agent edits | change values within declared bounds |
| **Harness** (`tr-bench`, oracle, workload definitions, thresholds, statistics code) | human-owned, path-protected (CODEOWNERS + CI check that diffs touching it are rejected from agent branches) | read the *train* workload definitions and the harness API; **cannot modify** |
| Test (held-out) workloads | human-owned, **hidden** from the agent | nothing; returns only pass/fail + aggregate score at milestone gates |
| Experiment ledger | harness appends | read; cannot rewrite |

The harness runs experiments from a **pristine checkout of the candidate commit** in a clean container, never in the agent's working tree, so an agent cannot alter the measurement environment, the oracle, cached inputs, or timers.

---

## 2. The search space (what the agent may change)

Two tiers. Start with tier 1 only; tier 2 is for structural experiments proposed with a hypothesis.

### Tier 1 — declared tunables (`tunables.toml`; typed, bounded, with defaults)

| Parameter | Controls | Interacts with | Expected sensitivity `[INFER]` | Safe bounds / notes |
|---|---|---|---|---|
| `global.max_wire_inflight_bytes` | bytes in flight | memory budget, BDP | high when NIC ≫ default | ≤ memory budget × fraction |
| `host[h].conn_min/max` | per-host connection window | provider limits (CDSE ≤ 4), S3 503 SlowDown | high on cloud | **hard ceilings from provider facts**, never agent-editable above them |
| `host[h].controller` + AIMD params (`α`, `β`, latency-gradient threshold, sample window) | adaptive concurrency | RTT, tail latency | medium; robustness matters more than the best value | a self-tuning rule beats a tuned constant (§5) |
| `io.coalesce_gap_bytes`, `io.max_merged_span` | range coalescing | tile layout (8-byte tile gaps on ES; counter-evidence from cptkirk: coalescing can be slower) | medium/high; layout-dependent | gap ∈ {0 … 4 MiB}; merged span ≤ 16 MiB |
| `io.read_mode_theta` | ranged ↔ whole-object crossover | AOI fraction, object size | medium | derived from E5, not free |
| `io.warmup_connections` | pre-opened connections | TLS cost on HTTP/1.1 | low–medium (startup) | ≤ conn_max |
| `io.hedge.enabled/quantile/budget_pct` | tail hedging | p99, wire overhead | tail-only | budget ≤ 5%; must pass byte-overhead gate |
| `io.retry.*` | retry/backoff/budget | throttling | robustness | cannot disable `Retry-After` handling |
| `priority.policy` | order key weights (scene priority vs out-chunk rank vs band) | memory, early completion | medium | enumerated policies |
| `cache.layout_*`, `cache.tile_lru_bytes` | layout index & tile LRU | memory, reuse | low–medium | within memory budget |
| `decode.threads`, `decode.queue_depth` | CPU parallelism | NIC, cores | high when decode-bound | ≤ cores |
| `pipeline.channel_capacity.*`, `pipeline.memory_bytes` split | buffer sizes | backpressure, latency | medium | sum ≤ budget |
| `order.output` ∈ {CellMajorZ, TimeMajor, SourceTileMajor} | output chunk ordering | sink caps, pyramid state, header cost | medium/high | sink caps constrain legal values |
| `sink.*` (parallel writes, buffer, PUT concurrency) | write path | object-store write limits | medium for large outputs | provider limits |

### Tier 2 — code changes
Algorithmic changes (e.g. a new controller, a fused predictor kernel, a different admission rule, dedupe improvements). Each must be filed as a **hypothesis** in the ledger before implementation: what mechanism, what regime, what measurement would falsify it, and the expected effect size. Changes without a mechanism are rejected.

---

## 3. Workload splits (prevent overfitting)

Data and environments are split along every axis that a tuned constant could silently exploit.

| Split | Content | Visible to agent? | Use |
|---|---|---|---|
| **Train** | E-LAB frozen mirrors of W1/W3 subsets under *randomised* `netem` profiles (RTT 10–200 ms, jitter, loss 0–1%, bandwidth 0.5–25 Gbps, injected 429/503 patterns), plus 2–3 real-host AOIs | Yes | fast inner loop (minutes), deterministic request counts |
| **Validation** | Different AOIs, tiles, months, and **a different provider** (PC) than train; real hosts and E-CLOUD | Aggregate score only; AOI identities hidden | candidate selection, ≥ 10 interleaved runs vs incumbent |
| **Test (held-out)** | W5 (overlap), W6 (zone-straddle), another continent/climate (arid, boreal, tropical-cloudy), a different instance type, a different date range | **No** | milestone gates only, run by a human; result returned as pass/fail + aggregate |
| **Mutation set** | Train workloads transformed: AOI shifted by random offsets (changes block alignment), band order permuted, scene subsets, different resolutions | Generated at run time | catches constants tuned to a specific tile alignment/scene count |

Rules:
- **Rotation / burn-in:** a test set evaluated more than *k* times (suggest k = 3) is demoted to validation and a fresh test set is created. Test results never feed back to the agent beyond pass/fail.
- **Time split:** validation/test use dates after the training months, so provider-side cache state and reprocessing do not leak.
- **Environment split:** tune on one instance class, accept on another (and on E-WS) so a result tied to one NIC/core ratio is detected.
- **Network profile randomisation** in the lab means a candidate must be good across a *distribution* of RTT/bandwidth, not one measured network.

---

## 4. Acceptance protocol

A candidate commit is **accepted** only if it passes every stage in order. Any failure stops the pipeline and is logged.

**Stage A — hard gates (any failure = reject)**
1. **Pixel correctness** against the GDAL oracle on all correctness workloads, plus golden fixtures (`downsample_2x`, radiometry, SCL mask, PixelIsPoint, tiepoint), plus property tests (arrival-order invariance: three shuffled orders ⇒ identical output hash) and a fuzz corpus for the TIFF/JP2 parsers (no panics).
2. **Bounded memory:** peak RSS ≤ configured `memory_bytes × 1.1` on every run, including adversarial arrival orders; no unbounded queue (static + runtime check).
3. **Reproducibility:** three consecutive runs from a clean checkout give identical output checksums; plan hash stable; timing CV < 5% on the lab tier (else the experiment is noise-limited and is rejected as inconclusive).
4. **Provider safety:** zero sustained 429/503 (rate < 0.1%), per-host connection and request-rate ceilings never exceeded, `Retry-After` honoured, user-agent present. Violations are rejected regardless of speed.
5. **Wire discipline:** total wire bytes ≤ planned bytes × 1.05 (hedge/retry overhead cap); request count ≤ plan estimate × 1.1.
6. **No gaming checks:** no workload identifiers, AOI constants, hostnames or object keys in engine code (static grep + review); no reading outside the harness-provided inputs; no cross-run caching that survives between harness runs (the harness wipes it); timers and counters come from the harness/NIC, not from the engine's self-report.

**Stage B — performance on train (lab)**
Primary score = geometric mean over train workloads of `T_total_incumbent / T_total_candidate`, with secondary constraints: no train workload regresses > 3%; p99 completion not worse; CPU time per useful MB not worse by > 5% unless T_total improves ≥ 10%.

**Stage C — validation (real hosts, interleaved)**
≥ 10 interleaved runs per cell vs the incumbent, randomised order, ≥ 2 days. Bootstrap 95% CI of the ratio of medians must exclude 1 and the effect must be ≥ 10% (matches [04 §8](04-benchmarks.md)). Winner's-curse protection: if many candidates compete on the same validation fold, the top candidate gets a **confirmation run** on a fresh fold with new seeds before acceptance, and p-values/CIs are adjusted for the number of candidates tried (Benjamini–Hochberg or a simple Bonferroni on the final shortlist).

**Stage D — robustness**
Candidate must hold on the mutation set and on the *worst* of the randomised network profiles (no profile slower than the incumbent by > 5% unless the gain elsewhere is ≥ 25%).

**Stage E — milestone (human, test set)**
At each roadmap gate a human runs the hidden test set once. Result recorded; if the held-out gain is < 50% of the validation gain, the milestone is marked **overfit** and the last accepted batch is reviewed/reverted.

**Complexity penalty:** a change that adds a tunable, a code path, or > N lines must clear a higher bar (e.g. ≥ 15% on validation) than a simplification. Removing a tunable with no loss is a win and is scored as one. This implements "no speculative features" mechanically.

---

## 5. Making tuning robust instead of fitted

- Prefer **self-tuning controllers** (AIMD on observed latency/throughput) over tuned constants; the agent's job is the controller's *structure* and safe bounds, not a magic number for one network.
- Parameters are tuned on **distributions** (network profiles), and the reported effect is the median and worst-case across profiles.
- The agent receives **structured trace summaries** (per-stage utilisation, queue depths, request histograms, bytes by host, `wire_amp`) so it reasons from bottlenecks, not blind search. Every proposal names the bottleneck it targets (NIC-bound? connection-bound? decode-bound? memory-admission-bound?).
- **Ablation duty:** an accepted change must also be shown to *not* depend on others it was developed with (re-run with each earlier change removed); interactions are recorded.
- Defaults must be **safe without tuning** (a new user with default config gets within a stated bound of the tuned optimum).

---

## 6. Experiment ledger and reproducibility

Every experiment (accepted or not) appends an immutable record: hypothesis, bottleneck targeted, diff hash, config hash, environment manifest, split, raw run IDs, trace artefacts, gate outcomes, effect with CI, decision, reviewer. Negative results stay. The ledger is the dataset for the next round of proposals (and for auditing agent behaviour).

Reproducibility inputs, pinned per record: container image digest (or Nix lock), `Cargo.lock`, `uv.lock`, GDAL/PROJ versions, rustc and flags, frozen STAC snapshot + object ETags, netem profile seeds, harness commit. The same ledger format can sit in a ledger/SOP system such as dgov's if desired; it is not a dependency.

Budgets per agent session: maximum cloud spend, maximum real-host requests per hour, maximum experiments per hypothesis (to cap multiple-comparison inflation), wall-clock cap. Real-host runs are rate-capped by the harness, not by the agent.

---

## 7. What the agent is explicitly **not** allowed to do

- Edit the harness, oracle, workload definitions, thresholds, statistics code, or test set.
- Relax a hard gate, raise a provider ceiling, or add an authentication workaround.
- Cache pixels or layouts across harness runs, or ship precomputed per-workload artefacts.
- Optimise a metric the harness didn't define (e.g. internally reported time).
- Run unbounded loops against public hosts.

---

## 8. Staging of the loop

1. **Manual baseline round:** human-run E0/E2/E3 establish the floor, the tuned baseline, and the sensitivity of the main knobs. If most knobs are flat, the agent loop is not worth building.
2. **Lab-only agent loop:** tier-1 tunables on E-LAB with randomised networks; Stage A + B only.
3. **Validation on real hosts:** add Stage C/D; human reviews every accepted change for the first N.
4. **Tier-2 hypotheses:** structural changes with mechanism-first discipline.
5. **Milestone test-set runs** at each roadmap gate.

Gate G4 for entering this phase and the pass criteria for exiting it are in [06](06-roadmap-risks.md).
