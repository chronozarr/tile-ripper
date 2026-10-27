# Installation

Tile-Ripper is alpha software distributed as source (version 0.1.0-alpha.1).
Commands, the Rust API and the manifest format may change. There is no package
registry entry, prebuilt binary or container: you build it from a checkout of
[the repository](https://github.com/chronozarr/tile-ripper) with Cargo and the
checked-in `Cargo.lock`. Python is not needed to fetch data.

## Choose a build

Cargo features select what the executable can read. Use the complete native
build unless you need GDAL for something specific.

| Build | Cargo flags | What it covers | Needs GDAL |
| --- | --- | --- | --- |
| **Complete native (recommended)** | `--no-default-features --features native-cog,native-hdf5,authenticated-http` | Every dataset in [Supported data](guide/supported-data.md): native cloud-optimized GeoTIFF (COG) products, GOES/SMAP/NISAR arrays, SWOT, the Zarr stores and explicit Earthdata login | No |
| Default | none | The established GDAL GeoTIFF and Zarr paths | Yes, installed separately |
| Zarr-only | `--no-default-features` | Supported Zarr acquisition only | No |
| All features | `--all-features` | Validation and comparison build; includes GDAL and experimental output choices | Yes |

The rest of this page uses the complete native build. The others are under
[Other builds](#other-builds).

## Prerequisites

- **Rust 1.91 or newer (tested with 1.92).** The optional `icechunk` feature
  requires Rust 1.91.1 or newer; use 1.92 for the full validation matrix. Install Rust with
  [rustup](https://rustup.rs). A Linux distribution's packaged Cargo may be too
  old for this lockfile; do not regenerate `Cargo.lock` to work around that.
- **A C/C++ compiler, CMake, pkg-config and CA certificates.** Bundled Zarr
  codecs (Blosc, Snappy) and TLS compile C/C++ code during the build, so this
  is not a Rust-only build. They do not need system libtiff or GDAL.
- **Git**, to clone the source.

On macOS, install Apple's command-line developer tools and Homebrew packages:

```sh
xcode-select --install
brew install cmake pkg-config
```

On Ubuntu or Debian:

```sh
sudo apt-get update
sudo apt-get install build-essential cmake pkg-config \
  ca-certificates curl
```

GDAL, PROJ, libtiff, HDF5 and Python are not needed for this build.

## Get the source and check the tools

```sh
git clone https://github.com/chronozarr/tile-ripper.git
cd tile-ripper
sh scripts/check-environment.sh --native
```

It prints your `rustc` and `cargo` versions and ends with
`Basic build/runtime checks passed; this does not validate remote access or a dataset.`
A missing tool stops it with `Missing TOOL. See docs/installation.md for
platform prerequisites.` Without `--native` the script also requires GDAL
(`gdal-config`, `gdalinfo` with GTiff support), which only the default build needs.

## Build

From the `tile-ripper` directory:

```sh
cargo build --release --locked --no-default-features \
  --features native-cog,native-hdf5,authenticated-http \
  --bin tile-ripper
./target/release/tile-ripper --help
```

If your default Rust is older, Cargo stops with
`error: rustc 1.88.0 is not supported by the following packages`
(with your version). Install the tested toolchain and build with it:

```sh
rustup toolchain install 1.92.0
cargo +1.92.0 build --release --locked --no-default-features \
  --features native-cog,native-hdf5,authenticated-http \
  --bin tile-ripper
```

You can now run `./target/release/tile-ripper` from the repository root, as in
the [Quickstart](quickstart.md).

## Install on your PATH

To run `tile-ripper` from any directory, install it into its own prefix. This
leaves any other installation untouched:

```sh
cargo install --path . --locked --no-default-features \
  --features native-cog,native-hdf5,authenticated-http \
  --root "$HOME/.local/tile-ripper"
export PATH="$HOME/.local/tile-ripper/bin:$PATH"
tile-ripper --help
```

Add the `export` line to your shell profile (for example `~/.zshrc` or
`~/.bashrc`) to keep it in new terminals. Bundled example requests such as
`examples/weatherbench2.json` are still paths relative to the repository root,
so run the examples from there.

Copying a built executable to another machine is not tested and not a
supported installation method; build on each machine.

## Verify

Check that the installed executable starts:

```sh
sh scripts/check-environment.sh --native \
  "$HOME/.local/tile-ripper/bin/tile-ripper"
```

This repeats the tool check and runs the executable's `--help`. If the
executable cannot load or run, it stops with `Executable failed to load/run.`
The check does not contact any data provider; the
[Quickstart](quickstart.md) fetch is the first end-to-end test.

## Uninstall

```sh
cargo uninstall --root "$HOME/.local/tile-ripper" tile-ripper
```

This removes only the executable from that prefix. Remove the `export PATH`
line you added, and delete the `$HOME/.local/tile-ripper` directory, which
still holds Cargo's small bookkeeping files. Uninstalling does not delete your
fetched datasets, request files, explicit `--cache` files, the repository's
`target/` build directory or Cargo's download cache; remove those yourself if
you no longer need them. No service, startup task or scheduler is installed.

## Tested platforms

The complete native build is tested on macOS ARM64 (Apple silicon) and in
Linux ARM64 (glibc) containers. Windows, Intel macOS, Linux x86_64, musl and
WebAssembly (WASM) are unverified. Container results do not replace a test on
a physical machine. At run time Linux needs libstdc++, libgcc_s, libm and libc;
macOS needs its own system frameworks and C++ runtime.

## Troubleshooting

- **Linker killed or signal 9 while compiling.** The linker ran out of memory.
  Add `--jobs 1` to the `cargo build` or `cargo install` command and make sure
  there is enough free memory and disk space. A parallel debug build on Linux
  once exhausted linker memory; the serial release build passed.
- **Existing output.** Fetch refuses an output path that already exists.
  Choose a new path. A caught failure removes the output it created, but an
  abrupt interruption can leave a partial directory or a hidden
  `.NAME.partial-*` sibling. Use a result only if the command exits
  successfully and `manifest.json` says `"status": "complete"`. See
  [Output, values and masks](guide/output.md).
- **Source version conflict.** The source's metadata or files changed during
  the run, or the server did not honor the required version checks. Save the
  error, run `plan` again from the original request and fetch into a new path.
  Do not reuse partial output or edit a manifest to mark it complete. Report
  conflicts that keep happening.
- **No observations or no matching times.** Check the exact collection, the
  native variable names, the longitude convention and the inclusive UTC start
  and end times. Public catalogs change. A plan that succeeds with nothing
  selected is not a successful acquisition.
- **Unsupported data type, dimension order, filter or codec.** The source is
  outside the supported profile; stop there and report it. A file that GDAL or
  xarray can read is not automatically supported, and there is no automatic
  format conversion. See [Supported data](guide/supported-data.md).
- **HTTP 403 or 404, an unsupported redirect, or a rejected public URL.** Use
  the exact bundled request file and the documented endpoint, and check that
  the provider is up. Public Zarr requests have no credential, signed-URL or
  redirect workaround. For datasets that need a NASA account, see
  [Earthdata login](guide/earthdata-login.md). Never put tokens or passwords in
  a request file or a bug report.

When reporting a problem, include the exact command, your operating system,
Rust and tool versions, the full error, and whether you used a bundled example
or another source. Report Python reader problems separately from Tile-Ripper
fetch problems. The [feedback form](alpha/feedback-template.md) lists the
details that help; nothing is uploaded automatically.

## Other builds

Every build writes the same `./target/release/tile-ripper`, so building a
different feature set replaces the previous executable. Rebuild with the
complete native build to get every supported dataset back.

### Default build (GDAL)

Cargo's default feature set uses the system GDAL library. It is needed for
GeoTIFF output, which fails explicitly in builds without GDAL. The tested
combination is macOS arm64 with Rust 1.92.0 and GDAL 3.12.4; an Ubuntu 24.04
arm64 container with GDAL 3.8.4 was also tested. This is a tested combination,
not a cross-platform certification.

Install GDAL and the build tools first. On macOS (Homebrew, plus the
command-line developer tools above):

```sh
brew install gdal pkg-config
```

On Ubuntu or Debian:

```sh
sudo apt-get install build-essential pkg-config cmake \
  libgdal-dev gdal-bin ca-certificates curl
```

GDAL must provide GTiff, HTTP/curl, PROJ and GEOS support for the STAC
(SpatioTemporal Asset Catalog) workflow. Do not mix Homebrew and Conda GDAL
installations. Then check, build and optionally install into a separate
prefix:

```sh
sh scripts/check-environment.sh
cargo build --release --locked --bin tile-ripper
cargo install --path . --locked \
  --root "$HOME/.local/tile-ripper-gdal"
```

The executable links GDAL dynamically and needs that same GDAL at run time; in
this build even Zarr-only commands link GDAL. Pass the installed executable's
absolute path to `scripts/check-environment.sh` to confirm it loads. The macOS
test reused an existing Homebrew GDAL; provisioning a fresh operating system
was not tested. See the official references for
[Cargo local installation](https://doc.rust-lang.org/cargo/commands/cargo-install.html),
[GDAL packages](https://gdal.org/en/stable/download.html) and
[Rust GDAL binding requirements](https://github.com/georust/gdal#version-support).

GDAL troubleshooting:

- **gdal-sys cannot locate GDAL.** Inspect `gdal-config --version`,
  `gdal-config --cflags` and `gdal-config --libs`. The compiler, headers and
  libraries must come from the same prefix and architecture. The binding
  supports `GDAL_HOME`, `GDAL_LIB_DIR` and `GDAL_INCLUDE_DIR`; see its
  [build documentation](https://github.com/georust/gdal/blob/master/README.md).
  Do not point at a different installation just to silence the error.
- **`Library not loaded: …libgdal…` (macOS) or `libgdal.so…: cannot open
  shared object` (Linux).** This happens before Tile-Ripper can print anything.
  Run the environment check, `otool -L /path/to/tile-ripper` (macOS) or
  `ldd /path/to/tile-ripper` (Linux). Restore a consistent GDAL installation
  and rebuild Tile-Ripper after a GDAL upgrade. Do not copy or symlink a
  differently versioned library into place.
- **`GDAL could not open asset`.** Confirm `gdalinfo --formats` lists GTiff and
  that a small local GeoTIFF opens. Then check that the provider is reachable
  from your network and your CA certificates are installed. The error can mean
  missing GDAL HTTP/TLS support or a source access failure; it does not say
  which. Source URLs are deliberately left out of error messages.

### Zarr-only, native-COG-only and native-HDF5-only builds

```sh
cargo build --release --locked --no-default-features
cargo build --release --locked --no-default-features \
  --features native-cog
cargo build --release --locked --no-default-features \
  --features native-hdf5
```

The Zarr-only build reads the supported Zarr v2 stores and the Zarr v3 input
profile (zarrs 0.23.14 sharding and Zstd included); experimental Zarr v3
output needs a separate feature and an explicit encoding, and uncompressed v2
remains the default output. Adding `native-cog` reads native GeoTIFF/COG
sources (Sentinel-2 and the Landsat, HLS and OPERA products, with UInt8
class/palette indices, UInt16, Int16 and Float32 samples) and writes
GDAL-free Zarr output. GOES, SMAP and NISAR profiles need `native-hdf5`; a
build without it answers a GOES request with
`unknown scientific product profile`. Earthdata login needs
`authenticated-http`; each archive (LP DAAC for HLS, PO.DAAC for DSWx-HLS
v1.1, ASF for RTC-S1 v1.0, and the SWOT, SMAP and NISAR archives) has its own
exact access policy, and older ASF datapool routes are not supported. The
native path converts a WGS84 area only to nearby WGS84 Universal Transverse
Mercator (UTM) source grids; raster values are never reprojected. The
complete native build combines all three.

### All features and reader selection

`--all-features` (or `cargo build --features native-cog`) builds both the
GDAL and native COG readers for comparison. Choose with `--backend native` or
`--backend gdal`, or set `TILE_RIPPER_COG_BACKEND=native` or `gdal` before
preparation and keep it fixed for the process; the command-line flag wins. A
build with both readers still links GDAL. See
[Reader selection and Rust API](native-http/cli.md).

### What `--gdal-free` does

`--gdal-free` on `plan`, `fetch` or `stream` makes a GDAL-linked build fail
immediately instead of silently using GDAL, and selects the native COG reader
when it is compiled in. It also works with the Zarr-only build. The Rust
`acquisition::prepare_gdal_free` helper performs the same check. To confirm
what an executable actually links, inspect it with `otool -L` (macOS) or `ldd`
(Linux) rather than relying on environment settings.

Maintainer notes on portability, packaging and release steps are in
[Release preparation](release-preparation.md).
