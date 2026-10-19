# Buffered Zarr output: workflow and measurement protocol

This milestone keeps the native Zarr v2 layout, shared planner and source traversal.
Only the Zarr directory output adapter buffers destination chunks. No dependencies,
codecs, formats, downloader or remote Zarr implementation were added.

## Reproduce

From the repository root, with the frozen [Zarr environment](zarr-workflow.md):

```sh
cargo build --release --locked --bins --examples
prototype-output/zarr-env/bin/python bench/zarr_workflow.py --fixture
./target/release/tile-ripper plan examples/unified-zarr.json --human
./target/release/tile-ripper fetch examples/unified-zarr.json --output prototype-output/my-buffered-extract
cargo run --release --locked --example acquire -- examples/unified-zarr.json --stream
```

The output directory must be new. The fixture is deterministic, georeferenced,
contains gzip-compressed variables, different native chunks, missing data and
real coordinate arrays. No network downloads are needed. The documented real
[STAC demonstration](unified-workflow.md) remains supported.

To reproduce the comparisons, preserve release binaries built with the same
Cargo.lock/GDAL/toolchain from `b3ceb70` and `46dd503`, at
`prototype-output/write-profile/b3ceb70` and
`prototype-output/unified/previous-46dd503`. Build those revisions in separate
checkouts; do not change the current source during a measurement.

```sh
mkdir -p prototype-output/write-profile
cc -O2 -dynamiclib bench/io_profile.c -o prototype-output/write-profile/io.dylib
prototype-output/zarr-env/bin/python bench/output_write.py --phase after --reps 3
prototype-output/zarr-env/bin/python bench/output_validation.py
cargo test --locked
```

The syscall profiler is macOS-specific and used only for benchmarking. It does
not replace the production storage library. On other systems, the Rust store
counters/timings remain available, but these syscall measurements need a native
tracing equivalent. No privileged/global cache dropping is required.

## Mechanism and fix

Source-aligned buffers overlap the fixed destination chunk grid. Before this
change, every callback invoked zarrs' partial-array writer, which could read,
modify, gzip-encode and replace several destination chunks. The pinned
`zarrs_filesystem 0.3.13` synchronizes every object write using Rust `sync_all`;
on this Mac that reaches `F_FULLFSYNC`. Repeating that work, rather than source
retrieval, decoding, GDAL or copying, explains the measured regression.

The output adapter now holds an LRU of decoded destination chunks, capped at
one quarter of the existing buffer budget. It copies incoming rectangular
regions into those buffers and calls the existing zarrs full-chunk writer on
eviction, variable changes and finalization. Source reads and the DAG are
unchanged. Cache misses use zarrs to retrieve existing destination chunks or
native fill. No manual TIFF/Zarr decoder, compression implementation or disk
staging layer was added. Eviction can still cause rewrites on larger frontiers;
this is a bounded improvement, not a universal one-write-per-destination proof.

The bound covers decoded payloads; maps, coordinates, library codec buffers,
filesystem bookkeeping and allocator overhead contribute to RSS separately,
as before. One active variable shares the output allowance, rather than each
variable receiving its own allowance. The reported peak confirms admission.
Write/codec errors propagate, caught failures remove the newly owned output
directory, and the shared completion manifest is still published last.
Per-object synchronization was retained. This does not newly guarantee atomic
multi-file snapshots or power-loss durability of the entire directory/manifest.

## Measurement contract

The final [raw evidence](output-write-after.json) includes every repetition,
binary/source/harness hashes, source key traces, stage timings, process peak RSS,
file counts, final file bytes, store reads/writes and syscall counts/bytes/times.
The final regression record identifies the measured binary and the final binary;
the only post-measurement Rust tidy moves the diagnostic helper above tests.
Three shuffled fresh-process repetitions per reader, workload and cache mode
compare current, `b3ceb70`, `46dd503`, xarray and a minimal direct Zarr writer.
All outputs are checked independently with xarray/Zarr, both raw and CF-decoded.

