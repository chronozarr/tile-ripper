# 03 — Proposed architecture (Deliverable 3)

Status: design sketch for review. No production code exists. Rust below is **interface sketches, not compiled**.
Evidence tags like `[N03]` point to `evidence/0X-*.md` (raw agent notes with URLs and pinned commits). Grades: `[SRC]` read the code/docs, `[BENCH]` reproducible benchmark, `[CLAIM]` assertion without method, `[INFER]` my inference.

---

## 1. Constraints the evidence puts on the design

| # | Constraint | Evidence |
|---|---|---|
| C1 | Do **not** build a new codec stack first. C beats Rust for deflate (libdeflate ~2× flate2/miniz_oxide), zstd (libzstd 1.4–3.5× ruzstd) and JP2 (OpenJPEG). async-tiff decode was ~1.7× slower than GDAL at 16 threads in the maintainers' own benchmark. | [N07 §3–4], [N01 §4] |
| C2 | Do **not** build a general warp first. No Rust crate does a GDAL-style warp (`rwarp` is 3.2k LOC, one author, 48 downloads). GDAL has no identity fast path. The part to build is the **zero-warp / integer-shift path**; everything else delegates to GDAL via `gdal-sys`. | [N07 §5–6] |
| C3 | Discovery is solved at 0.3–3 s per STAC page. No novelty budget there. | [N02 §1] |
| C4 | STAC carries `proj:shape`/`proj:transform` (exact pixel window) but **never** COG tile offsets/sizes. A byte-range plan needs one ~32–64 KiB header GET per asset (all IFDs fit in the first ~4 KB on Earth Search). | [N02 §2], [N08 §B.7] |
| C5 | The S2 hosts (S3, Azure Blob, CDSE) negotiate **HTTP/1.1**. Concurrency = TCP connections. HTTP/2 tuning does nothing there. | [N03 §3], [N04 §3] |
| C6 | Block layout differs by provider: Earth Search 1024² + Deflate + predictor 2; Planetary Computer 512² + Deflate, **no predictor, 15-bit packed**; CDSE/AWS-L2A/GCS are JP2. Request counts do not transfer between providers. async-tiff cannot read PC's 15-bit tiles. | [N05 §2], [N07 §1–2] |
| C7 | Radiometric offsets are a correctness trap (baseline ≥ 04.00, +1000 DN; flags wrong on some Earth Search legacy items; PC offset not in STAC). | [N05 §3] |
| C8 | No object-store library ships cross-object planning, adaptive per-host limits, or hedged range reads in general form. AWS's Rust transfer manager has real hedging code but is S3-only, whole-object/one-range, "not for production". | [N03 verdict], [N08 §B.6] |
| C9 | ChronoZarr v0.3.0 removed star-delta. Every (t, cell) chunk is independent; level 0 can arrive in any order and in parallel. Pyramid levels ≥ 1 are derived from level k-1 with an exact integer algorithm. | [N06 §1, §3] |
| C10 | The Python ChronoZarr encoder runs ~110–170 MB/s; fetch+warp is the dominant cost in the current ingest (6.7 s/month on a one-chunk chip). The sink is not the bottleneck. | [N06 §2] |
| C11 | `zarrs` is the only credible Rust Zarr v3 crate: bus factor 1, 28–43 breaking-change bullets per minor, no bounded-memory shard writer, full-shard writes are in-memory. | [N06 §4–5] |
| C12 | Tokio docs say CPU-bound work does not belong on async workers; use a separate rayon pool. | [N07 §8] |

Consequence: **the engine's job is orchestration** — planning, scheduling, memory-bounded flow, sink coupling, observability — with best-in-class C libraries behind narrow interfaces where Rust is not faster.

---

## 2. Stage separation

The brief asks for six things to be kept distinct. Each is a stage with its own trait, its own resource, and a bounded queue to the next.

| Stage | Question it answers | Touches | Scarce resource | Cacheable? | Crate |
|---|---|---|---|---|---|
| 1 Metadata discovery | Which scenes/assets exist for (AOI, time, collection)? | STAC API / GeoParquet | API rate limits, RTT | Yes (snapshot) | `tr-discovery` |
| 2 Layout resolution | Where are the bytes? (IFD/tile offsets, dtype, nodata) | One header GET per asset | RTT, connections | **Yes, forever per (href, etag)** | `tr-layout` |
| 3 Transfer planning | Exactly which byte ranges, in what order, with what priority? | Nothing (pure CPU) | CPU, memory | Plan is a value (hash, replay) | `tr-planner` |
| 4 Remote I/O | Fetch ranges | HTTP(S) | NIC, connections, provider quotas | Range cache (optional) | `tr-io` |
| 5 Decompression | Wire bytes → pixels | Memory | CPU cores, memory bandwidth | No | `tr-decode` |
| 6 Geospatial transform | Pixels → target grid; mosaic; temporal reduce | Memory | CPU, memory | No | `tr-transform` |
| 7 Output writing | Chunks → sink | Disk / object store / pipe | Disk or NIC, sink ordering rules | No | `tr-sink-*` |

