# Reproduce the bounded SWOT WSE milestone

Use `codex/acquisition-planner`, continued from `92a6f6b`. Rust acquisition uses
the existing public `acquisition::prepare` / `Prepared::into_stream` API and
requires no Python or libhdf5. Python is independent reference and benchmark
tooling. Build the native executable and validation consumer:

```sh
cargo build --release --locked --no-default-features --features native-hdf5 \
  --bin tile-ripper --example hdf5_subset_oracle
```

The [request](request.json) records the exact original product and index-space
selection, `/wse` with row axis `/y` and column axis `/x`. The current native HTTP
transport rejects its archive redirects. Do not present that unmodified request
as an executable live acquisition. The qualification harness replaces only the
locator with its loopback mirror and binds the captured provider identity in
the receipt. It does not add geographic AOI mapping or a file-output workflow.

Every streamed chunk is provisional until `finish()` succeeds. The validation
consumer emits its successful JSON result only after that final validation.
`Prepared::explain()` retains source/axis attributes, root acquisition metadata,
packing, missingness, dataset path and the original grid-mapping attributes.
`Chunk.coordinates` supplies the requested stored `(x,y)` centers. The existing
HDF5 `crs` token identifies stored axes and `transform` describes array indices;
the original WKT/CF projection is in the prepared metadata. Use those projected
coordinates and original projection together.

## Reference environment and source capture

Prefix Python commands below with the pinned reference environment:

```sh
uv run --no-project --python 3.12 \
  --with-requirements docs/native-hdf5/reference.txt python bench/swot_wse.py science
```

The network actions are explicit:

- `inspect`: at most 512 KiB of file response bodies through the official
  method-specific archive routes. It writes `product-profile.json` with actual
  physical layout, attributes, allocated chunks and request extents.
- `access`: checks exact eight-byte conditional ranges, rejects a deliberately
  incorrect ETag, revalidates length/version, tests method-specific resolved
  routes, and records the expected native redirect rejection.
- `capture`: an 8 MiB total provider-body cap. It batches inspected metadata
  into bounded 16 KiB pages, captures the four WSE chunks required by the
  declared selections and the two compressed coordinate chunks, and discovers
  any additional reference metadata under the same cap. It seals extent hashes
  only after revalidating the original provider identity.

These research tools use Requests' existing caller-owned Earthdata/netrc
configuration. They never save credentials, cookies, authorization headers or
signed redirect URLs. Network-boundary exceptions redact URLs. They do not
implement authentication or redirects inside the native adapter. The access
receipt contains only the canonical public locator, response identities,
statuses and host names. Do not copy signed URLs into requests or reports.

The ignored mirror is `prototype-output/swot-wse/mirror/source.nc`. Its logical
size equals the provider object, but **it is incomplete**. `manifest.json`
records every downloaded extent, the canonical source, opaque strong ETag,
logical length and SHA256 per captured extent. It is not a whole-file hash.
The HTTP mirror refuses every uncaptured hole. `Mirror.verify()` runs before
replay. No benchmark or replay action has a provider-download fallback.
Preserve this manifest alongside the sparse file; do not use the sparse file as
a complete product. The versioned capture receipt retains the same extent hashes.

`netCDF4` uses the sparse local file as an independent NetCDF-C reference. Its
selected science/coordinate chunks and all variable metadata required for open
were captured and independently checked through range-backed h5py. This local
control does not establish remote NetCDF-C support or instrument kernel reads.

## Scientific, fault and resource gates

Run in the same pinned HDF5 reference environment:

```sh
python bench/swot_wse.py science
python bench/swot_wse_axes.py science
python bench/swot_wse_axes.py faults
python bench/swot_wse_axes.py resources
```

The real-product matrix has 18 cells: three selections, full/experimental
selective index strategies, and exact/4/16 KiB metadata policies. Every WSE
stored-value bit, validity bit, coordinate and interpreted metadata value is
compared with h5py; WSE is also compared with NetCDF-C. Separately executed
NetCDF-C workers compare complete WSE and x/y hashes and reproduce the summary
with all automatic masking/scaling disabled. Only finite, non-fill values enter
that summary. No SWOT quality flag is applied.

