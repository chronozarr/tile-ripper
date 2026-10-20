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

All callback and file paths use the shared scheduler. It admits COG library windows and Zarr source chunks as individual operations.
Each worker retains its own current native reader, reusing a GDAL dataset or
Zarr array context across adjacent operations from the same selection. Operations use the existing GDAL/zarrs
readers and the same plan. COG reads remain delegated to GDAL; concurrency limits
are reader-operation limits, not a promise about GDAL's internal HTTP requests.
Per-window operations overlap within one ordered queue; a long earlier selection
can occupy all slots and delay later assets. A bounded rendezvous/acknowledgement path
presents chunks to the sink in deterministic plan order. This retains existing
file adapters and prevents asynchronous arrival from overwriting or reordering
observations. Deterministic delivery can wait behind a slow earlier operation;
later operations can proceed only within the admitted worker/buffer window.

Defaults use up to four CPUs/workers and four operations per origin. Set CLI
`--workers` / `--per-host`, or library-process environment variables
`TILE_RIPPER_WORKERS` / `TILE_RIPPER_PER_HOST` (each 1–64). `budget.buffer_bytes`
limits the sum of active working reservations plus worker-lifetime Zarr decoded
coordinate/dimension-array reservations. Worker count is reduced if retaining those readers
would leave insufficient space for one admitted task. Reservations account for native,
encoded and plane buffers for Zarr, and output values for COG. GDAL's own block
cache, prepared metadata, runtime allocations, decoder internals, object/plan
bookkeeping and caller-retained chunks are separate; this is not an RSS sandbox.
A task that cannot fit fails explicitly, rather than exceeding that admission
budget. The report records reserved-byte and active-worker peaks. Metadata HEAD
batches use the same concurrency defaults/overrides and source identity checks.
They run during preparation/validation, separately from data tasks.

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
