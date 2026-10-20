# Native streaming and shared acquisition execution

Build from `codex/acquisition-planner`; this remains an unreleased development
milestone. Existing alpha preparation and licensing documents are unchanged.

```sh
cargo build --release --locked --bins --examples
# Existing JSON source/selection schema, no output files created by stream:
./target/release/tile-ripper stream request.json --workers 4 --per-host 4
cargo run --release --locked --example chunk_stream -- request.json
```

The CLI emits one NDJSON descriptor per native chunk, followed by an explicit
`type=complete` report. Descriptors carry the observation, variable, native grid,
window, dtype, scaling and nodata metadata. They are a working diagnostic
consumer; pixel values are delivered directly by the Rust API below.

```rust,no_run
use tile_ripper::{acquisition, cog::LayoutCache, Request, Result, stream::StreamOptions};
fn consume(request: &Request) -> Result<()> {
    let prepared = acquisition::prepare(request, &mut LayoutCache::default())?;
    let mut stream = prepared.into_stream(StreamOptions { capacity: 1 })?;
    // stream.provenance contains sanitized source references and pinned versions.
    for chunk in stream.by_ref() {
        let chunk = chunk?;
        // Pixels::UInt16(Vec<u16>) or Pixels::Float32(Vec<f32>), never calibrated.
        // chunk.validity(), chunk.pixel_center(column, row), datetime, indices,
        // variable and asset_id preserve masks, grid, time and source identity.
        assert_eq!(chunk.pixels.len(), chunk.window.width * chunk.window.height);
    }
    let report = stream.finish()?;
    assert!(report.scheduler["completed_tasks"].is_number());
    Ok(())
}
```

Chunks are provisional until successful `finish()`: post-execution source
validation can invalidate an otherwise complete pixel stream. Iterating to EOF
joins the producer and emits any terminal error once; `finish()` also drains
unread chunks and returns the final report. Cancelling produces an error, and
dropping closes the receiver, cancels the producer, and joins it. Cancellation
stops scheduling new work; an in-flight native read or HTTP retry sequence may run through
its configured timeouts and bounded retry delays. No thread is detached. Panics in a read task become errors.

`capacity=0` is a rendezvous channel, default capacity is one, and the maximum is
64. The queue adds at most `capacity × largest delivered chunk payload`, including
its coordinate vectors, beyond scheduler reservations. One consumer-owned
chunk and any chunks the consumer retains are outside that queue. A caller that
collects the iterator owns the resulting memory. Callback `Prepared::execute`
avoids this additional channel and remains available for borrowing consumers.

All callback and file paths use the shared scheduler. Each COG selection is
admitted once and its existing windows run in plan order on one worker-local
GDAL dataset. Intersecting output windows reuse that dataset's source blocks;
only one native output buffer is live per selection. Independent assets can
run concurrently. Zarr source chunks remain individual admitted operations.
A bounded rendezvous/acknowledgement path delivers chunks in deterministic plan
order. Later selections can prepare a chunk while waiting for earlier output;
slow consumers apply backpressure. A single COG selection uses one data worker.

Defaults use up to **two** CPUs/workers and four operations per origin. Set CLI
`--workers` / `--per-host`, or library-process environment variables
`TILE_RIPPER_WORKERS` / `TILE_RIPPER_PER_HOST` (each 1–64). These settings also
bound remote COG layout inspection and metadata HEAD batches. Layouts are
inspected in bounded batches across independent asset URLs, with caller-local
GDAL options copied to their threads; plans are assembled in input order.
Repeated URLs within a batch retain serial inspection. Remote cache invalidation
occurs before binding/inspection; execution retains those same-version headers
and still validates sources before reading and after completion.

`budget.buffer_bytes` limits active working reservations plus worker-lifetime
Zarr decoded coordinate/dimension-array reservations. For a COG selection it
reserves the largest output window, never an entire acquisition. Worker count
is reduced when retained reader state would leave insufficient space for one
task. GDAL caches, decoder scratch, prepared metadata/plans, runtime allocations
and caller-retained chunks are additional. This is **not an RSS limit**.
`completed_tasks` counts the original read nodes; `admitted_jobs` additionally
shows selection-level COG admissions. Cancellation is checked between windows.

For the tested resource-constrained profile, use two workers, a **32-MiB GDAL
block-cache setting**, a **16-MiB working budget**, queue capacity zero and a
consumer that retains at most one delivered chunk. The measured native-consumer
RSS gate is 192 MiB. Explicit worker, request-buffer, queue and GDAL cache
settings remain available; the CLI respects explicit environment overrides.
Embedded callers must configure GDAL's process cache before its first use;
Tile-Ripper does not reset an already-initialized host cache. See the
[measured gate and its limits](../cog-efficiency/report.md).

HTTP Zarr stores share a pooled client and an atomically enforced aggregate
metadata/coordinate cache capped at half the request buffer.
 Each object remains ETag/size bound;
metadata/coordinate bytes are reused only within the prepared snapshot. Source
data is not cached across acquisitions: overlapping requests reread the selected
chunks. No unbounded data cache, coalescing heuristic or extra calibration probe
was added. Missing chunks retain native fill and remain revalidated. Retries are
bounded and occur on the owning worker; unrelated workers remain available.