The Python baselines explicitly match UInt16 values, shape, coordinates,
attributes, nodata, gzip level, destination chunk shape, sparse fill behavior and
per-object `F_FULLFSYNC`. The minimal writer also matches each array's dot/slash
key separator; xarray uses its default key separator, which changes directory
organization but not scientific/output chunk semantics. Both include a common
manifest payload. Whitespace, optional non-scientific metadata and manifest
schema/profiling fields differ, so final directory byte counts are reported,
not assumed identical. The historical xarray timing used native-sized output
chunks and did not synchronize every object: it is not the matched baseline.

“Warm” repeats use the OS file cache. The cold-source condition opens source
files with Darwin `F_NOCACHE`; this bypasses source file caching, not hardware
caches or every metadata cache. Each run creates a fresh destination. Neither
condition is a pristine machine reboot. Full synchronization has substantial
workstation variance; ranges matter. Counters measure successful bytes passed
to filesystem writes, not physical-device traffic or filesystem journaling.
Create-opens include replacement/truncation; unique final files are counted
independently. Per-PID trace files prevent Python helper processes overwriting
parent measurements.

Stage accounting is explicit:

- `planning_seconds` includes metadata lookup/validation and DAG creation;
  `source_open_seconds` includes execution-time coordinate validation/opening.
- `source_retrieval_seconds` times source data-store calls. Subtracting it from
  fused `read_decode_seconds` gives decode/materialization/library overhead,
  not pure gzip CPU time. Independent read-only and gzip decode-only probes
  are reported separately and are not additive production timings.
- Assembly includes callback allocation, row copying, validity counting and
  chunk descriptors; allocation/copy subtimers are nested, not additional work.
- `output_setup_seconds` includes group/array metadata and coordinate writing.
  Output I/O timings cover library store calls; syscall timers separate
  creation/opening, byte writing, synchronization and closing.
- `encode_seconds` times full-chunk encoding/serialization/library overhead
  after subtracting nested destination store writes. Output allocation includes
  fill initialization or decoding an evicted destination; output row copying
  is timed separately. This is not an isolated compression microbenchmark.
- `sink_seconds` and `output_finalize_seconds` include eviction and final buffer
  draining respectively. These overlap the I/O/encoding subtimers.
- Opt-in `TILE_RIPPER_STAGE_TRACE` records native/common manifest construction,
  serialization, write/close and final rename. The trace sits outside outputs.
  Historical executables lack these added timers; unmeasured work remains in
  wall time rather than being invented from differences.

## Bounded stream assessment

`Prepared::execute` already provides a fallible synchronous push stream: the
consumer processes or persists a buffer before requesting more work. It does
not collect the extraction, require a worker thread, or need an async/plugin
framework. Returning an error cancels execution. A successful final execution
report is necessary: earlier delivered chunks can precede a later source error.

Each `Chunk` carries stored UInt16 values, variable, acquisition timestamp,
source indices, native CRS/affine, window, nodata, scale/offset and source asset
identity. `pixel_center` exposes affine pixel centers; `validity` exposes the
nodata mask without allocating another buffer. The library example emits one
sanitized provenance header from the prepared inspection, joins it by asset ID,
and consumes each buffer immediately. Callers retaining buffers exceed the
executor's memory guarantee.

Affine centers are not a promise of bit-identical stored Zarr coordinate-axis
values: the source validation allows a small affine tolerance. A fully specified
export stream should add one small header with exact selected axes and source
references, followed by the existing buffers and a completion record. Coordinates
already fit the planner's admission limits. This makes architectural sense as a
small lifecycle interface, but this milestone does not claim a new general
scientific-array stream or implement a ChronoZarr adapter.

Preservation review: the pinned filesystem store retains a lock per object.
Output-object count (including group/coordinate metadata) is now preflighted
against `budget.max_nodes`, bounding that bookkeeping as well as cached payload.
The guard rejects excessive tiny-chunk outputs before creating the directory;
normal benchmark requests and their measured I/O are unchanged.