Rules:
1. Stages 1–3 are **pure functions of their inputs plus declared I/O**. The plan (3) is serialisable, hashable and replayable. That is what makes dry-run cost estimates, simulation, and benchmark replay possible.
2. Stage 4 is the only code that touches the network. Stages 5–6 never block on I/O. Stage 7 is the only code that writes.
3. Memory is a budgeted resource shared by stages 4–7 (§9).
4. No stage knows about a concrete format except through `ChunkedFormat` (§5.3).

---

## 3. Crate map

```
tr-core          types only: Aoi, TimeSpec, GridSpec, AssetRef, Layout, Plan, errors, Budgets
  ├─ tr-discovery    Catalog trait; StacApi, GeoParquet adapters; provider normalisers (ES v1/v2, PC, CDSE)
  ├─ tr-layout       LayoutResolver; LayoutCache (mem + on-disk Parquet, key = href+etag); header fetch policy
  ├─ tr-format       ChunkedFormat trait; cog/ (TIFF), zarr/, jp2/ (feature-gated)
  ├─ tr-planner      Planner; GridMapper selection; read-group coalescing; ordering; explain()
  ├─ tr-io           Transport (object_store + custom HttpConnector); Scheduler; controllers; hedging; auth refresh; trace
  ├─ tr-decode       DecodePool (rayon); codecs (libdeflate, zstd, lzw, openjpeg); fused predictor/unpack; BufferPool
  ├─ tr-transform    GridMapper impls (Identity, IntegerShift, Scale, Warp→gdal-sys); Resampling; Mosaic/Temporal accumulators
  ├─ tr-sink         Sink trait + local/obj-store Zarr, GeoTIFF dir, Arrow-IPC/NPY stream; tr-sink-chronozarr
  ├─ tr-engine       wires stages with bounded channels + MemoryBudget; cancellation; run manifest
  └─ tr-bench        harness binary: workloads, oracles, trace analysis (shared by humans and agents, see 04/05)
```

Dependency rule: arrows only point toward `tr-core`; `tr-engine` and `tr-bench` are the only crates that see everything. Planner never imports `tr-io`; decoder never imports `tr-planner`.

### 3.1 Adopt / wrap / avoid (decision table)

| Need | Decision | Why (evidence) |
|---|---|---|
| STAC types + API client | **Adopt** `stac` / `stac-io`, parse items as raw JSON maps; own the normaliser | Typed extensions are version-pinned; paging is serial, 0.x, bus factor 1 [N02 §4] |
| Cached index | **Adopt** stac-geoparquet *spec*; hand-written DuckDB/Arrow SQL with `ST_Intersects_Extent`/bbox-struct predicates | `stac-duckdb` does not use bbox covering columns; DuckDB 1.5.x does not prune on plain `ST_Intersects` [N02 §5] |
| Object access + auth | **Wrap** `object_store` 0.14 with a **custom `HttpConnector`** (one shared client, DNS cache, h1/h2 per host, instrumented) | Per-store clients, no metrics, no shared pool by default; SAS/anonymous/requester-pays handled [N03] |
| Scheduler | **Build** above the `ObjectStore` trait | Nothing shipped is adaptive/hedged/cross-object [N03, N08] |
| Alternatives | **Avoid** OpenDAL, AWS Transfer Manager, CRT/Mountpoint, obstore as I/O layer | S2 S3 object not S3-Express; static SAS only; TM is S3-whole-object; obstore is a thin wrapper [N03] |
| TIFF header + tile index | **Evaluate**: async-tiff (contribute upstream) vs. a ~500-line own parser for the COG subset | async-tiff panics via `.expect()`, tiled-only, 32 KiB prefetch cache, pre-1.0 breaking minors, single maintainer; 15-bit gap; open planar+predictor bug #346 [N01 §3] |
| Deflate | **Wrap** `libdeflater` (one-shot) | ~2× flate2/miniz_oxide; avoid flate2+zlib-rs `default-features=false` (loses CPU runtime detection) [N07 §3] |
| Predictor + 15-bit unpack + crop | **Build** (fused SIMD pass) | Scalar in libtiff; nobody ships it in Rust [N07 §3] |
| zstd, LZW | **Wrap** `zstd`, `weezl` | C wins on zstd [N07] |
| JP2 | **Wrap** OpenJPEG (`jpeg2k`/`openjpeg-sys` ≥ 2.5.3) behind a feature; build TLM range front-end | Only TLM-aware, threaded, permissive option; `hayro-jpeg2000` is whole-image, f32 output, bit-exactness bug fixed only 2026-07 [N07 §4] |
| CRS transform | **Adopt** `proj` (one instance per rayon worker); revisit `proj4rs` after a parity corpus | `proj` is `!Send`; `proj4rs` 0.2.1 released the day of the audit [N07 §7] |
| Warp (true reprojection) | **Delegate** to GDAL (`gdal-sys` `GDALWarpRegionToBuffer`) behind `GridMapper::Warp` | No Rust warp exists; GDAL is the reference [N07 §5–6] |
| Zero-warp/paste/mosaic | **Build** | Port the `odc.geo` `_can_paste` / `compute_reproject_roi` rule (integer shift + integer scale, 0.05 px tolerance) [N07 §6] |
| Zarr sink | **Adopt `zarrs`, pinned exact version, behind one `put_chunk` boundary**; Phase 2 may skip it entirely by streaming frames to the Python `chronozarr` encoder | Bus factor, churn, fsync cost on 0.3.13, no bounded shard writer [N06 §5–6] |
| COG output | **Defer** | `geotiff-writer` 0.8.1 is young; `gdal` crate is the pragmatic fallback [N06 §3] |

