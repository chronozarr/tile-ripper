# Prototype dependency scope

Direct Cargo dependencies use exact versions; Cargo.lock pins transitive
versions and checksums. Use `--locked` for builds and tests. The existing study
uv manifests/locks were preserved, and their already installed Python
environment was reused without adding dependencies.

The study records a seven-day uv quarantine and an unresolved Cargo policy.
For this proof, registry publication dates were checked against the same
conservative October 11 cutoff relative to October 18, 2026. Two newly resolved
transitives were downgraded in Cargo.lock: syn 3.0.9 → 3.0.7 and unicode-ident
1.0.27 → 1.0.26. The final 25 registry packages all predate that cutoff.
The exact versions, publication dates, licenses and lock checksums are in
`dependency-inventory.json`, from the primary crates.io version API
`https://crates.io/api/v1/crates/{name}/{version}`.

This is a version/age/license inventory, not a security certification or a
cargo-vet/cargo-deny audit. Host GDAL/PROJ/libtiff and the Python environment
remain separately managed native dependencies. No packages were published.
