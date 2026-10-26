# Quickstart

Tile-Ripper downloads a small area and time range from a supported dataset and
saves it as Zarr. It keeps the original stored values, the native grid and the
source's masks, and it records the exact source files and versions it read. It
does not reproject, resample or convert units. This page builds Tile-Ripper and
fetches a tiny public ERA5 temperature subset; no account is needed.

Tile-Ripper is alpha software distributed as source (version 0.1.0-alpha.1).
Commands, the Rust API and the manifest format may change.

## Before you start

- Rust 1.91 or newer (tested with 1.92), from [rustup.rs](https://rustup.rs).
- A C/C++ compiler, CMake, pkg-config and CA certificates. On macOS run
  `xcode-select --install` and `brew install cmake pkg-config`. On
  Ubuntu/Debian install `build-essential cmake pkg-config ca-certificates curl`.
- Git, to clone the source.

GDAL, PROJ, HDF5 and Python are not needed to fetch data. Python with xarray
and zarr is only used at the end to read the result.

## Build

```sh
git clone https://github.com/chronozarr/tile-ripper.git
cd tile-ripper
cargo build --release --locked --no-default-features \
  --features native-cog,native-hdf5,authenticated-http \
  --bin tile-ripper
./target/release/tile-ripper --help
```

If your default Rust is older than 1.91, run
`rustup toolchain install 1.92.0` and repeat the build as
`cargo +1.92.0 build ...`. To put `tile-ripper` on your PATH, see
[Installation](installation.md). Run every command below from the repository
root; the bundled example paths are relative to it.

## Plan

The bundled request [`examples/weatherbench2.json`](../examples/weatherbench2.json)
selects 2 m air temperature from the public WeatherBench2 copy of ERA5 (a
coarse 64×32 global grid): two 6-hourly times on 1 January 1959, inside the box
10–25° E, 30–50° N. `plan` checks the source and reports what a fetch would
read, without downloading the data itself:

```sh
./target/release/tile-ripper plan examples/weatherbench2.json \
  --gdal-free --human
```

A trimmed excerpt of the output:

```text
  era5 / 2m_temperature: 3 x 4 native pixels; zarrs; ...
Estimated logical output value bytes: 96 (...)
Estimated unique encoded data bytes: 580139
Native grids; no resampling, calibration or compositing.
```

The selection is 3 longitudes × 4 latitudes × 2 times of 4-byte Float32
values: 96 bytes. The source stores 100 time steps of the whole 64×32 grid in
one compressed chunk of 580,139 bytes, and a chunk can only be read whole. So a
fetch downloads that chunk once and keeps only the 24 values you asked for.
Small requests on coarsely chunked stores read far more than they keep; `plan`
shows the cost before you pay it.

## Fetch

Choose a new output path; existing paths are refused.

```sh
./target/release/tile-ripper fetch examples/weatherbench2.json \
  --gdal-free --output ./era5-subset --human
```

The command ends with:

```text
Acquisition complete: 1 output records.
Output: .../era5-subset
Completion and provenance: manifest.json
```

It creates a plain directory holding one Zarr store per source and a manifest:

```text
era5-subset/
  manifest.json       completion record and provenance
  era5.zarr/          Zarr v2 group
    2m_temperature/   Float32 (time, latitude, longitude)
    latitude/  longitude/  time/
```

Use a result only if the command exits successfully and `manifest.json` says
`"status": "complete"`:

```sh
grep '"status"' era5-subset/manifest.json
```

This prints `"status": "complete",`. The manifest also records the exact
source objects and their versions (ETags), the selection and what was read.
Running the same fetch again into `era5-subset` fails with
`output directory already exists; choose --output NEW_DIRECTORY`. Do not use
anything left behind by an interrupted run; fetch again into a new path.

## Read it in Python

Any Python environment with xarray and zarr works. With
[uv](https://docs.astral.sh/uv/) you can start a throwaway one:

```sh
uv run --no-project --with xarray --with zarr python
```

```python
import xarray as xr
ds = xr.open_zarr("era5-subset/era5.zarr",
                  consolidated=False, chunks=None)
print(ds)
print(ds["2m_temperature"].values)
```

`print(ds)` shows (trimmed):

```text
Dimensions:         (time: 2, latitude: 4, longitude: 3)
Coordinates:
  * time            (time) datetime64[ns] 16B 1959-01-01 ...
  * latitude        (latitude) float64 32B 30.94 ... 47.81
  * longitude       (longitude) float64 24B 11.25 16.88 22.5
Data variables:
    2m_temperature  (time, latitude, longitude) float32 96B ...
```

The values are Float32 temperatures in kelvin (`units: K`), between about 272
and 288 K here; nothing is converted to Celsius. Coordinates are cell centers.
Every grid cell that overlaps the requested box is selected, so outer cells can
extend past it: the 47.81° N row covers 45.0–50.6° N. This was checked with
xarray 2026.9.0 and zarr 3.4.1.

## Next steps

- [Supported data](guide/supported-data.md): check whether your dataset and
  version are covered.
- A second dataset with no account: GOES-19 brightness temperature in
  [GOES, SMAP and NISAR](guide/goes-smap-nisar.md).
- [Output, values and masks](guide/output.md): output layouts, stored codes
  versus physical values, and masks.