---

## 4. Core data model (sketch)

```rust
// ---- tr-core ----------------------------------------------------------
pub struct AcquisitionRequest {
    pub aoi: Aoi,                       // one geometry or many (chips); with CRS
    pub time: TimeSelection,            // interval | explicit instants
    pub sampling: TimeSampling,         // All | EveryNth(n) | PerPeriod{period, pick}
    pub collection: CollectionId,
    pub bands: Vec<BandName>,           // plus implicit mask band (e.g. SCL) if policy needs it
    pub grid: TargetGrid,
    pub resampling: Resampling,         // Nearest | Bilinear | Cubic | Average | Mode (non-Nearest => GDAL path)
    pub mosaic: MosaicPolicy,           // FirstValid{order} | Min | Max | Mean | Median | Count
    pub temporal: TemporalPolicy,       // PerScene | GroupBy{day|solar_day|period} + reduce; carry-forward optional
    pub masks: MaskPolicy,              // nodata | SCL classes | none
    pub radiometry: RadiometryPolicy,   // Raw | ApplyScaleOffset | HarmonizeBaseline  (explicit; never guessed)
    pub budgets: Budgets,
    pub read_mode: ReadMode,            // Selective | WholeObject | Auto{theta}
}

pub enum TargetGrid {
    /// First-class mode: output is the source grid of the scene's own UTM zone. No resampling by construction.
    Native { zone_policy: ZonePolicy },         // PerZone | PrimaryZone(EpsgCode) | Majority
    Explicit(GridSpec),                          // crs + affine + shape (reprojection path)
}

pub struct Budgets {
    pub memory_bytes: u64,              // hard cap, enforced by MemoryBudget (§9)
    pub max_wire_inflight_bytes: u64,
    pub host_limits: HashMap<HostKey, HostLimits>,   // min/max connections, req/s, optional byte/s ceilings
    pub deadline: Option<Duration>,
}

// ---- tr-discovery output ---------------------------------------------
pub struct SceneRecord {
    pub id: SceneId,
    pub datetime: DateTime<Utc>,
    pub footprint: Polygon<f64>,
    pub crs: Crs,                       // normalised (proj:epsg | proj:code)
    pub tile: Option<TileId>,           // normalised (s2:mgrs_tile | grid:code)
    pub cloud_cover: Option<f32>,
    pub provenance: Provenance,         // catalog, query hash, fetched_at, snapshot id
    pub assets: BTreeMap<BandName, AssetRef>,
}

pub struct AssetRef {
    pub object: ObjectRef,              // host, path, scheme, AuthRef
    pub media: MediaKind,               // CogDeflate | CogOther | Jp2 | Zarr
    pub size: Option<u64>,              // file:size where present
    pub etag: Option<String>,
    pub grid: Option<AssetGrid>,        // proj:shape + proj:transform from STAC (no I/O needed)
    pub radiometry: Radiometry,         // dtype, nodata, scale, offset, offset_already_applied (provider-normalised)
}

// ---- tr-layout output -------------------------------------------------
pub struct AssetLayout {
    pub key: LayoutKey,                 // href + etag|size  (COGs are immutable => cache forever)
    pub dtype: DType, pub bands: u16, pub nodata: Option<f64>,
    pub geo: GeoRef,                    // incl. PixelIsPoint handling and raster tiepoint (I,J)
    pub codec: Codec,                   // Deflate{predictor}, Zstd, Lzw, Jp2{..}, ...; bit depth (15-bit packing)
    pub levels: Vec<LevelLayout>,       // full-res first, overviews after
}
pub struct LevelLayout {
    pub shape: (u32, u32), pub transform: Affine, pub chunk: (u32, u32),
    pub index: ChunkIndex,
}
pub enum ChunkIndex {
    Dense { offsets: Arc<[u64]>, lengths: Arc<[u32]> },   // TIFF TileOffsets/TileByteCounts
    Jp2 { tlm: Arc<[Jp2TilePart]> },                      // TLM-derived tile-part spans
    Keyed { template: KeyTemplate, shard: Option<ShardIndexRef> },  // Zarr
}
```

