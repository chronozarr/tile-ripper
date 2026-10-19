# Candidate verification — 0.1.0-alpha.1

This is developer-run verification, not a human trial or a second independent
newcomer result. Runtime source/test/example inputs are unchanged from the prior
readiness candidate; only project version/description and preparation materials
changed. [candidate-inputs.json](candidate-inputs.json) records the 42 file hashes
shared by the worktree and the temporary clean source used for installation.
Registry versions/checksums in Cargo.lock are unchanged; only the project version
changed. The same metadata was used for both platform checks.

The pinned installation command from human-trial.md was run with empty target
and isolated prefix, Rust 1.92.0, `--locked --jobs 1`. Homebrew GDAL 3.12.4 was
already installed on macOS arm64. Ubuntu 24.04 arm64 started in a container
without Cargo/GDAL, installed apt prerequisites and Rust, and copied source
without the host .alpha build/reader/output directory. No ignored fixture was
required by acquisition. An optional clean Python 3.13 environment used the exact
existing fully pinned bench/zarr-requirements.txt from the guide. These are source/empty-target tests, not
bit-for-bit reproducible binary claims or fresh physical-machine tests.

The completed command receipts and results are recorded below after execution.
No performance comparison or human completion-time claim is made. The template
collects actual first-human setup/fetch times and interventions when that trial occurs.

| Check | Final candidate result |
| --- | --- |
| macOS arm64 source install | Rust 1.92.0 / system GDAL 3.12.4, empty target, 6m 17s serial release build; version listing 0.1.0-alpha.1 |
| macOS S2 / ERA5 | Both plans/fetches complete; gdalinfo opens both 91×113 S2 TIFFs; pinned Python 3.13/xarray reader opens 2×4×3 ERA5 with Kelvin units and actual values |
| Ubuntu 24.04 arm64 source install | Fresh apt GDAL 3.8.4/Rust 1.92.0, 5m 32s serial release build; version listing 0.1.0-alpha.1 |
| Ubuntu S2 / ERA5 | Both plans/fetches complete; gdalinfo opens S2 outputs; both manifests complete; Ubuntu-produced ERA5 also opens in the pinned macOS reader |
| Uninstall | Cargo uninstall removed each isolated executable; no system GDAL removal |
| Prior scientific/fault tests | 37 Rust tests on each platform, 28 HTTP cases and 14 scientific-profile cases retained as prior unchanged-runtime evidence; not redundantly rerun for version/docs-only changes |
| Proposal checks | Local guide links resolve; lockfile registry entries unchanged; no root project LICENSE/Cargo license field; recovered notice texts are original text, not bot-challenge HTML |

Receipts: [macOS install](source-install-verification/macos-install.txt),
[macOS trial](source-install-verification/macos-trial.txt),
[pinned reader installation](source-install-verification/reader-install.txt),
[Ubuntu install/trial](source-install-verification/ubuntu-install-trial.txt),
[Ubuntu Zarr reader check](source-install-verification/ubuntu-zarr-open.txt).
Build durations exclude platform/tool installation and human activity and are not
performance comparisons or human time-to-first-success measurements.

The commands in human-trial.md were verified. The optional reader uses the
existing full requirements pins instead of a fresh unconstrained transitive stack.
The receipt's Ubuntu output read took place on macOS after copying only the tiny
completed Zarr subset; no claim of a separately provisioned Ubuntu xarray reader.
Temporary test source/build/reader/data trees and the Ubuntu container were removed
after capture. The newly installed Rust 1.92.0 macOS toolchain remains available;
no default toolchain or system GDAL installation was replaced for this trial.
