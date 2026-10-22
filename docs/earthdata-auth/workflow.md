# Explicit authenticated native acquisition

The qualified target is the exact Version D PGD0 product and 128×128 window in
[the request](../swot-quality/request.json). See the [live decision gate](report.md)
for measured bytes, object/version binding, scope and scientific validation.

Build the optional transport with the native HDF5 reader:

```sh
cargo build --release --locked --no-default-features \
  --features native-hdf5,authenticated-http --example swot_quality
```

The executable uses neither system libhdf5, GDAL nor Python. The caller must
already have authorized Earthdata access and an existing private, caller-owned
netrc file with an explicit `urs.earthdata.nasa.gov` entry. Credentials stay in
that file. Do not put credential values in request JSON, command arguments or
logs. The reader supports machine/login/password/account fields, quoted or
escaped values and comments. It rejects default-only credentials, macros,
symlinks, public permissions, oversized files and malformed input. The private
netrc mechanism currently requires Unix ownership/permission checks; it fails
closed on other platforms.

The example opts in only when given the credential **file path**:

```sh
TILE_RIPPER_AUTH_NETRC="$HOME/.netrc" \
  target/release/examples/swot_quality \
  docs/swot-quality/request.json docs/swot-quality/cookbook-policy.json stream
```

The CLI's ordinary acquisition commands and `swot::prepare` stay anonymous.
Library callers pass a caller-owned session explicitly:

```rust,ignore
use std::sync::Arc;
use tile_ripper::{authenticated_http::AuthenticatedHttp, cog::LayoutCache, swot};

let session = Arc::new(AuthenticatedHttp::earthdata(
    &canonical_product_url,
    credential_file_path,
)?);
let prepared = swot::prepare_authenticated(
    &request,
    swot::Policy::cookbook(),
    &mut LayoutCache::default(),
    session.clone(),
)?;
let mut stream = prepared.into_stream()?;
for tile in stream.by_ref() {
    consume_provisional(tile?)?;
}
let completion = stream.finish()?;
let safe_session_counts = session.statistics();
```

Values remain provisional until `finish()` succeeds. A failed source check,
unsafe redirect, authentication failure or cancellation prevents completion.
`statistics()` is cumulative over deliberate reuse of this session. A session
is confined to its configured canonical object; it cannot authenticate another
asset, format or locator. Create a new session to isolate another request's
counts. Runtime/TLS allocation overhead is separate from working reservations.

`AuthenticatedHttp::new(Config, netrc_path)` accepts a separately qualified
exact-object policy. `Config` contains public canonical/login/resource origins,
separate HEAD/GET resource paths, directed redirect rules with exact destination
paths, and optional additional verified CA roots. It contains no credential
values. The Earthdata factory supplies the observed SWOT Raster D profile.
The example's `TILE_RIPPER_AUTH_POLICY` points to a public policy JSON file;
`TILE_RIPPER_AUTH_CA` adds a verified CA PEM for the local HTTPS fixtures. Neither
setting disables TLS verification. These overrides do not authorize wildcard
redirects or credentials at additional origins.

## Offline transport and scientific gates

The independent reference environment needs requests, h5py, netCDF4 and numpy.
The versions actually used are embedded in the receipts. The fixture command
also needs OpenSSL to create its local TLS certificate, and Cargo for the
concurrent source-conflict test:

```sh
python bench/earthdata_auth.py fixtures
cargo test --locked --all-features
cargo test --locked --no-default-features --features native-hdf5,authenticated-http
cargo clippy --locked --all-features --all-targets -- -D warnings
ruff check bench/earthdata_auth.py
```

`fixtures` creates independent local archive, login, signed-resource, trap and
control endpoints. It has no public-provider requests. Every successful subset
is compared with independently executed h5py and NetCDF-C. Failed operations
must have the expected explicit reason, and synthetic credential/cookie/signature
sentinels must not appear in diagnostics or provenance. Raw fixture artifacts
and provisional arrays remain in ignored `prototype-output/earthdata-auth/`.
Compact receipts are in this directory.

The controlled regression environment is intentionally separate from the native
binary. The historical pinned v3 reference requirements were unavailable from
the package index during this run; the v3 gates used a fresh Python 3.12 Zarr 3
environment instead. Its actual versions are recorded in the generated fixtures
and regression receipt. The five-layer references used the installed independent
h5py and NetCDF-C builds, with their versions recorded per comparison.

## Bounded live acceptance

This is an explicit NASA operation; it performs one native subset acquisition.
It requires the previously sealed mirror and manifest only for independent
offline comparisons. Missing reference material is an error, with no recapture
or full-download fallback.

```sh
python bench/earthdata_auth.py live --metadata exact
```

`--metadata page4k` and `--metadata page16k` select the existing bounded metadata
strategies. The gate verifies the mirror identity and captured hashes before
starting the native executable, pins the expected provider ETag, then compares
every returned stored bit, mask, coordinate and derived statistic against both
reference readers. It records actual native HTTP counters, all response-body
bytes, timings, working reservations, RSS and the executable hash. The native
executable is the only component fetching from NASA in this command.

The qualified live runs are already retained. Use local fixtures for repeated
optimization. Timing differences between NASA and the bounded mirror reflect
network/provider behavior and authentication/signing round trips; they do not
establish a general acquisition-engine performance lead.
