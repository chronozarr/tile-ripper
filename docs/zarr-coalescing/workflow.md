# Bounded remote shard reads

This extends the admitted [v3 scientific profile](../zarr-v3-input/workflow.md).
Ordinary v3 chunks, local sources, v2, COG and output defaults keep their existing
access strategies. Remote sharded v3 uses the cost policy by default.

`Prepared::explain().sources[].zarr_v3` includes
physical read plans: logical indices, physical offsets/lengths, physical order,
gaps, missing members, unique/repeated coverage, source URL and pinned ETag.
Candidate ranges there precede execution-window and memory limits. The execution
receipt's `scheduler.zarr_v3_coalescing.batches` records the actual admitted
groups and reservations. `transport` retains exact HTTP attempts, conditional
versions, status, range identities, retries and response-body bytes.

## Policy configuration

| Environment variable | Default | Meaning |
| --- | --- | --- |
| `TILE_RIPPER_ZARR_COALESCE` | `cost` | `individual`, `adjacent`, `fixed`, `cost` |
| `TILE_RIPPER_ZARR_MAX_SPAN` | 4194304 | Maximum merged span in bytes; `fixed` defaults to 16777216 |
| `TILE_RIPPER_ZARR_MAX_GAP` | 65536 | Maximum gap in bytes; `fixed` defaults to 1048576; `adjacent` uses zero |
| `TILE_RIPPER_ZARR_OVERFETCH_RATIO` | 1 | Extra bytes / selected bytes for each group; fixed reference effectively has no relative limit |
| `TILE_RIPPER_ZARR_LATENCY_MS` | 2 | Static expected request latency, milliseconds |
| `TILE_RIPPER_ZARR_BANDWIDTH_MBPS` | 32 | Static expected throughput in decimal **megabytes/second**, not megabits/second |

Existing `TILE_RIPPER_WORKERS`, `TILE_RIPPER_PER_HOST` and request `buffer_bytes`
remain authoritative. For a known 8 ms / 100 Mbit/s path, expected-cost parameters
can be set to latency `8` and bandwidth `12.5`. These are explicit estimates;
there is no provider detection, active probing, learned controller or persistent
cache. The benchmark reports also retain the default estimates for unknown
providers.

The cost rule accepts a physical gap when `gap / expected_bandwidth` is less
than `expected_latency / available_concurrency`. Each accepted extension removes
one request. Hard span, gap, relative-overfetch and memory limits still apply.
Directly adjacent ranges have no transfer penalty. The fixed reference follows
the Python 1 MiB gap / 16 MiB span settings; the adjacent reference permits no
overfetch. Individual mode preserves one payload range per logical chunk.

## Execution and memory

The scheduler considers consecutive windows of at most 32 original read nodes.
Within each shard it sorts the validated spans physically. Fetch membership
retains logical indices; delivery remains in the original node/chunk order.
Spans never cross objects or prepared source identities. zarrs interprets the
index and verifies the per-member byte range; its existing codecs decode each
inner chunk. Missing entries are handled before the library sentinel helper.

Each worker lazily fetches a merged body on its first needed member. Immutable
`Bytes` views slice that body without per-member transport copies. All merged
bodies in the task remain charged until task completion, including backpressure
waits. Bodies/views are cleared before releasing admission on success, errors,
cancel and caught worker panics. Another task or execution never reuses them.

The task charge is the original maximum per-member working reservation plus
twice the sum of merged body lengths (including one-byte body-bound sentinels).
This conservatively covers retained bodies, transport capacity/copy allowance,
and existing decoder/native/output work. Batch descriptions are separately
charged for the whole execution. A fair share of the remaining budget bounds
each window; excessive windows split, then fall back to precise reads. If the
upper bound for batch descriptions exceeds one quarter of available memory,
batching is skipped. The cost policy also splits a window when its serial
request count exceeds the original parallel request waves. It therefore avoids
serializing many unrelated sparse reads to save one request.

Requests retain strong ETag / If-Match, no redirects or whole-object fallback,
exact 206 Content-Range/Length, identity content encoding and three bounded
attempts. Range headers and versions are checked before allocating/reading the
body, which is bounded to the requested length plus one byte. Metadata and data
versions are checked through completion. In-flight requests can finish before
cancel is observed; successful `finish()` remains mandatory.

The working reservation is not an RSS ceiling. Process/runtime allocations,
HTTP machinery, explain/plan/receipt bookkeeping, caller retention and the OS
page cache differ. The retained index/coordinate admission limit and measured
RSS gates continue to apply. Per-object versions do not create an atomic
dataset snapshot. No output synchronization or publication rule changes here.

## Reproduction

Use the frozen reference environment in `docs/zarr-v3-input/reference.txt`:

```sh
CARGO_INCREMENTAL=0 cargo build --release --locked --no-default-features \
  --features native-cog --examples --bin tile-ripper --target-dir target/zarr-v3-input
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing.py science
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing.py extended
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing.py remote-memory
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing.py benchmark 3
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing.py pilot 3
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing.py hints 3
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing.py benchmark-science
```

Run timing experiments with no build or validation jobs in parallel. `benchmark`
compares the preserved fe6fcd1 executable, current cost policy and current tuned
Python on both frozen layouts and the added wide layout. `pilot` compares all
four policies plus Python across local, 8 ms / 100 Mbit/s and 40 ms / 1 Gbit/s
links. Both rotate reader order and retain all raw cells, HTTP logs and failures.
The wide fixture uses larger raw inner chunks, 128 KiB padding and reversed
physical member order. Its index is repacked using Zarr-Python's own index
decoder/encoder; production adds no index parser or codec.

`public` repeats the selective GFS oracle with conditional checks against the
recorded selected-object identities. Use it only as a restrained regression;
optimization experiments belong on the mirror. Reports retain public receipts
without asserting provider performance.

Inherited gates can target a separate receipt/binary location without changing
the frozen reports. Set `TILE_RIPPER_V3_REGRESSION_WORK`,
`TILE_RIPPER_V3_REGRESSION_DOCS`, and `TILE_RIPPER_V3_REGRESSION_BIN_DIR`, then run
`bench/zarr_v3_regressions.py` with `remote`, `cf`, `checks`, `faults`, or
`regressions` using the existing Zarr v2 reference environment. The COG regression
requires the native external consumer in that binary directory:

```sh
CARGO_INCREMENTAL=0 cargo build --release --locked \
  --manifest-path consumers/native-stats/Cargo.toml --no-default-features \
  --features native-cog --target-dir target/zarr-v3-input
TILE_RIPPER_ZARR_COALESCE=invalid CARGO_INCREMENTAL=0 cargo test --locked \
  --all-features --test native_cog --test remote_zarr --test cf_zarr
```

The last command verifies Zarr-only settings cannot interfere with COG/v2.
The v3 configuration and extra v3 scheduler diagnostics are scoped to prepared
v3 sources. The plan was structurally compiled and executed directly in this
checkout; automated plan merge/dispatch was not used.

The focused sparse follow-up is reproducible with
`prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing_sparse.py`.
It alternates old/new order for nine pairs per layout/link and records per-pair
differences, preparation, initial output, completion and precise traffic.

`prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_coalescing_coverage.py`
checks a 64-inner-member shard layout interleaving different logical execution
windows. It records selected, unique fetched and repeated physical coverage,
pinned identities, byte-exact independent samples and old/new delivery order.
Execution-window boundaries can duplicate fetched physical bytes within the
overfetch limit; selected index references themselves remain disjoint.
