> Historical gate. See the [private-alpha closeout](closeout.md) for current
> source-distribution scope, approvals and the human tester procedure.

# Private alpha decision — 2026-10-19

Candidate based on `5187801`, `codex/acquisition-planner`. Recommendation:
**more work required before distributing a licensed private alpha**. The docs-only
fresh-context agent completed the scientific workflows, but project ownership and
exact upstream notices remain unresolved. Stop at this demonstrated release blocker;
do not add formats or undertake binary packaging to bypass it.

## Deliverables and decision

- [Licensing recommendation and audit](licensing.md): recommend Apache-2.0 after
  owner rights confirmation; dual MIT/Apache is viable for owned code, not a way
  to erase third-party terms. Complete locked-tree and native-link evidence supplied.
- [Installation and diagnostics](../installation.md): source checkout plus locked
  Cargo install and system GDAL is the simplest maintainable channel. No packages
  or binary bundles published. GDAL is required even for Zarr-only use.
- [Newcomer findings](newcomer-trial.md): separate fresh-context agent installed,
  planned, fetched, opened Sentinel-2 and ERA5, and explained metadata/provenance.
  Existing native prerequisites/cache reused. No independent human trial occurred.
- [Exact alpha contract](contract.md) and [three scientific examples](examples.md).

## Current validation

`cargo test --locked`: 37 passed, 2 ignored controlled HTTP tests exercised through
separate harnesses; see [test log](rust-tests.txt). No Rust implementation changed.
The [28-case controlled HTTP suite](http-checks.json) passed, covering changed
metadata/coordinates/chunks, object appearance, conditional versions, cancellation,
corruption, decompression bounds, transient/permanent errors, interrupted HTTP
bodies, rejected redirects/credentials and output cleanup. This rerun used local
fixtures in an isolated temporary source tree, not old result files.

[CLI error probes](cli-errors.json) cover unsupported format, bad bounds, duplicated
selection, mixed request/flags, existing destinations and unknown options. Parser
validation order can report a missing required selection before an unsupported
format; complete requests produce the format diagnostic. [Repeated direct STAC
plans](reproducibility.json) had identical plan objects for the unchanged catalog;
full JSON timings differ. [Direct STAC inspection](direct-stac-plan.json) records
that actual query. Newcomer fetches used JSON requests. A separate [direct ERA5 fetch](direct-era5-report.json)
passed, along with direct plan, --dry-run and --buffer-mib 32.

[Actual SIGINT test](sigint.json): process exit −2 during output, partial directory
remained, no complete manifest. This matches documented interruption limits; do
not claim automatic signal cleanup. Test removed the exact temporary tree afterward.
Caught execution errors clean up outputs; the harness checks this. No resume.
[Missing-runtime diagnostic probes](loading-errors.json) passed on a temporary
modified binary copy; system libraries were untouched.
The [14-case paired scientific profile suite](profile-checks.json) also passed,
including reversed axes, missing cells, bad filters/units, irregular grids and retries.

Public outputs opened successfully in the newcomer trial: [ERA5 report](newcomer-era5-report.json),
[Sentinel report](newcomer-sentinel-report.json). gdalinfo checks S2 structure and
xarray checks readable ERA5 values/axes/units; these are not an independently
rerun all-value source oracle for these public fetches. Previous public GDAL/xarray
oracles remain historical evidence in ../external-usability-decision.md.
There is no demonstrated consistent speed advantage over tuned GDAL/xarray.

macOS arm64 clean-target/source installation succeeded with Rust 1.92.0 and
GDAL 3.12.4. Host provisioning was reused. Runtime recursive linkage is recorded,
not assumed portable. Ubuntu 24.04 arm64 container clean source install and public ERA5 plan/fetch
passed with Rust 1.92.0 / system GDAL 3.8.4; see [install log](linux-install.txt)
and [public-fetch log](linux-validation.txt). Default parallel debug testing hit
linker signal 9 (resource exhaustion). Serial `cargo test --release --locked -j 1`
then passed 37 tests (2 controlled tests ignored), and Cargo uninstall removed
the isolated executable; see [Linux tests/uninstall](linux-tests-uninstall.txt). Intel macOS, Linux x86_64 and Windows remain unverified; a Linux container
is not a fresh physical machine test.

## Actual blockers and next gate

1. Confirm project ownership/authority, historical code provenance and intended
   license scope; apply approved root license(s) and Cargo metadata only afterward.
2. Recover exact missing upstream notices (including blosc-src's embedded C code)
   and prepare a reviewed attribution bundle for the actual source/binary scope.
   Current system GDAL must not be redistributed as an assumed permissive bundle.
3. Before calling usability independently validated for scientists, run one human
   newcomer trial on a machine without the developer setup. The agent trial passed,
   but does not establish human completion time or fresh-machine provisioning.

GDAL-dependent installation, unstable API, source-mutability limits and signal
leftovers are explicit alpha limitations, not promises to solve with new systems.
Binary redistribution, conda recipe, Windows, Intel macOS and broad Linux support
are deferred; they need not block a narrowly declared source alpha after licensing.

Proposed version: **0.1.0-alpha.1** (not applied or tagged). Manual checklist:

- [ ] Resolve rights/license scope and install approved license(s)/Cargo fields.
- [ ] Complete exact third-party software and provider attribution notices.
- [x] Empty-target source install and two scientific newcomer workflows on macOS arm64.
- [x] Ubuntu arm64 clean container install, bounded ERA5 fetch, serial release tests and uninstall.
- [x] Controlled source-version/failure/cleanup and actual signal-interruption checks.
- [x] Document supported profiles, native outputs and no consistent speedup claim.
- [ ] Human scientist repeats README tasks and explains a manifest; record repairs.
- [ ] Freeze an approved revision; repeat bounded examples and platform checks there.
- [ ] Review lockfile/security advisories and native runtime status for that revision.
- [ ] Approve version/tag and intended private recipients; publish only with approval.

Temporary fixture trees, newcomer outputs/install/build trees, Linux containers and
the temporary validation image were removed after evidence capture. No merge,
deployment, domain changes, registry package, binary upload or release tag.
