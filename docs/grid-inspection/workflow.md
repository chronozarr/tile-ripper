# Reusing a native SWOT grid inspection

`discover-job` now inspects each geographic source once. It reads the real grid
shape and full stored `/y`,`/x` axes, resolves the original geographic AOI, and
builds the selected five-variable plan from the same source binding. It does not
construct a full-scene WSE execution plan to discover the axes.

The supported products, full-center predicate, `1e-10` degree boundary tolerance,
window/mask fingerprints, raw values and three independent mask meanings remain
those in [the AOI contract](../swot-aoi/workflow.md). Full legacy chunk-index
validation remains the default. The reusable SWOT inspector requires that policy;
the experimental selective policy is outside its capabilities.

## CLI

The existing commands and manifest schema are unchanged:

```sh
cargo build --locked --release --no-default-features \
  --features native-hdf5,authenticated-http --bin tile-ripper

target/release/tile-ripper discover-job examples/swot-aoi-selection.json \
  --auth earthdata --netrc /path/to/private.netrc --output new-job.json

target/release/tile-ripper plan new-job.json \
  --auth earthdata --netrc /path/to/private.netrc \
  --checkpoint /path/to/private-checkpoint

target/release/tile-ripper fetch new-job.json \
  --auth earthdata --netrc /path/to/private.netrc \
  --checkpoint /path/to/private-checkpoint --output new-observations.zarr
```

`discover-job`'s output is a source-pinned logical request. Each independent CLI
process still reauthenticates and builds its own scientific plan. `plan` and
`fetch` can share the existing source-range checkpoint for the exact same
scientific request. A plan can warm metadata ranges for a later fetch; it does
not capture science payloads. A completed fetch's checkpoint can reconstruct
all observations from retained ranges. Every reconstruction rewrites the output
and must pass stream completion and the fresh final source-validation sweep.

The checkpoint retains source bytes and their integrity records, never parser
objects. Its online GET authorization proof, canonical ETag/size binding,
scientific interpretation identity, private source directories, content digests,
locks and interruption behavior are unchanged. A geographic request cannot warm
this checkpoint before its indices and scientific identity are resolved.

## Rust API

`swot::GridInspection` is a nonserializable runtime binding with four operations:

- `inspect` / `inspect_authenticated`: inspect a local/anonymous source or an
  exact-object caller-owned authenticated session.
- `describe`: bounded diagnostic grid identity, capabilities, inspected dataset
  summaries, transport counters, timings and retention reservations.
- `with_grid`: borrow immutable source definitions and complete stored axes.
  The callback can call back into the API; no axis-cache mutex is held.
- `prepare`: validate the current source version, rebuild a separate physical
  plan for a five-variable window and admit its explicit quality policy.

For example, with an existing native five-variable `Request` and an explicit AOI:

```rust
let grid = swot::GridInspection::inspect_authenticated(&request, session)?;
let description = grid.describe();
let crs = description["grid"]["crs"].as_str().ok_or("missing CRS")?;
let (selection, _) = grid.with_grid(|definition, y, x| {
    swot_aoi::resolve_grid(&aoi, crs, y, x, &definition["grid_mapping"])
})?;
let selection = selection.ok_or("zero native centers")?;
let mut selected = request.clone();
selected.spatial = selection.window.spatial();
let Source::Assets { assets } = &mut selected.source else { unreachable!() };
assets[0].etag = description["source_identity"]["version"]["etag"]
    .as_str().map(str::to_owned);
let prepared = grid.prepare(&selected, swot::Policy::cookbook())?;
let mut stream = prepared.into_stream()?;
for tile in stream.by_ref() {
    consume(tile?)?; // provisional raw layers, source masks and derived quality
}
stream.finish()?; // required source validation before accepting the result
```

The production discovery bridge also attaches the original AOI, resolved grid
fingerprints and `aoi_mask` policy to its observation. The writer validates them
and creates the separate AOI array. The sample above illustrates raw aligned
streaming; it does not apply an implicit geographic mask to science values.
The [compiled benchmark adapter](../../examples/grid_inspection_bench.rs) is a
complete public-API example with explicit source bindings.

