# Reproduce Version D quality-aware acquisition

Start at `dbafecc` on `codex/acquisition-planner`. Use the real D PGD0 product
recorded in `product-profile.json`; the earlier C product is a regression only.

```sh
cargo build --release --locked --no-default-features --features native-hdf5 \
  --bin tile-ripper --example hdf5_subset_oracle --example swot_quality
```

The native core and public example need neither Python nor libhdf5 nor GDAL.
The following commands use the separate pinned scientific reference environment:

```sh
uv run --no-project --python 3.12 \
  --with-requirements docs/native-hdf5/reference.txt \
  python bench/swot_quality.py science
```

Replace `science` with the action required. Only `inspect`, `capture` and
`access` contact NASA; all other actions are offline and have no download
fallback. `inspect` has a 512 KiB metadata response-body cap. `capture` has a
24 MiB aggregate provider-body cap and at most 1 MiB per explicit capture read;
it collects selected variable chunks and metadata, never the complete object.
`access` checks eight-byte conditional GETs and native redirect rejection.
Existing caller-owned reference credentials are never printed or saved.

The incomplete sparse backing file is
`prototype-output/swot-quality/mirror/source.nc`. Retain its `manifest.json`:
the file has full logical length but only sealed captured extents. The HTTP
mirror validates extent hashes before replay and rejects every uncaptured hole.
NetCDF-C is a separately labelled local sparse-file control; its actual kernel
read ranges are not instrumented. h5py range controls verify ETag/length before
and after access. No remote NetCDF-C throughput equivalence is claimed.

`request.json` contains one native request for the five raw variables.
`cookbook-policy.json` contains the explicit separate derived policy. The live
canonical request is not executable by today's native authenticated transport;
replace its locator with a **verified local complete file** or the harness's
bounded mirror. Never present the sparse path without its manifest as complete.

```sh
target/release/examples/swot_quality request.json cookbook-policy.json stream
```

The public API is `swot::prepare(&request, policy, &mut cache)?.into_stream()?`.
Each tile contains five `Chunk`s in `swot::PATHS` order, unchanged native values,
matching windows/strides/coordinate centers, and a separate UInt8 derived mask.
`Chunk::validity()` supplies each layer's independent source missingness mask.
Original projection WKT/CF attributes, source and variable identity, flag
attributes, packing, fills and processing times are in `Prepared::explain()` /
`Stream::provenance`. HDF5 `transform` remains an array-index placeholder and
`crs` an axis token; use stored coordinates with the original projection.

All tiles and consumer calculations are **provisional until `finish()` succeeds**.
EOF alone is not a completion receipt. Cancel/drop joins the worker. Final source
validation failure prevents a successful receipt even after tiles were delivered.
Dump mode writes validation intermediates before finish; they are not published
science outputs and must be ignored if completion fails.

The prepared plan/read estimates describe the ordinary raw acquisition. Aligned
execution assembles intersections and may change physical request counts; use
the observed report for traffic. Aligned layer `node_id` values identify their
WSE logical anchor. Each layer's variable path and the source's dataset bindings
identify its own physical inputs.

The five-layer reader uses one pinned file metadata context and one coordinate
read per axis. It assembles logical intersections from each layer's own index.
A bounded execution-local LRU of encoded ranges avoids repeat payload GETs when
possible; eviction may require rereads. Decoded HDF5 chunks are not retained in
an unbounded cache. Output tile size follows remaining working admission, and
only one tile is sent through a rendezvous channel. Large source chunks fail
admission. The full index policy remains the default; experimental selective
traversal validates only visited subtrees/pointers.

Preparations, selected coordinates and plan records are bounded by buffer and
node admission, but plan/inspection representation grows with selected chunk
count. The working reservation includes metadata state, encoded cache capacity,
conservative decoder planes, assembly, raw outputs and derived mask slack.
It is not a process RSS cap. Runtime, allocator overhead, diagnostic clones,
consumer state and caller-retained tiles differ. Synchronous preparation and
blocking HTTP calls preserve the inherited timeout/cancellation boundaries.

Run the new gates:

```sh
python bench/swot_quality.py fixtures   # independent small golden files
python bench/swot_quality.py science
python bench/swot_quality.py raw-science
python bench/swot_quality.py faults
python bench/swot_quality.py resources
python bench/swot_quality.py real-resources
python bench/swot_quality.py integer-admission
```

Science runs every selected value, source mask, coordinate, logical coverage,
derived mask and summary through isolated h5py and NetCDF-C workers. Cases cover
D windows, boundaries, strides, dense and missing regions, different physical
layouts, both integer byte orders and high/undefined flag bits. The dump files
are ignored; compact versioned receipts record the evidence. `fixtures` also
reproduces four tiny unsigned-initialization files with no allocated science
chunks, independently through h5py.

`inherited` runs the Rust feature/Clippy matrix, HDF5 science/fault/resource and
full/selective/coordinate gates, the original real C WSE qualification and the
compressed-axis matrix. `formats` rebuilds and checks COG, Zarr v2/v3, native
streaming, source faults and transactional publication using the preserved
reference environments. Earlier real C and GOES cases require their preserved
sealed mirrors; there is no implicit provider download if they are absent.
See `verification.json` for commands and log hashes.
The vendored reader has one additional local rejection guard for integer bit
offsets/partial precision; its complete unit suite is repeated. This is not an
upstream submission.

After validation and compilation have stopped, run benchmarks serially:

```sh
python bench/swot_quality.py benchmark
python bench/swot_quality.py compact_receipts
python bench/swot_quality_report.py
```

There are three fresh processes per cell. Native WSE-only, raw multi-variable
and aligned quality-aware operations use the same conditional mirror at zero
and 20 ms added HEAD/GET delay. Mature h5py controls use exact/16 KiB reads;
h5py and NetCDF-C local controls are labelled separately. Native validates more
metadata and emits provisional tiles, while mature controls return complete
slice summaries. Record CPU, RSS, preparation, first output, completion, bytes,
requests, overlaps, variable phases and reservations without claiming a
universal performance lead. No benchmark contacts the provider.

The dgov plan is structurally compiled. Dispatch/provider configuration is
absent; implementation and checks run directly in this existing worktree.
No agents, merges or automated deployment workflows are dispatched.
