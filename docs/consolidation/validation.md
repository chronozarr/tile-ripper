# Repeatable candidate validation

Run commands from the source root. Results use new directories and cannot
overwrite an earlier run. Cargo dependencies must first be available locally:

```sh
cargo fetch --locked
rustup component add --toolchain 1.92.0 rustfmt clippy
python3 scripts/validate.py --toolchain 1.92.0 --output prototype-output/consolidation/rust-1.92-new
```

The local command uses no provider data. It checks documentation links and the
qualified Wave 4 source hashes, formatting, Clippy, seven feature builds, both
vendored test suites, the standalone consumer and Python syntax. Each check has
a log and hash in `report.json`. The feature builds are default GDAL, Zarr-only,
native COG, native HDF5, complete native, Icechunk, and all features. Install system GDAL
for this full matrix. It uses two Cargo build jobs and serial Rust tests to avoid
the resource/deadline failures recorded during earlier parallel runs.

Intentional implementation follow-ups can select a new exact source-hash
receipt with `--source-receipt PATH`; the default remains the original Wave 4
receipt. See the [acquisition review follow-up](../review-hardening/workflow.md).
The selector still verifies all listed hashes and does not replace independent
scientific replay or provider-access qualification.

Historical links into ignored `prototype-output/` captures and the compressed
acquisition-engine raw logs are recorded as local evidence links. Their absence
from a clean source checkout does not fail the source-link check. Frozen science
replay checks the separate capture prerequisites explicitly. The owner-approved
Apache-2.0 Cargo metadata addition is recorded in `license-decision.json`; it does
not replace the qualified scientific source hashes or change production dependencies.

The baseline profiles still support Rust 1.91.0. The optional Icechunk dependency
requires 1.91.1, so run the full matrix above with 1.92.0. To exercise the baseline
minimum without the Icechunk feature:

```sh
cargo +1.91.0 test --locked --no-default-features --all-targets -- --test-threads=1
cargo +1.91.0 test --locked --all-targets -- --test-threads=1
```

Ignored tests include subprocess helpers, foreign-UID permission controls,
real-product probes and dedicated HTTP/HTTPS fixture tests. Their ignore messages
name the owning harness. A green Cargo result does not assert those ran. Detailed
archive access, transfer-resume and inherited transport controls retain their
separate milestone receipts and commands. Keep permission-sensitive tests under
an ordinary user, including in Linux containers.

## Independent frozen scientific replay

The maintained replay command reuses the independent h5py/HDF5-C, NetCDF-C,
PROJ/GEOS, Zarr-Python and xarray assertions from Waves 3 and 4. It writes copies
of reports and generated requests into a new sandbox; original milestone JSON,
examples and sealed capture manifests are preserved. Python references reject
external HTTP during replay. Sparse sources are incomplete files: guarded reads
reject uncaptured holes, and all captured extent hashes are rechecked.

```sh
cargo +1.92.0 build --locked --release --no-default-features \
  --features native-cog,native-hdf5,authenticated-http \
  --bin tile-ripper --example array_faults --example array_product
uv run --no-project --python 3.12 --with-requirements docs/products/wave4/reference.txt \
  python scripts/science-replay.py --captures prototype-output \
  --output prototype-output/consolidation/science-replay-new
```

Required capture directories below `--captures`:

| Wave | Sealed mirrors and adjacent inspection files |
| --- | --- |
| `wave3` | `goes-C13`, `goes-C01`, `smap/global`, `nisar`, plus `C13-inspection.json` and `C01-inspection.json` |
| `wave4` | `nisar-geographic`, `nisar-additional`, `goes-m1-c02`, `smap-polar`, plus `goes-inspection.json` |

Each mirror needs `source.nc`, `manifest.json` and the original adjacent inspection
metadata. A source archive alone does not include these ignored captures. Preserve
the local mirrors or explicitly acquire fresh bounded captures following the
[Wave 3](../products/wave3/qualification.md) and [Wave 4](../products/wave4/qualification.md)
procedures in a separate research checkout. Protected capture needs an explicit
private netrc file. Do not run historical harness entrypoints directly in the
candidate checkout: they write their original report locations.

The replay covers 19 Wave 3 science cases, 18 Wave 4 cases, 21 + 40 lifecycle
controls, all 1,229,401,152 centers in the original NISAR grid and all four million
northern-SMAP centers. It is separate from provider authentication, catalog
availability and throughput measurements.

## Explicit live smoke test

The anonymous ERA5/GOES commands in the [scientist trial](../alpha/human-trial.md)
are the small live smoke test. Use a new output directory on every run and reopen
the outputs. They do not use credentials or replace independent source equality.
Protected live checks remain opt-in, with exact per-archive policies and private
netrc paths; consult each qualification report. Never put credentials, signed
resources or private request files into a report or source snapshot.

For the complete automated source-only installation trial, first commit the
candidate, then run:

```sh
uv run --no-project --python 3.12 --with-requirements docs/products/wave4/reference.txt \
  python scripts/source-trial.py --toolchain 1.92.0 \
  --output prototype-output/consolidation/source-trial-new
```

This archives tracked files, extracts to a fresh source tree, builds into an empty
target, installs to a private prefix, runs both live requests with output reopening,
and uninstalls that executable. It preserves the archive hash and command logs.
Cargo's shared registry cache and existing system compiler are reused; this is
not fresh-machine provisioning or a human usability result.

Linux ARM64 portability can be repeated with the existing `rust:1.92-slim` image
and `bench/wave4_linux.sh`, mounting source and the Cargo registry read-only. Its
temporary target and UID-1000 test user are inside the disposable container.
