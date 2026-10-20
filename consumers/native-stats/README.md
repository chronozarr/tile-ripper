# Independent native statistics consumer

This is a standalone Cargo package with a path dependency on Tile-Ripper. It
uses the public `acquisition`, `ChunkStream`, `Chunk` and `Pixels` APIs; there
are no private imports or intermediate image files.

```sh
CARGO_TARGET_DIR=target cargo build --release --locked --manifest-path consumers/native-stats/Cargo.toml
TILE_RIPPER_WORKERS=2 GDAL_CACHEMAX=32 target/release/tile-ripper-native-stats request.json
prototype-output/zarr-env/bin/python bench/consumer_validation.py
```

Supply the ordinary Tile-Ripper request schema, with `sink.kind = callback`.
Several assets can be requested together. A chunk's valid-pixel count and native
sum are useful for observation coverage and stored-value quality control;
these are not calibrated reflectance or physical-temperature statistics.
The checksum independently verifies native sample bits.

Each NDJSON chunk is explicitly `provisional_chunk`, with source/variable/time
identity, window, native transform/CRS, exact coordinate vectors if present,
missingness/scaling metadata, valid count, sum and sample checksum. The opening
`provisional_start` record preserves the full prepared provenance. Consumers of
this NDJSON must require exactly one final `complete` record and a successful
exit code before committing statistics. An `aborted` record invalidates every
preceding provisional result. An interrupted process has no receipt.

The consumer retains one delivered payload and O(1) aggregate state; descriptors
are streamed to stdout. It checks a 2-MiB delivered-payload limit (values plus
coordinates), uses a rendezvous stream, and verifies processed chunk/valid-pixel
counts against `finish()`'s report. Set request `buffer_bytes` and `chunk_edge`
to admit appropriately sized chunks. The payload check happens on delivery; it
is not an allocator sandbox. Prepared plans/reports and GDAL/runtime allocations
are additional. The validation harness sets a 32-MiB GDAL cache and enforces a
separate measured 192-MiB process-RSS gate. A downstream stdout collector owns
any memory it retains.

`--cancel-after N` demonstrates cooperative cancellation. `--pause-after N`
waits for a line on stdin after delivery, allowing the external test to mutate
an already-read source before allowing completion. Neither is a production
scheduler feature. `--max-chunk-bytes N` adjusts the consumer's payload check.

The real COG gate uses this fixed two-worker recipe for its 192-MiB RSS target.
The default four-worker profile exceeded that target in three latency-matrix
runs, while all 24 two-worker follow-up acquisitions passed. The working-buffer
budget alone cannot guarantee a process RSS limit; see the gate report.