---

## 5. Interfaces between modules

### 5.1 Discovery → Layout/Plan

```rust
pub trait Catalog: Send + Sync {
    /// Metadata only. No pixel I/O. Streams normalised records; paging is concurrent by time window.
    fn search(&self, q: &SceneQuery) -> BoxStream<'_, Result<SceneRecord, DiscoveryError>>;
    fn snapshot_id(&self) -> Option<SnapshotId>;   // enables frozen-input benchmarks
}
```
Adapters: `StacApi` (Earth Search v1/v2, PC, CDSE), `GeoParquet` (local/remote; bbox-struct predicate; optional), `StaticItems` (test fixtures / frozen benchmark snapshots). Provider quirks (offset flags, `_0_/_1_` duplicate items, `grid:code` vs `s2:mgrs_tile`, STAC 1.0 vs 1.1) are resolved **here**, once, into `AssetRef.radiometry`, and nowhere else.

### 5.2 Layout resolution

```rust
pub trait LayoutResolver: Send + Sync {
    /// May do one speculative ~64 KiB GET, escalating only if the IFD chain extends past it.
    /// Single-flight per LayoutKey; hit => zero network.
    async fn resolve(&self, asset: &AssetRef) -> Result<Arc<AssetLayout>, LayoutError>;
    /// Batch form lets the scheduler pipeline header GETs for upcoming assets behind pixel GETs for current ones.
    fn resolve_many<'a>(&'a self, assets: &'a [AssetRef]) -> BoxStream<'a, (AssetIdx, Result<Arc<AssetLayout>, LayoutError>)>;
}
```
`LayoutCache` persists to a Parquet file per collection/region (rasteret's schema is useful prior art [N02 §3]). Because the key includes the ETag/size, a stale entry is detected, not trusted.

### 5.3 Format plug-in (the only place a format is known)

```rust
pub trait ChunkedFormat: Send + Sync {
    fn media(&self) -> MediaKind;
    /// Pure: which chunks of `level` intersect `window`, and their byte spans.
    fn plan_chunks(&self, l: &AssetLayout, level: usize, window: PixelWindow) -> SmallVec<[ChunkRef; 8]>;
    fn span(&self, l: &AssetLayout, c: ChunkRef) -> ByteSpan;
    /// Pure CPU. Decode into a caller-provided pooled buffer (no per-tile Vec allocation).
    fn decode(&self, l: &AssetLayout, c: ChunkRef, wire: &[u8], out: &mut TileBuf) -> Result<DecodeStats, DecodeError>;
    /// Cheap overview choice for a target resolution (never decodes).
    fn pick_level(&self, l: &AssetLayout, target_res: f64) -> usize;
}
```
Initial impls: `Cog` (deflate/zstd/lzw, predictor 1/2/3, 8/15/16-bit, planar/chunky), `Jp2` (OpenJPEG, TLM), later `Zarr` (EOPF, sharded). A future HDF5/NetCDF byte-reference format fits the same shape (layout = chunk index; decode = codec chain).

### 5.4 Planner

```rust
pub trait Planner {
    /// Phase A: which assets need a layout at all (STAC proj window prefilter, no I/O).
    fn layouts_needed(&self, req: &AcquisitionRequest, scenes: &[SceneRecord]) -> Vec<AssetIdx>;
    /// Phase B: exact plan once layouts are known.
    fn plan(&self, req: &AcquisitionRequest, scenes: &[SceneRecord], layouts: &LayoutSet)
        -> Result<AcquisitionPlan, PlanError>;
}

pub struct AcquisitionPlan {
    pub out: OutputGrid,                 // grid(s), chunking, dtype, nodata
    pub out_chunks: Vec<OutChunk>,       // unit of sink writes, in sink-preferred order
    pub tasks: Vec<ChunkTask>,           // unit of transfer+decode
    pub reads: Vec<ReadGroup>,           // coalesced wire requests, per object
    pub estimate: Estimate,              // wire_bytes, decoded_bytes, requests, per-host load, ideal_wire_bytes
    pub hash: PlanHash,                  // deterministic given (request, scene snapshot, layouts, planner version)
}
pub struct ChunkTask {
    pub id: TaskId, pub asset: AssetIdx, pub level: u8, pub chunk: ChunkRef,
    pub span: ByteSpan, pub src_window: PixelWindow, pub dst: DstRect,
    pub out_chunk: OutChunkId, pub order_key: OrderKey,   // (out-chunk rank, scene priority, band)
    pub cancel_if: Option<CancelRule>,                    // e.g. skip optical tiles if SCL shows no valid px (policy-safe only)
}
pub struct ReadGroup { pub object: ObjectId, pub spans: Vec<ByteSpan>, pub tasks: Vec<TaskId>, pub mode: ReadKind } // Ranged | WholeObject
impl AcquisitionPlan { pub fn explain(&self) -> PlanReport; }   // dry-run: requests/bytes/host load/amplification, no I/O
```

### 5.5 Transfer scheduler (the only network-facing component)

```rust
pub trait RangeSource: Send + Sync {                       // implemented over object_store
    async fn get_ranges(&self, obj: &ObjectRef, spans: &[ByteSpan], ctx: &ReqCtx)
        -> Result<Vec<Bytes>, IoError>;
    async fn get_whole(&self, obj: &ObjectRef, ctx: &ReqCtx) -> Result<ByteStream, IoError>;
}

pub struct Scheduler { /* priority queue, per-host controllers, memory permits, hedger, retry budget, tracer */ }
impl Scheduler {
    /// Admission is by plan order (OrderKey); results stream back as they complete.
    pub fn run(&self, groups: Vec<ReadGroup>, budget: Arc<MemoryBudget>) -> BoxStream<'static, Result<Fetched, IoError>>;
}

pub trait ConcurrencyController: Send {                    // one per host (or host class)
    fn limit(&self) -> usize;
    fn on_sample(&mut self, s: Sample);                     // Sample { latency, queue_delay, bytes, status, retried }
}
// Impls: Fixed(n), Aimd{..}, Gradient{..}. The *tunable surface* for Phase 5 lives here (see 05).
```

### 5.6 Decode, transform, sink

```rust
pub struct DecodePool { /* rayon, dedicated; sized to cores; separate from the tokio I/O runtime */ }
impl DecodePool { pub fn submit(&self, job: DecodeJob) -> oneshot::Receiver<Result<DecodedTile, DecodeError>>; }

pub trait GridMapper: Send + Sync {
    fn kind(&self) -> MapKind;                              // Identity | IntegerShift | Scale | Warp
    fn src_window(&self, dst: DstRect) -> PixelWindow;      // conservative, validated (not 4-corner only: see N01 §2 edge-risk note)
    fn apply(&self, src: &TileView, dst: &mut TileViewMut, masks: &Masks, rs: Resampling) -> Result<()>;
}
pub trait Accumulator: Send {                               // per out-chunk, order-independent result
    fn push(&mut self, key: OrderKey, tile: &TileView, valid: &Mask);
    fn finish(self: Box<Self>) -> OutBuf;                   // deterministic regardless of arrival order
}

pub trait Sink: Send {
    fn caps(&self) -> SinkCaps;          // out_of_order, parallel_writes, needs_time_order, preferred_order
    async fn begin(&mut self, schema: &SinkSchema) -> Result<()>;
    async fn write(&mut self, chunk: OutChunk, data: OutBuf) -> Result<WriteAck>;   // `await` is the backpressure signal
    async fn finish(self: Box<Self>) -> Result<Manifest>;
}
```
`Sink::caps().preferred_order` feeds back into the planner: it chooses `OutChunk` order (CellMajor-Z / TimeMajor / SourceTileMajor) so that the *sink's* constraint, not an incidental loop order, bounds memory.

---

## 6. Native-grid extraction as a first-class mode

`TargetGrid::Native` means: output pixels are the source pixels of the scene's own UTM zone grid. Consequences:

1. `GridMapper` is `Identity` (same scene) or `IntegerShift` (mosaicking adjacent MGRS tiles of one UTM zone). Pure windowed copy; **no resampling code runs**; bit-exactness vs `rasterio` window read is the correctness test. GDAL has no such path [N07 §6], so this is genuinely new work, but small (~1–2 weeks, [INFER]).
2. All tiles within one UTM zone should share a 10 m lattice, so shifts are integers [INFER from MGRS definition; the planner must *verify* using the `_can_paste` tolerance (0.05 px) and fall back to `Warp`, never assume].
3. Multi-resolution bands: 20 m → 10 m is `Scale` with `Nearest` (integer factor) — also a pure index map. Bilinear/cubic route to GDAL.
4. AOI straddling a UTM zone boundary: `ZonePolicy::PerZone` emits one native output per zone (lossless); `PrimaryZone(epsg)` warps the minority zone (the only case where "native" needs `Warp`). The request says which; the engine never silently picks.
5. Radiometry stays explicit: `Raw` returns stored DN exactly; harmonisation is a separate, declared transform (C7).

---

## 7. The global read planner

### 7.1 What "global" means here

Per-asset/per-chunk scheduling (GDAL, odc-stac, stackstac, lazycogs, Rasteret) makes each asset or chunk responsible for its own I/O, with one outer cap (threads, or a semaphore: 32 in lazycogs, 150 in Rasteret) [N01 §2, N08 §A.1]. The global planner instead:

1. **Prefilters** scenes by STAC footprint + `proj:shape/transform` → exact pixel windows with zero I/O.
2. **Resolves layouts in one batch**, deduped by `LayoutKey`, pipelined ahead of pixel reads (hides the 1–3 dependent header RTTs per asset that per-asset schedulers pay serially) [N08 §B.4].
3. **Maps every output chunk to source chunks**, then **dedupes** `(object, chunk)` across output chunks, overlapping scene strips (adjacent MGRS tiles overlap ~9.8 km) and duplicate items. Only sentle does the overlap dedupe today (synthetic 408→94 requests) [N08 §A.1].
4. **Coalesces per object** with a gap derived from measurement: `gap* ≈ b_conn × t_firstbyte`, then capped by a max merged-span size (so hedging and pipelining stay possible). Note consecutive S2 COG tiles are separated by exactly 8 bytes, so a gap-0 coalescer never merges them, while `object_store`'s 1 MiB default does, and Rasteret uses 1 KiB [N08 §B.7, N03]. Counter-evidence: cptkirk measured coalescing *slower* on a planar 64-band layout (66 s vs 48 s) [CLAIM] → gap is a tunable, and "no coalescing" is a valid plan.
5. **Orders** reads by `OrderKey` so out-chunks complete early and memory stays bounded (§9), not by object or scene.
6. **Holds a global queue** drained by per-host adaptive controllers (the only genuinely global resource is memory + NIC; host limits are per-host).
7. **Plans conditional cancellation**: SCL (20 m, small) first; when the mosaic policy makes it semantically safe (FirstValid, composites with masks), cancel optical tasks for tiles with zero valid pixels. This is a *byte-reduction* lever independent of scheduling, and no incumbent does QA-first skipping [N04 §top-5, item 5].
8. **Chooses `ReadMode` per object**: ranged vs whole-object GET when estimated coverage exceeds θ (θ measured, §04).

### 7.2 Does it beat per-asset scheduling? Honest answer: plausible, bounded, unproven head-to-head

Direct raster evidence is partial and none of it isolates scheduling: Rasteret's baselines are sequential or thread-pooled rasterio; sentle's gain is dedupe; Kayrros' 110→3 requests is header indexing for JP2 [N08 §B.1–3]. No tool hedges or adapts concurrency [N08 grep result]. The rest is analogy (DuckDB async I/O 2.9–3.7× on S3, Lance scheduler, Arrow range cache, AnyBlob) where the common pattern is a **global queue/priority/backpressure with per-object coalescing**.

Model (from [N08 §B.4]):

```
T ≈ T_startup + T_plan + max( B / min(NIC, C·b_conn),
                              (R_hdr·t_hdr + R·t_req) / C,
                              D_raw / (P · r_dec) )  + T_tail
Concurrency to saturate a link (Little's law):  C* = (NIC/b_conn) · (1 + b0·b_conn/s)
   b0 ≈ 30 ms in-region S3 first byte; b_conn ≈ 50–60 MiB/s median (≈0.4 Gbps, the figure AWS CRT also assumes)
   → ~200–250 connections to fill 100 Gbps; ~25 for 10 Gbps with 8–16 MiB ranges; far more with 1.3 MB ranges
```
Regimes (workload numbers from [N08 §B.7], one scene, modelled):

| Regime | Dominant term | Can a global planner help? |
|---|---|---|
| Laptop, ~1 Gbps | bytes/NIC | **No.** Bytes-bound (~15.6 s for the 1.8 GiB large-AOI case). Only byte reduction (QA-first, overviews, dedupe) helps. |
| Cloud, 10 Gbps, small AOI × many scenes (1 km chip, 1600 tiles ≈ 2 GiB read for ≈ 32 MB useful, ~67× over-read) | bytes at C ≥ 64 | **Little.** Header cache ≈ +20% at C=64, 1.5× at C=16. Nothing to coalesce. The lever is over-read (tile granularity), not scheduling. |
| Cloud, 10 Gbps, large AOI × few scenes (1440 tiles, 1.8 GiB) | bytes 1.56 s, decode 1.05 s | Coalescing buys 0% at C=64; **1.8× at C=16**. Becomes decode-bound at ≥ 25 Gbps. |
| Cloud ≥ 25–100 Gbps | decode + connection count | Planner matters for keeping 200+ connections full and cores fed (per-tile requests at C ≥ C* already saturate). Decode (C libs) dominates; a bounded pipeline matters. |
| Any host with origin throttling (CDSE: 4 conn × 20 MB/s, 2000 req/min) | provider quota | **No.** Quota sets the ceiling [N05 §5]. |
| Tail latency (≥ 1000 requests, a few slow) | T_tail | **Yes.** Nobody hedges raster reads; AWS guidance is to retry the slowest ~5% on a new connection [N03]. |

Conclusion: the planner's wins are **(a) under-concurrent baselines**, (b) header round trips, (c) dedupe, (d) hedged tails, (e) bounded memory with early-completing chunks, (f) byte reduction. Several of (a)–(c) can be obtained in a tuned Python/GDAL stack too; what is structural to a Rust async engine is cheap, very high concurrency (hundreds of in-flight requests without a thread each — GDAL's curl state is thread-local, one multi handle per thread [N04 §5]) and bounded-memory streaming into a sink.

