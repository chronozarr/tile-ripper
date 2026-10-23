# Authenticated transfer resume

This is an implemented, tested CLI and Rust workflow for the qualified SWOT
Raster Version D PGD0 selection. See [the contract](decision.md) and
[the decision gate](report.md). It persists source bytes. Each invocation starts
a new scientific stream at tile zero; a tile delivered before interruption can
be delivered again. Discard the failed attempt's consumer calculation.

Build the normal executable (no GDAL or system HDF5 linkage):

```sh
cargo build --release --locked --no-default-features \
  --features native-hdf5,authenticated-http --bin tile-ripper
```

Use an existing private, caller-owned regular netrc file with an explicit
`urs.earthdata.nasa.gov` machine entry and authorized Earthdata access. Supply
only its file path. The normal CLI never reads credentials because `.netrc`,
`NETRC`, or `TILE_RIPPER_AUTH_NETRC` happens to exist. Authentication requires
both explicit `--auth` and `--netrc` flags; request JSON contains no credentials.

From this checkout, the tested command sequence is:

```sh
# 1. Inspect the five-layer selection and explicit quality policy.
target/release/tile-ripper plan docs/swot-quality/request.json \
  --quality-policy docs/swot-quality/cookbook-policy.json \
  --auth earthdata --netrc "$HOME/.netrc" > inspection.json

# 2. Start a new acquisition with bounded private source-byte persistence.
mkdir -m 700 transfer-checkpoint
target/release/tile-ripper stream docs/swot-quality/request.json \
  --quality-policy docs/swot-quality/cookbook-policy.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint "$PWD/transfer-checkpoint" --values > attempt.ndjson

# 3. Press Control-C while the stream is running. This attempt is provisional.

# 4. Invoke the same command again, using a fresh output and consumer calculation.
target/release/tile-ripper stream docs/swot-quality/request.json \
  --quality-policy docs/swot-quality/cookbook-policy.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint "$PWD/transfer-checkpoint" --values > resumed.ndjson
python examples/consume_swot_ndjson.py resumed.ndjson
```

The selection is small and may finish before a manual interrupt. The local test
harness below deterministically interrupts metadata preparation, science reads,
partial bodies and provisional delivery. A completed invocation can also be
replayed from its checkpoint, while still performing fresh online authorization
and source validation. The consumer rejects streams lacking a successful native
completion record and emits its calculation only after that record. Check the
native process exit status when using it in a pipeline. Never concatenate
attempt files or append restarted results to an old calculation.

`stream --quality-policy` emits NDJSON: one `prepared` inspection, ordered
provisional `tile` records, then `complete` only after successful `finish()`.
`--values` adds exact native bits as base64 little-endian arrays, each source
mask, explicit coordinates and variable/type identity. Each tile also contains
the separate derived quality mask. Float32 bits, including fill/NaN payloads,
are preserved by the encoding. Without `--values`, it emits descriptors and the
derived mask. The example consumer supports the qualified finite-WSE summary
and bitwise policies and independently recalculates the completion summary.

The defaults are 64 MiB of logical managed-file bytes, an 8 MiB range limit,
2,048 entries, one active write and a conservative 2,162,688-byte checkpoint
working reservation, in addition to the existing 1 MiB authentication
reservation. This reservation is charged to HDF5 admission. Range bodies reuse
the existing admitted encoded-data buffers. Override disk/count bounds with:

```sh
# Add these flags to the same stream command when needed:
# --checkpoint-mib 64 --checkpoint-entry-mib 8 --checkpoint-entries 2048
```

The maximum supported entry size is 16 MiB and entry count is 16,384. Index
reservation scales as `512 * max_entries + 1,114,112` bytes. Exceeding a bound,
running out of space, or encountering an unsafe file fails the acquisition;
there is no eviction or silent uncached fallback. Filesystem blocks, inodes and
directory overhead are outside logical-byte accounting; measured RSS is
separate from working reservations. Only macOS/Linux atomic no-replace record
publication is implemented; validation here ran on macOS.

The caller directory must be private and owned; it can contain unrelated files.
Managed files live in `tile-ripper-checkpoint-v1/`. The store holds an exclusive
lock through preparation, execution and completion. Close every prepared clone
and stream before opening the directory again. Unknown managed files fail
closed and are preserved. Known incomplete temporary files are discarded only
inside this namespace; unrelated caller files are never cleaned up. Startup
checks every retained record header/length; every reused body is hashed before
delivery. A corrupted entry, changed source generation, selection, coordinate
path/stride, quality definition/policy or access policy requires a fresh
directory or correction of the underlying error. No reset/overwrite flag exists.

Worker count, metadata page size, traversal policy, buffer budget and tile edge
are execution settings. They do not change the scientific binding, but new read
boundaries can cause exact-range misses. Cached reads must also fit the current
transport/decoder admission; a larger old checkpoint does not bypass a smaller
current buffer budget. ETags identify provider versions, not whole-object hashes.

Rust callers use the existing explicit session API with the checkpoint variant:

```rust,ignore
use std::sync::Arc;
use tile_ripper::{authenticated_http::AuthenticatedHttp, checkpoint::Options,
                  cog::LayoutCache, swot};

// Create a fresh, unused session for each preparation, including resume.
let session = Arc::new(AuthenticatedHttp::earthdata(&canonical_url, netrc_path)?);
let prepared = swot::prepare_authenticated_checkpointed(
    &request, swot::Policy::cookbook(), &mut LayoutCache::default(),
    session, Options::new(checkpoint_directory),
)?;
let mut stream = prepared.into_stream()?;
let mut calculation = new_calculation();
for tile in stream.by_ref() {
    calculation.consume_provisional(tile?)?;
}
let report = stream.finish()?;
calculation.commit(report)?;
```

The application supplies its own calculation/transaction. The ordinary
`acquisition::prepare_authenticated_checkpointed` variant similarly binds its
native HDF5 request; `swot` additionally binds the aligned quality policy and
interpreted flag definitions. An already-used session is rejected for checkpoint
preparation. The no-checkpoint APIs remain available. Authentication and
checkpointing of COG/Zarr, bearer tokens, arbitrary Earthdata products, offline
success, partial-dataset recovery and exactly-once side effects are outside this
milestone. There is no HDF5 output writer.

Reproduce the local gates with the existing independent reference environment
(Python, h5py, netCDF4, numpy, requests, psutil and OpenSSL):

```sh
python bench/transfer_resume.py fixtures
python bench/transfer_resume.py matrix
cargo test --locked --all-features
cargo test --locked --no-default-features --features native-hdf5,authenticated-http
cargo clippy --locked --all-features --all-targets -- -D warnings
ruff check bench/transfer_resume.py bench/earthdata_auth.py examples/consume_swot_ndjson.py
```

`fixtures` and `matrix` contact only local HTTPS endpoints and verified pinned
mirrors; uncaptured holes are refused. The matrix separates exact/4 KiB/16 KiB
metadata strategies and includes interrupted partial responses, fresh restarts,
uninterrupted checkpoint writes, and failed-login controls. The `held-out`
command rechecks the existing bounded native capture offline; missing stage
files are an error. Its NetCDF-C result remains an open qualification gate.
`python bench/transfer_resume.py live` explicitly performs one fresh native
interrupt/resume acceptance against NASA and requires the already verified
reference mirror; it never recaptures a full product. Prefer local gates for
repeated work. File fsync and atomic renames support process-crash recovery;
these tests do not establish power-loss durability.
