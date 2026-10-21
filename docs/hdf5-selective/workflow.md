# Reproduce the selective v1 experiment

Use the existing `codex/acquisition-planner` checkout. The starting commit was
`fe58a2c`. Native acquisition requires neither Python nor libhdf5; Python below
is independent validation and benchmark tooling.

```sh
cargo build --release --locked --no-default-features --features native-hdf5 \
  --bin tile-ripper --example hdf5_subset_oracle
TILE_RIPPER_HDF5_INDEX_POLICY=selective \
  target/release/tile-ripper stream docs/hdf5-selective/goes19-request.json --gdal-free
```

The request selects `/TEMP[2500:2564,2500:2564]` and stored `/y`,`/x` axes from
the actual 28,545,639-byte GOES-19 object. Returned UInt16 codes retain their
packing metadata. The source is NOAA's official anonymous S3 archive.

`TILE_RIPPER_HDF5_INDEX_POLICY` accepts `full` (default) and `selective`.
`TILE_RIPPER_HDF5_METADATA_POLICY` accepts `exact` (default), `page4k` and
`page16k`. Invalid choices fail explicitly. Full traversal with 16 KiB pages
retains inherited whole-index traversal while reducing network round trips;
page overfetch and cache reservations are reported. Selective traversal cannot
validate unread subtrees. It is an explicit experiment, not a default promotion.
Payload grouping keeps the existing `TILE_RIPPER_HDF5_RANGE_POLICY` behavior.
No HDF5 index parser was added to Tile-Ripper.

Run every Python command below in the pinned HDF5 reference environment, for
example with this prefix:

```sh
uv run --no-project --python 3.12 \
  --with-requirements docs/native-hdf5/reference.txt python bench/hdf5_selective.py science
```

Actions are `science`, `faults`, `extended`, `extra`, `resources`, `capture`,
`live`, `small`, `benchmark`, `tuning`, `baseline`, `products`, and `coordinates`.
`small`, `benchmark` and `tuning` accept `--repeats 3`. Run timing experiments
serially, without builds or other validation work. CPU records include whole
child processes; mature-reader API-only CPU is retained separately.

- `science` repeats all original 64 cases with explicit selective traversal.
- `faults` repeats the original 33 fault/lifecycle cases.
- `extended` adds full/selected geometry, ordering and structural-scope checks.
- `extra` adds page-cache framing, mutation, cancellation, policy/budget and
  fully unallocated-data checks, including distinct storage initialization and
  CF missingness.
- `coordinates` independently checks stored axes through NetCDF-C for all 123
  scientific comparison cases, with automatic packing disabled.
- `resources` repeats inherited resource gates with the full control and adds
  selected-mode working-budget checks at 4 and 8 MiB.
- `capture` obtains bounded provider ranges for all three declared large-file
  selections, full-index metadata and independent readers. It caps the mirror
  at 16 MiB, conditionally reads exact extents and revalidates provider identity.
- `tuning` uses a separate derived mirror for h5py's larger 256 KiB pages, still
  within the same 16 MiB transfer cap. It reports additional provider bytes.
- `live` reruns full and selective sparse acquisition directly against NOAA;
  the mirror is used only for independent verification afterward.
- `baseline` uses the frozen, unmodified `fe58a2c` oracle copied before editing.
  Rebuild that commit in an isolated checkout if the ignored frozen binary is
  absent. Baseline receipts retain the executable hash.
- `products` performs at most 512 KiB of metadata reads per actual SWOT/NISAR
  object through official GET redirects. Existing Earthdata credentials may be
  used by Requests. It records no credentials or signed redirect URLs and does
  not qualify native product acquisition.

Fresh `capture` is an explicit public-data transfer. Repeated replay never
downloads a whole object. The ignored sparse backing file is incomplete:
uncaptured holes are refused by the HTTP mirror. Its manifest, extents, hashes,
ETag and transfer counts are retained. Do not use it as a complete scientific
product. Local h5py/NetCDF-C checks address already captured selections;
native and range-backed benchmark reads also enforce manifest coverage.

The small whole-file control uses the previously captured 1,268,526-byte RRQPE
object, verifies its MD5 and serves it on loopback. It has no new provider
transfer. A whole-object h5py control is explicitly limited to objects ≤2 MiB;
there is no native whole-object fallback.

Derive report tables after all receipts are complete:

```sh
uv run --no-project --python 3.12 \
  --with-requirements docs/native-hdf5/reference.txt python bench/hdf5_selective_summary.py
```

## Rust and inherited gates

`cargo test --locked` and warning-denying all-target Clippy run under default,
no-default, native-cog, native-hdf5 and all-features configurations. The final
HDF5 and all-features checks are refreshed after parser changes. The vendor
crate also passes all its library unit tests and all-target Clippy:

```sh
cargo test --locked --manifest-path vendor/hdf5-reader/Cargo.toml \
  --no-default-features --lib
cargo clippy --locked --manifest-path vendor/hdf5-reader/Cargo.toml \
  --no-default-features --all-targets -- -D warnings
```

Build the existing GDAL-free regression executables:

```sh
cargo build --release --locked --no-default-features \
  --features native-cog,native-hdf5,experimental-output \
  --bin tile-ripper --example zarr_subset_oracle --example native_cog_oracle \
  --example remote_zarr --example durable_output_candidate \
  --target-dir target/hdf5-regression
cargo build --release --locked --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/hdf5-regression
```

Set `TILE_RIPPER_HDF5_REGRESSION_WORK` to an absolute fresh work directory and
`TILE_RIPPER_HDF5_REGRESSION_DOCS` to this checkout's `docs/hdf5-selective/inherited`.
Then run `bench/hdf5_regressions.py` with `remote`, `cf`, `cog-stream`, `checks`,
and `faults` in the Zarr v2 reference environment. `v3-checks` and `v3-faults`
use the separate frozen v3 environment. The cached environment at
`prototype-output/zarr-v3-input/reference-env/bin/python` was used here; its
Zarr/xarray versions were checked. Fresh installation of the old v3 freeze
failed because its dated certifi and numcodecs pins are unavailable from the
configured index. The existing frozen environment ran all checks successfully;
no scientific dependency versions were substituted.

The dgov plan records the work and was structurally compiled with its identity
bundler. Execution was direct in the reused worktree; no agent dispatch, merge,
tag, push, publication, deployment or ChronoZarr change occurred.