### 7.3 Experiments that settle it (Phase 0/1, defined in 04)

- **E1 Header elimination:** warm vs cold layout cache, same reader.
- **E2 Concurrency curve:** throughput vs C ∈ {1…512} per host and range size {16 KiB … 8 MiB} on each provider and environment (finds b_conn, C*).
- **E3 Scheduling isolation:** *same reader, same decoder, same plan*; vary only the policy — (A) per-asset pools of 10 with 4–64 asset workers, (B) one global queue + adaptive controller. This isolates scheduling from language and codec, which no published benchmark does.
- **E4 Dedupe/over-read:** tile-aligned vs naive output ordering; coalescing gap sweep {0, 1 KiB, 64 KiB, 1 MiB, 4 MiB}.
- **E5 Ranged vs whole-object crossover:** sweep AOI fraction 1%→100% of a tile.

---

## 8. Scheduler details

- **Transport**: `object_store` 0.14 `ObjectStore` per (scheme, host, credential scope) but **one** shared `HttpClient` via a custom `HttpConnector` (own DNS cache, h1/h2 per host, pool sizing, the `HttpService` wrapper as the instrumentation seam) [N03]. `LimitStore` is not used (its permit covers a whole `get_ranges` call of up to 10 requests).
- **Adaptive concurrency**: per-host `ConcurrencyController`. Start `Fixed` as the control arm; implement `Aimd` on (latency gradient, 429/503/timeouts) with floor/ceiling from provider facts (CDSE ceiling 4 connections, 2000 req/min; S3 5500 GET/s/prefix; PC unpublished). Ship `Gradient` only if AIMD measurably loses (no speculative features).
- **Connection warm-up**: open N connections during discovery (TLS handshakes are on the critical path with HTTP/1.1).
- **Hedging**: after `p95(latency by size class)`, once per request, from a token bucket (default ≈ 5%, mirroring the AWS TM design [N03]); the hedge goes on a **new connection** (AWS retry guidance). Off by default until E-series shows a p99 win without > 5% byte overhead.
- **Retries**: classify (5xx, 429, 408, transport) with a retry budget and `Retry-After` honoured (`object_store`'s retry has neither a budget nor `Retry-After`) [N03].
- **Auth**: single-flight credential refresh; PC SAS tokens ≈ 45 min and signing endpoint 429s ("Try again in 23 seconds") → one token per container, cached, refreshed early; CDSE OIDC 10 min/refresh 60 [N05 §5].
- **Whole-object mode**: stream sequentially; decode tiles as bytes arrive; retain only intersecting tiles.
- **Etiquette**: configured max per-host connections, identifying `User-Agent`, global kill switch on sustained 429/503. The aggressive-by-default behaviour of a high-concurrency client against free public hosts is a risk in itself (see 06).
- **Tracing**: every request emits `{plan_task_ids, host, method, range, size, t_queue, t_connect, t_first_byte, t_body, status, retry#, hedge?}` to a columnar trace. This is the substrate for benchmarking and for the agent loop (05).

---

## 9. Bounded memory, backpressure, runtimes

- **`MemoryBudget`**: a byte-permit pool shared by (a) wire buffers in flight + queued, (b) decoded tiles, (c) accumulators, (d) sink write queue.
- **Deadlock rule**: admit work by *out-chunk working set*. An out-chunk is admitted only when the budget can hold the full set of source tiles it still needs (minus those already resident). Permits are acquired in pipeline order and released at the next stage's completion. At least one out-chunk's working set is always reserved so progress is guaranteed.
- **Channels** between stages are bounded; `Sink::write(...).await` is the terminal backpressure signal, propagating back to read admission (network never runs far ahead of decode).
- **Runtimes**: one multi-thread tokio runtime for I/O + control (few threads), one **dedicated rayon pool for decode/transform** (≈ cores). Never run CPU-bound decode on tokio workers [N07 §8]. Disable GDAL/OpenJPEG/`NUM_THREADS` internal pools (set to 1) when the engine owns parallelism. One `Proj` per rayon worker.
- **Buffers**: flat pooled `Vec<u16>`/slices with explicit stride; `ndarray` only at API edges. async-tiff's decode returns an owned `Vec<u8>` with a cast/copy (#234, #203) — decode-into-caller-buffer is a required interface (§5.3) and a candidate upstream contribution [N01 §3].
- **SIMD**: `std::simd` is unstable; use `wide`/`pulp` for the fused predictor+unpack+crop pass [N07 §8].

---

## 10. Sinks and the ChronoZarr adapter

Contract derived from the local spec (v0.3.0, 2026-10-03) and writer [N06 §3]:

| Item | Requirement |
|---|---|
| Unit of work | One `(t, level-0 cell r,c)` array `[n_band, ≤512, ≤512]`, dtype uint8/uint16/int16/float32; edge chunks padded to 512×512 with the fill value; optional uint8 mask/coverage planes |
| Input grid | All bands co-registered on one north-up EPSG grid, origin/pixel size fixed at creation |
| Level 0 | Chunks may arrive **in any order and in parallel** (no temporal encoding any more) |
| Levels ≥ 1 | Recursive from stored level k-1: valid-pixel mean, uint32 sums, floor division, edge replication; mask any-valid; coverage `(sum+2)//4`. A Rust port must reproduce `chronozarr._writer.downsample_2x` **bit-exactly** (golden fixtures required) |
| Pyramid state | A level-k cell needs its four level-(k-1) siblings for the same `t`: Z-order / row-major arrival keeps state small; random arrival can be unbounded → `preferred_order = CellMajorZ` |
| Time axis | `n_time` and the `time` coordinate chunk fixed at creation; append needs strictly later dates + a resize + root-metadata rewrite (local, ordered). Carry-forward gap fill is serial in `t` (engine concern; state = one prior frame per active cell in cell-major order) |
| Known spec/impl gaps | `coverage` plane: spec says valid-observation count, `ingest.py` writes 0/1; §8.3 working-copy rule vs streaming to the final bucket is ambiguous [N06 contradictions] |

**Phase-2 recommendation (no premature abstraction):** do **not** write a Rust ChronoZarr sink first. Stream level-0 frames/cells from the engine over Arrow IPC or `.npy` to the existing Python `chronozarr` encoder (110–170 MB/s, `zarrs-python` is a config-line accelerator [N06 §2]). Write the Rust sink only if Phase-2 measurements show the encoder is the bottleneck. When it is written: `zarrs` pinned to an exact version behind one `put_chunk` boundary; cross-validated against `chronozarr validate` and `zarrita` in CI; unsharded default (R2/S3 ranges on big shards stall CDNs; `zarrs` full-shard writes are in-memory and no `set_partial` on object stores [N06]).

Other sinks, same trait: local Zarr/GeoTIFF directory, Arrow IPC stream (stdout/Unix socket), `.npy`, `object_store` multipart (≥ 5 MiB parts, `WriteMultipart` backpressure) for shards/COG/Parquet. Icechunk is a staging option only (non-plain keys break the static-host premise) [N06].

---

## 11. Determinism and observability

- Same (request, scene snapshot, layouts, planner version, config) ⇒ same `PlanHash` ⇒ same output bytes (mosaic ties broken by `(datetime, scene id)`, never arrival order).
- `explain()` produces request counts, per-host load, ideal vs planned wire bytes, amplification factors, without I/O.
- Run manifest records tool versions, config, plan hash, trace file hash, output checksums.

## 12. Deliberate non-goals (per "no speculative features")

No in-tree warp implementation; no new JP2 decoder; no GPU path; no distributed multi-node mode; no COG writer; no Icechunk sink; no server/API; no caching layer for pixels beyond the layout index and an optional tile LRU inside one run. Each is revisited only if a gate in [06-roadmap-risks.md](06-roadmap-risks.md) shows it is the bottleneck.