Clones share immutable metadata, parsed definitions/indexes and `Arc<[f64]>`
axes. Every preparation owns distinct selections, coordinates, jobs and physical
range groups. The caller controls concurrent requests; the serial job retains
only one active source and its existing aggregate admission.

A retained inspection is bound to its exact source ID/locator, source version,
authentication scope, `buffer_bytes`, `max_nodes`, metadata policy and index
interpretation. A later selection must fit the inspected axes and datasets.
Changing the retention budget or policy requires a new inspection. Changing
`chunk_edge` or the per-request quality policy can reuse the inspection, subject
to normal admission. A changed source or an unavailable profile/axis fails
explicitly. There is no fallback to a changed object or another granule.

## Information and memory boundaries

| Information | Retained representation | Admission/use |
|---|---|---|
| Geographic selection | Actual source CRS, dimensions, row/column ordering, axis direction, full bounded stored centers and SHA-256 fingerprints | Existing strict 100 m WGS84 UTM profile; at most 8,192 positions per axis and 16,777,216 centers |
| Scientific admission | Five raw dataset definitions, types, missingness/packing metadata, source flag definitions, chunk shapes and full parsed legacy indexes | Policy and selected schema validated for every request; metadata inspection alone does not establish useful WSE |
| Execution | A newly constructed bounded physical plan, selected coordinates, immutable source metadata and an exact-object transport | Conditional source reads, memory admission, stream completion and final validation |

Metadata ranges retain the existing `buffer_bytes / 32` cap. Parsed metadata,
root/time/mapping definitions, full axes, parsed indexes, plans and transport
reservations are charged in prepared-memory accounting. The aggregate retained
source preparation must fit half the source budget; execution/decoder/assembly
reservations still have to fit the whole source budget. Full coordinate
reservations are bounded before retrieval, and full index retention is bounded
before collection. Ordinary HDF5 profiles too large for the parsed-index
optimization retain their existing bounded traversal path. The reusable SWOT
inspector instead reports that full index retention cannot be admitted.

Diagnostic byte reservations are conservative working allowances, not a process
RSS ceiling. The matched benchmark separately records Rust allocations and RSS.
See [measurements and the decision](report.md).

## Reproduction

Use the existing qualified reference environments and captures; the frozen
commands never contact NASA. Build the baseline first, then rebuild the current
checkout because both builds use the existing Cargo target directory:

```sh
python bench/grid_inspection.py prepare-baseline
cargo build --locked --release --no-default-features \
  --features native-hdf5,authenticated-http --bin tile-ripper \
  --example swot_aoi_oracle --example grid_inspection_bench \
  --example hdf5_subset_oracle
python bench/grid_inspection.py benchmark
python bench/grid_inspection.py faults
```

The baseline is `afe41c2`. Its benchmark-only patch changes the visibility of its
existing resolver; the resolver body is unchanged. The adapter uses the same
native preparation, resolution and admission operations with catalog provenance
omitted for loopback HTTPS sources. Production CMR provenance still requires
the authoritative NASA locator. The independent CLI plan/fetch trials exercise
normal entrypoints; a separate bounded live trial exercises production
`discover-job` with actual catalog provenance and archive authentication.

The live acceptance is explicit and reuses the original three pinned products,
geographic inputs, authoritative catalog receipts and a copied completed
checkpoint. Its predeclared limits are 2 MiB of source bodies and 240 wire
requests. It creates no new catalog search or science capture:

```sh
python bench/grid_inspection_live.py --netrc /path/to/private.netrc
```

The live harness checkpoints its generation measurements so `--resume RUN_DIR`
can use an already generated manifest without repeating provider inspection.

Full diagnostic receipts remain in ignored local storage; compact evidence and
its content hashes are versioned. After regenerating receipts, run
`python bench/grid_inspection_receipts.py`; [the receipt index](receipt-index.json)
links each compact document to its full local original.