The axis matrix checks all currently admitted coordinate types in both byte
orders, raw/shuffle-DEFLATE/Fletcher32 pipelines, descending axes, positive
strides, skipped chunks, ragged final chunks and the inherited compact path.
Contiguous paths are covered by the inherited HDF5 matrix. Synthetic WSE data
include packing, CF fill, signed zero, infinities and a noncanonical NaN payload.
Fault gates cover selected/unselected coordinate holes, finite CF markers,
nonfinite coordinates, implicit-fill ambiguity, compressed corruption/truncation,
expansion, invalid chunk/index pointers, modern-index rejection, malformed
dimensionality, exact data/coordinate mutation and terminal cancellation.

Coordinate preparation is synchronous, one chunk at a time. Its declared
reservation includes existing prepared state, metadata, encoded buffers,
decoded/filter working planes and selected coordinate storage. The preparation
peak is reported separately from stream-scheduler reservations. Cancellation
uses the existing stream lifecycle after synchronous preparation; blocking
range calls retain the existing request timeout/cancellation boundary.

Resource gates consume 1 and 64 MiB of Float32 output at 4/8 MiB working budgets.
Huge native coordinate chunks and encoded extents fail admission before payload
allocation/read. Native process RSS includes runtime/inspection overhead and is
not capped by the working reservation. Caller retention is outside that budget.

## Inherited gates

```sh
python bench/swot_wse_regressions.py matrix
python bench/swot_wse_regressions.py hdf5-science
python bench/swot_wse_regressions.py hdf5-faults
python bench/swot_wse_regressions.py hdf5-selective-science
python bench/swot_wse_regressions.py hdf5-selective-faults
python bench/swot_wse_regressions.py hdf5-selective-extended
python bench/swot_wse_regressions.py hdf5-selective-extra
python bench/swot_wse_regressions.py hdf5-resources
python bench/swot_wse_regressions.py coordinates
```

The Rust matrix covers default/no-default/native-cog/native-hdf5/all-features
tests and all-target warning-denying Clippy, plus the unchanged vendor library's
269 unit tests and all-target Clippy. Golden fixtures can be recreated with
`python bench/hdf5_golden.py tests/data/hdf5`.

Build inherited format consumers:

```sh
cargo build --release --locked --no-default-features \
  --features native-cog,native-hdf5,experimental-output \
  --bin tile-ripper --example zarr_subset_oracle --example native_cog_oracle \
  --example remote_zarr --example durable_output_candidate \
  --target-dir target/hdf5-regression
cargo build --release --locked --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/hdf5-regression
```

Use the existing `prototype-output/zarr-env/bin/python` environment for
`bench/swot_wse_regressions.py` actions `remote`, `cf`, `cog-stream`, `checks`,
and `faults`. Use the preserved v3 environment at
`prototype-output/zarr-v3-input/reference-env/bin/python` for `v3-checks` and
`v3-faults`. Their original documented dependency freezes still apply. No
scientific reference packages were substituted in those preserved environments.
New receipts go under `docs/swot-wse/inherited`; prior milestone reports remain
unchanged.
The coordinate audit reuses the existing sealed GOES mirror without provider
traffic, repeats six full/selective real-GOES selections, and independently
checks the inherited 123 coordinate cases through NetCDF-C.

The dgov plan was structurally compiled with `dgov compile
.dgov/plans/swot-wse --dry-run`. Dispatch validation requires a provider
configuration absent from this checkout. Execution and verification ran
directly in the reused worktree; no agents were dispatched.

## Benchmark and derive reports

After builds and correctness gates finish, run the following serially in the
pinned HDF5 reference environment, without concurrent compilation or validation:

```sh
python bench/swot_wse.py benchmark --repeats 3
python bench/swot_wse_summary.py
```

This measures fresh native and isolated mature-reader processes. h5py controls
use exact and bounded 16 KiB mirror ranges; netCDF-C is a separate local-file
control. Native exact/4/16 KiB metadata batching is tested with full and
experimental selective traversal at zero and 20 ms added response delay.
CPU, RSS, preparation, first result, completion, GET/HEAD counts, duplicate
ranges/bytes, page overfetch and reservations are retained. Original provider
capture costs and per-operation loopback response bytes are distinct.

Native full traversal remains the default. Selective traversal cannot validate
skipped subtrees; this product's three relevant indexes are flat and all are
visited under either strategy. No upstream issue/PR, merge, publication,
release, deployment, ChronoZarr change, modern index family, reprojection or
new file-output system belongs to this milestone.
