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

## STAC milestone

The existing `stac` 0.17.6 and `stac-io` 0.3.4 libraries provide STAC types,
query serialization and pagination. Tokio 1.48.0 supplies their required
runtime; it does not replace GDAL's I/O/decoder. These are three necessary
direct dependencies in the same package; no workspace crates, DuckDB, new
codec stack or extra geometry library were added. Upstream STAC/CQL/TLS
support brings a substantial transitive dependency tree (325 registry package
versions in the final lock). This compile/supply-chain cost is explicit.

`stac-dependency-inventory.json` records final publication dates/licenses and
checksums from the same primary registry API. Nineteen recently resolved
versions were downgraded to pre-cutoff compatible releases (including paired
Pest/TLS/derive dependencies); all final registry versions predate 2026-10-11.
The `bench/check_dependency_ages.py` inventory helper reproduces the check.
It is still an age/license inventory, not a vulnerability certification.


## Zarr milestone

One direct dependency, zarrs =0.22.10, with only filesystem and gzip features,
adds the maintained Zarr codec/storage implementation. No new workspace crate
or downloader. Cargo.lock contains 372 registry package versions (47 more than
the STAC milestone). zarrs_filesystem was pinned transitively to 0.3.13 to keep
the existing seven-day quarantine; all locked registry versions predate the
October 11 cutoff. `zarr-dependency-inventory.json` records publication dates,
licenses and checksums. The Python oracle uses an isolated, frozen environment
in `bench/zarr-requirements.txt`, installed with the same uv cutoff. Existing
study and ChronoZarr environments were not changed. This remains an age/license
inventory, not a vulnerability certification.


## Unified lifecycle milestone

Cargo.toml/Cargo.lock are unchanged: the same 372 locked registry versions and
host GDAL are used. Scoped native diagnostics bind GDAL's existing CPL
thread-local handler stack, without a new dependency or global library handler.
Python benchmarks reuse the existing isolated environments and locked/frozen
requirements. No codecs, downloader, workspace crate or remote Zarr backend
were introduced.


## Remote HTTP Zarr milestone

reqwest =0.13.5 is now a direct dependency with its blocking feature. That exact
package and feature stack were already locked through STAC dependencies: no new
registry packages or versions are added. HTTP transport uses reqwest; Zarr
serialization and decoding remain zarrs. Python uses the same frozen Zarr oracle
environment and standard-library HTTP, with no new installed packages.


## First conventional geographic Zarr profile

zarrs remains pinned to 0.22.10; its established Blosc feature is enabled. That
feature brings seven native build/library packages, including the other codec
libraries enabled by zarrs' Blosc feature even though this profile admits only
LZ4. No workspace crate or downloader is added. flate2 1.1.10 was already locked
and is now direct for fixed-output-buffer gzip decoding. Existing package
versions are unchanged. `cf-zarr-dependencies.json` records the newly locked
packages, checksums, license expressions and upstream publication dates. The
Python oracle environment is unchanged. Payload allocation tests do not certify
the native libraries as vulnerability-free.
