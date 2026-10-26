# Installation and release preparation

See the [consolidation checklist](consolidation.md) for current validation and
candidate preparation. The October 19 alpha trial linked below covers its
earlier feature scope; its source-handoff instructions predate the vendored forks.

For the exact pinned `0.1.0-alpha.1` candidate instructions, isolated install/uninstall,
and two real acquisitions use [the self-contained scientist trial](alpha/human-trial.md).
[ALPHA.md](../ALPHA.md) is the short entrypoint; licensing remains pending.

Tile-Ripper is private-alpha candidate source software, not a published package or standalone
binary release. Build from the repository checkout with Cargo.lock. The default-build
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
or tested. In the default build, Zarr-only commands also link GDAL. The optional
GDAL-free builds below remove that requirement. Static linking is not a small packaging switch: GDAL drivers,
PROJ data, dependency licensing and platform testing require deliberate work.

The smallest maintainable initial distribution is a tagged source checkout with
`cargo install --path . --locked`, plus documented system GDAL packages. Keep
`publish=false` until metadata/licensing and dependency review are completed.
Do not adopt a multi-platform binary-release generator before runtime packaging
has been tested. No registry, GitHub release, upload or deployment occurred here.

## Optional GDAL-free build

The native COG/Sentinel build is opt-in. Use Rust 1.92 (tested), a C/C++ compiler,
CMake and pkg-config. On Debian the tested prerequisites were `build-essential`,
`cmake`, `pkg-config` and CA certificates, with **no** GDAL, PROJ or libtiff package.
The first Rust slim-container attempt without C++ failed on vendored Snappy;
installing the compiler toolchain resolved that requirement. This is not a
Rust-only build: existing Zarr Blosc/Snappy and TLS dependencies compile native
code. Their codecs are bundled; they do not require system libtiff or GDAL.

```sh
cargo build --locked --release --no-default-features --features native-cog
# Optional isolated local installation, preserving any existing installation:
cargo install --path . --locked --no-default-features --features native-cog \
  --root /absolute/path/to/new-native-install
# Zarr-only, with no COG reader:
cargo build --locked --release --no-default-features
```

Use the [GDAL-free Sentinel workflow](native-sentinel/workflow.md) for WGS84
polygon/date/band STAC requests and masked native chunks, or direct assets with
bounds in their exact source CRS. The native path maps WGS84 longitude/latitude
only to supported nearby Sentinel WGS84 UTM grids; it does not reproject raster
values. [GDAL-free scientific output](scientific-output/workflow.md) now supports
`fetch --gdal-free --output ./sentinel.zarr`, including actual stored UInt16
values and per-observation coordinates, masks and provenance. No Rust consumer
or Python installation is needed to create it. Zarr acquisition and native Zarr output retain their existing profiles.
GeoTIFF output requires GDAL and fails explicitly when it is disabled.

For a comparison build, `cargo build --features native-cog` includes both readers.
Use CLI `--backend native` or `--backend gdal`, or set
`TILE_RIPPER_COG_BACKEND=native` or `gdal` before library preparation and keep it
fixed for the process. An explicit CLI choice overrides that environment setting.
The dual-reader executable still links GDAL. An executable built
with `--no-default-features --features native-cog` selects native automatically
and contains no GDAL link. CLI `--gdal-free` rejects any GDAL-linked build and
selects native when compiled; it also works for the Zarr-only build. The public
Rust `acquisition::prepare_gdal_free` helper checks the same build requirement.
See [selection and Rust examples](native-http/cli.md). Do not infer linkage from
the selected environment variable; inspect the actual executable (`otool -L` on
macOS, `ldd` on Linux).

macOS ARM64 and a clean Linux ARM64 container were exercised. Linux still needs
libstdc++, libgcc_s, libm and libc; macOS needs its OS frameworks/C++ runtime.
Windows, Intel macOS, Linux x86_64, musl and WASM are unverified. The
[decision report](native-cog/report.md) records actual sizes, startup, build costs,
linkage and the science/fault/performance gates. No binary distribution is published.

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

## Public source access and trial failures

- `GDAL could not open asset`: first run the environment check and confirm
  `gdalinfo --formats` lists GTiff. Then check that the public provider is reachable
  from your network and your system CA certificates are installed. This error can
  mean native GDAL HTTP/TLS support or source access failure; it does not identify
  which one. Locators are deliberately omitted from errors.
- HTTP 403/404, unsupported redirect or public-URL rejection: use the exact bundled
  public request file and current documented endpoint. The alpha has no credential
  provider, signed-URL or redirect workaround for Zarr. Check provider availability;
  record the failing command and source ID. Do not add private tokens to feedback.
- Source version conflict: the selected metadata or objects changed, or the server
  did not honor the required identity checks. Save the error, plan again from the
  original request, and use a new output path. Do not reuse partial output or edit
  the completion manifest to mark it complete. Persistent conflicts are feedback.
- No observations / no matching times: confirm the exact collection, native
  variable names, longitude convention and inclusive UTC dates. The public catalog
  can change. Planning succeeded with no result is not an acquisition success.
- Unsupported dtype, dimension order, filter or codec: report source compatibility
  and stop. Having a system GDAL/xarray reader for a format does not make it an
  admitted Tile-Ripper profile. No automatic format conversion is provided.
- Linker killed / signal 9 during compilation: use the documented `--jobs 1` and
  enough memory/disk space. The earlier Linux default parallel debug test exhausted
  linker memory; the serial release build/tests passed.
- Existing output: choose a new directory. Caught failures clean up newly owned
  output; abrupt interruption may leave a partial directory. Check exit status
  and `manifest.json` completion before using results.

Send the exact command, platform/tool versions, error and whether the bundled
example or a different source was used. The [human trial feedback form](alpha/feedback-template.md)
collects this without automatic uploads. Optional Python reader installation
failures should be recorded separately from Rust acquisition failures.


The [v3 source profile](zarr-v3-input/workflow.md) uses the same native or
Zarr-only build, with zarrs 0.23.14 sharding and Zstd included for reading.
It needs no Python runtime. Experimental v3 **output** still requires the
separate feature and explicit encoding; uncompressed v2 output remains default.


## Wave 1 named scientific products

For Landsat 8/9 SR, public HLS/OPERA references and qualified native scientific COGs:

```sh
cargo build --release --locked --no-default-features --features native-cog
./target/release/tile-ripper products
./target/release/tile-ripper fetch examples/product-landsat.json --gdal-free --output NEW.zarr
```

For explicitly authenticated production HLS and current OPERA COGs, add `authenticated-http` and supply a private netrc file with `--auth earthdata --netrc PRIVATE_FILE`. The product helper selects the separately qualified HLS L30/S30 v2, PO.DAAC DSWx-HLS v1.1 or ASF Cumulus RTC-S1 v1.0 exact-object policy; none inherits the SWOT host policy. Older ASF datapool routes remain unqualified by this authentication policy. Source type/codec/grid gates remain mandatory and Cargo defaults are unchanged. The qualified native COG sample types are UInt8 class/palette indices, UInt16, Int16, and Float32; see [exact capabilities, limits, and evidence](products/qualification.md) and [workflow](products/workflow.md).
