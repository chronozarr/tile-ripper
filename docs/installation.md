# Installation and release preparation

Tile-Ripper is pre-alpha source software, not a published package or standalone
binary release. Build from the repository checkout with Cargo.lock. The current
verified environment is macOS arm64, Rust 1.92.0, GDAL 3.12.4. This is a tested
combination, not a claimed minimum Rust version or cross-platform certification.

## Build locally

Install Rust/Cargo using your normal trusted installer. On macOS/Homebrew:

```sh
brew install gdal pkg-config
cargo build --release --locked --bin tile-ripper
./target/release/tile-ripper --help
# Optional local installation; no registry publication:
cargo install --path . --locked
```

The final command installs into Cargo's bin directory; ensure it is in PATH.
It compiles locally and still needs GDAL at runtime. Python is not an acquisition
dependency. Tests comparing against xarray/rasterio use separate Python environments.

On Debian/Ubuntu the usual native prerequisites are `build-essential`, `pkg-config`,
`cmake`, `libgdal-dev` and `gdal-bin`; the alpha follow-up tested an Ubuntu 24.04 arm64 container with GDAL 3.8.4.
The locked native codec dependencies compile C/C++ code, so a compiler toolchain
is required. GDAL must provide GTiff, HTTP/curl, PROJ and GEOS support for the
advertised STAC workflow. A header/library mismatch or unfindable GDAL should be
resolved in the native environment before building Rust. Avoid mixing Homebrew
and Conda GDAL prefixes. Windows and Intel macOS remain unverified.

Official references: [Cargo local installation](https://doc.rust-lang.org/cargo/commands/cargo-install.html),
[GDAL package options](https://gdal.org/en/stable/download.html),
[Rust GDAL binding build requirements](https://github.com/georust/gdal#version-support).

## Portability assessment

This binary dynamically links Homebrew's `libgdal.38.dylib` under
`/opt/homebrew/opt/gdal/lib/`, as recorded in `external-usability-build.json`.
GDAL has further native dependencies and data files. Copying the executable to
a machine without that runtime is **not** a supported installation method.
A recipient with an appropriate GDAL runtime would not need Rust development
headers to run a prebuilt binary, but a portable bundle has not been constructed
or tested. Even Zarr-only commands currently link GDAL because the unified crate
depends on it. Static linking is not a small packaging switch: GDAL drivers,
PROJ data, dependency licensing and platform testing require deliberate work.

The smallest maintainable initial distribution is a tagged source checkout with
`cargo install --path . --locked`, plus documented system GDAL packages. Keep
`publish=false` until metadata/licensing and dependency review are completed.
Do not adopt a multi-platform binary-release generator before runtime packaging
has been tested. No registry, GitHub release, upload or deployment occurred here.

## Proposed release checklist (manual, no publishing automation)

1. Freeze a local candidate revision; verify lockfile, licenses and security review.
2. Build from a fresh source tree and empty target directory with `--locked`.
3. Run Rust tests and the independent controlled GDAL/xarray regressions.
4. Smoke-test the three public-source examples with bounded requests and open outputs.
5. Run independent docs-only trials and then a human trial; fix instructions.
6. Verify supported OS/runtime combinations in clean environments and record linkage.
7. Review release notes and only then request approval to tag/publish artifacts.

The clean-source test here uses a Git source archive overlaid with the candidate's
changed source files, with no ignored fixture or previous target directory. Cargo's
local dependency cache and the installed native toolchain were reused. It is not
a fresh-machine installation test or a byte-for-byte reproducible-build claim.

## Private-alpha candidate diagnostics (2026-10-19)

From the checkout, run `sh scripts/check-environment.sh` before compiling. After
installation, run `sh scripts/check-environment.sh /absolute/path/to/tile-ripper`.
This catches absent tools, GTiff support and a binary that cannot load its runtime.
It is not a remote-source compatibility test. See the [current alpha gate](alpha/readiness.md).

For a reversible isolated installation:

```sh
cargo install --path . --locked --root "$HOME/.local/tile-ripper-alpha"
"$HOME/.local/tile-ripper-alpha/bin/tile-ripper" --help
# Uninstall only this prefix's Tile-Ripper:
cargo uninstall --root "$HOME/.local/tile-ripper-alpha" tile-ripper
```

Cargo's registry cache, build target, request files, explicit `--cache` files and
acquired datasets remain separate. Remove only the specific test directories you
created after inspection; uninstall does not delete scientific outputs or system
GDAL. No service, startup task or scheduler is installed.

For Ubuntu/Debian, install prerequisites before using the same Cargo command:

```sh
sudo apt-get update
sudo apt-get install build-essential pkg-config cmake libgdal-dev gdal-bin ca-certificates curl
```

Install Rust 1.92.0 through the official Rust installer if it is absent; a distro's
Cargo may be too old for this lockfile. Do not silently regenerate Cargo.lock.
Linux container validation status is recorded in the current gate, separately from macOS.
Homebrew users also need Apple's command-line developer tools (`xcode-select --install`)
if a C/C++ compiler is missing. The macOS test reused installed Homebrew GDAL;
it did not test provisioning a fresh operating system.

If gdal-sys cannot locate GDAL, inspect `gdal-config --version`,
`gdal-config --cflags` and `gdal-config --libs`. Check that the selected compiler,
headers and libraries come from the same prefix/architecture. The binding supports
`GDAL_HOME`, `GDAL_LIB_DIR` and `GDAL_INCLUDE_DIR`; consult its
[build documentation](https://github.com/georust/gdal/blob/master/README.md).
Do not point at a different installation just to suppress the error.

`Library not loaded: …libgdal…` on macOS or `libgdal.so…: cannot open shared object`
on Linux happens before Rust can print diagnostics. Use the shell check above,
`otool -L /path/to/tile-ripper` (macOS), or `ldd /path/to/tile-ripper` (Linux).
Restore a consistent system GDAL installation and rebuild Tile-Ripper after an ABI
upgrade. Do not copy/symlink a differently versioned library into place. A
`GDAL could not open asset` error can also mean missing HTTP support, TLS/network
failure, or an inaccessible object. First verify `gdalinfo --formats` and a small
local GeoTIFF; source locators are intentionally omitted from runtime errors.

Conda-forge is a future option, not a delivered Tile-Ripper package. GDAL itself
is available there (`conda install -c conda-forge gdal`), but a Tile-Ripper recipe
would need compiler variants, runtime pinning, licenses and platform CI. Mixing
Conda and Homebrew libraries is not a supported alpha setup. Prebuilt executables
and bundled GDAL are deferred: current linkage includes many transitive libraries
and data resources, with unresolved redistribution notices. No new package manager
channel is needed for this source-only candidate.
