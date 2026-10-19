# First scientist trial — Tile-Ripper 0.1.0-alpha.1

This is a trial to be completed by a real person. No human validation is claimed.
Allow time for native packages and a Rust build. You need a terminal, public HTTPS
access and a few GB of temporary build space. The scientific downloads are small:
Sentinel-2 about 3 MB estimated encoded chunks (actual GDAL wire bytes unavailable),
ERA5 about 0.65 MB measured total bodies on previous trials. This is not a strict
network cap. Don't expand dates, regions or variables during this first trial.

The maintainer supplies a clean source checkout at a fixed revision **after**
licensing/access approval, with no credentials required for these data sources.
Open a terminal and change into that folder; all commands below run there.
Record its revision (`git rev-parse HEAD` if Git metadata is included). Copy the
[feedback template](feedback-template.md) before starting. Record UTC now with
`date -u '+%Y-%m-%dT%H:%M:%SZ'`: this is the overall trial start time.

## 1. Prerequisites

Tested: macOS arm64 (Apple Silicon) / Homebrew GDAL 3.12.4, or Ubuntu 24.04 arm64
container / apt GDAL 3.8.4. Other platforms are untested, not excluded by design.
`uname -sm` records your architecture. A fresh Mac provisioning test has not been
completed; Ubuntu was tested in a container, not a physical machine.

On macOS, install Apple's command-line tools if `xcode-select -p` fails
(`xcode-select --install`, then finish the installer). Install Homebrew from its
[official instructions](https://brew.sh/) if absent, then:

```sh
brew install gdal pkg-config cmake
```

On Ubuntu:

```sh
sudo apt-get update
sudo apt-get install build-essential pkg-config cmake libgdal-dev gdal-bin ca-certificates curl
```

Install Rust/rustup from [the official Rust instructions](https://www.rust-lang.org/tools/install)
if absent; open a new terminal so Cargo is in PATH. Do not change a working system
GDAL to match a particular version number just for this trial. Record the version
you actually get; Homebrew/apt releases may change. Then on either platform:

```sh
rustup toolchain install 1.92.0 --profile minimal
rustup run 1.92.0 sh scripts/check-environment.sh
```

Stop and record errors if prerequisites fail. Do not mix Conda and Homebrew GDAL.

## 2. Isolated source installation

Use the exact lockfile; no registry package is published. One build job reduces
linker memory pressure. All trial-specific files go in `.alpha/` in this checkout.

```sh
mkdir -p .alpha
cargo +1.92.0 install --path . --locked --jobs 1 --root "$PWD/.alpha/install" --target-dir "$PWD/.alpha/build"
export PATH="$PWD/.alpha/install/bin:$PATH"
rustup run 1.92.0 sh scripts/check-environment.sh "$PWD/.alpha/install/bin/tile-ripper"
cargo +1.92.0 install --list --root "$PWD/.alpha/install"
```

The listing should show `tile-ripper v0.1.0-alpha.1`. No CLI `--version` flag is
promised. Save the revision, Rust/GDAL versions and any prerequisite repairs.

## 3. Sentinel-2 plan, fetch and inspection

Before running, write down what you expect the dates, bands and AOI to select.
The bundled request selects two bands on 2025-07-28 in a tiny North Carolina
polygon. It is not a cloud-free composite. Choose a new output path if repeating.

```sh
tile-ripper plan examples/unified-stac.json --human
date -u '+%Y-%m-%dT%H:%M:%SZ'
tile-ripper fetch examples/unified-stac.json --output .alpha/sentinel --human
date -u '+%Y-%m-%dT%H:%M:%SZ'
find .alpha/sentinel -name '*.tif' -exec gdalinfo {} \;
```

Record first successful fetch time, including all prerequisite/build time from
trial start; also report time from the timestamp immediately before fetch.
Stop on errors; don't count partial output as success. Expect two native 91×113
UInt16 TIFFs, EPSG:32617, 10 m, nodata 0, scale 0.0001 and offset −0.1 on the
previously tested observations. If the public catalog differs, report it.
Stored integer samples are not already scaled reflectance; clouds remain data.

## 4. ERA5 Zarr plan, fetch and inspection

```sh
tile-ripper plan examples/weatherbench2.json --human
tile-ripper fetch examples/weatherbench2.json --output .alpha/era5 --human
```

Expect `.alpha/era5/era5.zarr`, two UTC times (1959-01-01 00:00, 06:00), four
latitudes × three longitudes and `2m_temperature` in Kelvin. This is a coarse
64×32 regridded ERA5 product. Bounds select intersecting native cells; outer
centers may fall beyond the rectangle. No Celsius conversion occurs.

Inspect both completion manifests with your text editor. To verify status and see
available provenance fields with ordinary Python (no libraries):

```sh
python3 - <<'PY'
import json
from pathlib import Path
for folder in (Path('.alpha/sentinel'), Path('.alpha/era5')):
    m = json.loads((folder / 'manifest.json').read_text())
    assert m['status'] == 'complete'
    print(folder, 'status:', m['status'], 'fields:', ', '.join(sorted(m)))
PY
```

To read actual Zarr values, use an existing xarray/Zarr v2 environment, or create
this optional isolated reader (Python 3.11–3.13 recommended; package installation
needs internet; this environment is not part of Tile-Ripper):

On macOS, use an installed Python 3.13 (`brew install python@3.13` if needed),
then `python3.13 -m venv .alpha/reader`. On Ubuntu 24.04, install `python3-venv`
if needed and run `python3 -m venv .alpha/reader` (the system Python is 3.12).
Do not use Python 3.14 with these older reader pins. Then:

```sh
.alpha/reader/bin/python -m pip install -r bench/zarr-requirements.txt
.alpha/reader/bin/python - <<'PY'
import xarray as xr
ds = xr.open_zarr('.alpha/era5/era5.zarr', consolidated=False, chunks=None)
print(ds)
print(ds.time.values)
print(ds['2m_temperature'].attrs)
print(ds['2m_temperature'].values)
assert dict(ds.sizes) == {'time': 2, 'latitude': 4, 'longitude': 3}
assert ds['2m_temperature'].attrs['units'] == 'K'
PY
```

Record any
reader setup issue separately from acquisition errors. Python is not required to
install or acquire with Tile-Ripper; these steps use it only for inspection.

Explain in your own words: What did plan estimate versus fetch measure? What is
native versus transformed? How do the manifest's source references, versions,
source_metadata, output records and complete status support reuse? Do per-object
ETags guarantee an atomic dataset snapshot? What happens after interruption?
Do not look at implementation code to answer; unclear docs are useful feedback.

## 5. Feedback and cleanup

Complete [feedback-template.md](feedback-template.md) and give it to the maintainer
through your agreed channel. No automatic telemetry or upload occurs. Include
exact commands/errors and manifests only after checking for sensitive local paths
or any changed private URLs. A failure is useful; don't silently repair docs.

Uninstall just this isolated executable:

```sh
cargo +1.92.0 uninstall --root "$PWD/.alpha/install" tile-ripper
```

Acquired datasets, reader, logs and build files remain in `.alpha/`. After saving
feedback, delete that exact folder using your file manager. This does not remove
system GDAL, Rust, Homebrew or Cargo's shared dependency cache. Remove the PATH
entry by opening a new terminal. On SIGINT, inspect output and choose a new path;
there is no resume or automatic cleanup guarantee for a killed process.
