# Acquisition architecture profile

The comparison baseline is local commit
`94c901493f8f6ec972728b1417e6557192095b1a`, rebuilt from `git archive HEAD` with
its unchanged lockfile and a separate target directory. Existing alpha preparation
is preserved. This milestone changes execution and consumption, not source
selection or the admitted format/decoder families.

| Stage | Committed baseline | Implemented path |
| --- | --- | --- |
| Discovery | Bounded STAC pagination; explicit source lists bypass discovery | Retained |
| COG identity/layout | ETag resolution and GDAL inspection, serial across assets | Retained; significant remaining latency for many small assets |
| Zarr layout/coordinates | Cached within a prepared source; each selected data key HEAD pinned serially | Same unique keys and identities; bounded HEAD batches across a store |
| Version checks | Metadata/coordinate/absence HEAD validation, serial | Same validations, bounded parallel batches; successful completion still requires post-validation |
| Data scheduling | Selected asset/variable loop; COG windows or Zarr source chunks read serially | Shared ordered queue of COG windows and Zarr source chunks, global worker and per-origin operation caps |
| Connections | GDAL owns COG transport; Zarr pooled client limited to one idle connection, request mutex held through retries | GDAL retained; HTTP Zarr shares one pooled client across stores, without store-wide request serialization |
| Decode | GDAL UInt16 and zarrs UInt16/Float32; checked native chunk/codec admission | Retained, with worker-owned reusable format reader contexts; no custom decoder |
| Output | Synchronous callback; GeoTIFF or bounded Zarr destination LRU | Same adapters consume deterministic plan order; fetching/decoding progresses on other workers while the sink runs |
| Native consumer | Synchronous fallible callback | Callback retained, plus bounded Rust iterator, provenance header, terminal errors/report, cancellation and joined shutdown |
| Repeated requests | Prepared metadata/version reuse; data reread | Retained; no persistent encoded/decoded data cache or cross-request overlap deduplication |

The scheduler core admits opaque operation costs and origins; format-specific
reader creation, native value types and physical operations remain explicit
private adapters. The logical planner, coordinates, physical chunk identities,
masking and stored-value semantics remain the existing authority.

## Measurement that changed the implementation

The initial four-worker candidate used a COG asset as one scheduling unit, which
retained one dataset open but blocked later assets at their first output chunk.
At 8 ms/request and 100 Mbit/s shared bandwidth, five repetitions gave:

| COG workload | Baseline wall median | Initial four-worker median |
| --- | ---: | ---: |
| 16 small intersections | 1.479 s | 0.982 s |
| Four large intersections | 3.714 s | 3.709 s |

Large-window baseline fused read/decode occupied 3.230 s of a 3.714 s process
wall median; planning was 0.155 s, source opens 0.094 s and sink calls 0.086 s.
In the first candidate, active-worker peak was four, but ordered acknowledgements
left most workers waiting behind the current asset. An active-worker count alone
therefore did not establish parallel throughput. Final scheduling uses each COG
read window as an operation and reuses the current dataset independently on each
worker. Output order remains unchanged. The accepted benchmark evaluates that
final implementation rather than promoting this exploratory timing.

Many-small baseline planning alone was 0.556 s of 1.479 s. COG identity/layout
inspection remains serial, so execution concurrency cannot eliminate that cost.
The recorded stage times include fused reads and overlapping worker totals;
they must not be added to synthesize process wall time.

The original Zarr path also held a per-store network mutex over requests and
retry sleeps and selected data HEADs serially. The final experiment compares
serial-worker control and four workers through both preparation and execution,
with independent HTTP counters separating data bytes, metadata and attempts.
[Exploratory numerical profile](initial-profile.json) and the final report retain
losses and bottlenecks alongside improvements.

## Resource and safety limits

Each in-flight operation owns an admission reservation until its output has
been acknowledged. A fixed shape-based reservation also covers worker-retained Zarr decoded
coordinate/dimension arrays. This estimates native buffers rather than proving
an allocator-level bound on every cloned selection/index object. Worker count shrinks when that state plus one
operation would exceed the requested working buffer. HTTP metadata and expanded
consolidated metadata use an additional shared atomic B/2 cache allowance;
Zarr file output retains its existing B/4 destination buffer. The stream queue
has an explicit chunk-count bound, default one. GDAL cache, library/codec internals,
plan/object bookkeeping, runtime memory and caller-retained values are separate;
these bounds are not a process RSS ceiling. RSS is independently measured.

Manual worker/per-origin overrides remain available. The defaults use up to four
available CPUs, no calibration requests, and admission based on actual native
chunk shape and pinned encoded size. The gate evaluates static settings; there
is no unvalidated throughput feedback controller or oscillating concurrency.

Per-origin limits bound explicit reader operations. GDAL can issue internal
range requests, so this is not a strict global count of HTTP subrequests across
both transports. Work is admitted in selected-source/node order, without a fairness scan. A long
earlier selection can occupy all slots and delay later assets. Ordered delivery
may also wait for an earlier slow read; later work progresses only within the
bounded in-flight window. Retries retain the
existing bounded backoff and source-version checks. Cancellation stops admission
and closes channels, then joins workers after in-flight native requests return.
A failed fetch removes its newly owned output directory. A rerun reads the full
selection; no persistent partial checkpoint protocol was introduced.