On callback cancellation, the same immutable `Prepared` can execute again and
reads the full selection. File adapters remove a failed new output directory;
a rerun starts the full acquisition. There is no persistent partial checkpoint
or resume-at-byte protocol. Existing directories are never overwritten.

## Public consumer contract

`Prepared::into_stream(StreamOptions)` owns one prepared acquisition. The
callback API has the same native chunks and final validation. The external
[`native-stats` consumer](../../consumers/native-stats/README.md) is a separate
Cargo package and uses only these public types and methods.

| Property | Contract |
| --- | --- |
| Identity | Use `(asset_id, variable, indices, window)` within the prepared acquisition. `node_id` identifies a planned read, not a globally unique output chunk: one Zarr source read can emit several times/windows. IDs are not stable across newly prepared plans. |
| Order | Deterministic selected-source/variable and planned-read order, then source time index and row/column subdivisions within each Zarr read. This is not a global chronological sort. A different worker count does not reorder delivery. |
| Dimensions | Each buffer is one 2D row-major plane: `pixels[row * window.width + column]`. `window.x/y` are source column/row offsets; edge chunks may be smaller. Nonspatial source indices are in `indices`; the current Zarr profile has one time index, COG has none. The prepared plan records source dimension names/order. |
| Native values | `Pixels::UInt16` or `Pixels::Float32` after native codec/filter decoding, without radiometric scale/offset, resampling, mosaicking or calibration. Float bits (including NaN/signed-zero bits) and UInt16 values are copied without numeric conversion. Polygon exclusions are intentionally replaced by source nodata. Missing Zarr chunks are native fill, not original observations. |
| Coordinates | `transform` is a GDAL-order corner affine translated to this chunk's upper-left source cell. `crs` describes native coordinates. `pixel_center(c,r)` uses exact rectilinear `coordinates` when present; those vectors take precedence over an approximate affine. Column/row arguments are chunk-relative. Grids can differ across assets. |
| Time/variable | `datetime` identifies the observation, `variable` identifies the source band/dataset, and Zarr `indices` retain the source time index even when selection is sparse. COG multi-file band names and acquisition provenance are joined through `asset_id` to the prepared source record. |
| Missingness | `validity()` rejects any NaN and the declared nodata value. Finite fill equals nodata where declared. It does not apply cloud/quality filtering and does not distinguish a filled missing source chunk from stored pixels with the same fill. The final report counts filled source chunks; it is not a per-pixel missing-object mask. Infinities are not automatically invalid. |
| Provenance | `stream.provenance` is the sanitized prepared inspection, including source references, versions, native plan, and dimension/scaling metadata. Preserve it with consumer results. URL secrets are removed; it is diagnostic metadata, not a replayable execution document. |
| Backpressure | Queue capacity is 0–64 (default 1). A full queue blocks delivery and eventually task admission; workers can still finish their bounded in-flight work. Capacity 0 is a rendezvous. A slow consumer can delay later assets. |
| Cancellation | `cancel()` is cooperative. Consume the error or call `finish()`; neither authorizes committing results. Dropping cancels, closes the receiver and joins all producer threads. In-flight native requests/retries can run to configured timeouts. Consumer callback errors abort acquisition. |
| Errors | Items are fallible. A terminal error appears at most once from the iterator and remains an error from `finish()` even if the caller already observed it. Earlier chunks and aggregates must be discarded or kept explicitly provisional. |
| Mutation | Local size/mtime identities, conditional COG ETags and pinned Zarr object identities are checked; remote COG identities are also checked before and after execution. The final pass covers already-read objects and missing Zarr dependencies. This detects observed version changes, not an atomic cross-object snapshot; an ABA replacement or a provider lying about versions is outside the guarantee. |
| Completion | Only successful `finish()` returns the final `ExecutionReport`. EOF, the expected pixel count, or the last chunk does not replace that receipt. `finish()` drains unread chunks; it validates the acquisition, not whether the consumer processed those drained values. A consumer claiming complete statistics must process every item and compare its counts with the report. |

A consumer should stage summaries or derived data while reading, then commit
only after `finish()` succeeds. No rollback of external consumer side effects
is performed by the library. A process crash or dropped stream produces no
successful completion receipt. There is no durable transaction or persistent
resume protocol. A consumer can use its own staging/atomic-rename policy.

For queue capacity `C`, largest delivered payload `L` (values plus coordinate
vectors), working reservation `B`, and consumer-retained payload `R`, the
separate payload components are `B + C*L + L + R`, plus HTTP metadata cache at
most `B/2` and file-writer buffering where applicable. GDAL caches, decoder
scratch, plans, reports, source identity tables and runtime allocations are
additional. This is an admission/payload model, not a process RSS ceiling.
The standalone demonstration uses capacity 0, a 2-MiB maximum delivered payload,
no retained pixel arrays, and measures a separate 192-MiB process RSS gate.

Remote COG version checks now use bounded batches and a shared pooled client.
The existing worker/per-origin settings cap each whole batch conservatively;
results and errors are restored to source order before pins or plans change.
GDAL layout inspection remains serial, and meaningful pre/post source checks
are retained. The [real-source gate](../production-streaming/report.md) records
both the improvement and the remaining costs.
