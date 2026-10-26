# Fetch a first native subset

Tile-Ripper is a source alpha for bounded scientific raster acquisition. It
preserves admitted native values, grids and masks, and records the source identity
and completion status. This example reads a tiny public ERA5 selection without
credentials or GDAL. Python packages are needed only for optional output inspection.

## Install from source

Install Rust 1.91 or newer, a C/C++ compiler, CMake and pkg-config. On macOS,
Apple's command-line tools plus `brew install cmake pkg-config` supply the native
build prerequisites. On Ubuntu, use `build-essential cmake pkg-config ca-certificates`.
The commands below use the validated Rust 1.92 toolchain:

```sh
git clone https://github.com/chronozarr/tile-ripper.git
cd tile-ripper
rustup toolchain install 1.92.0 --profile minimal
cargo +1.92.0 build --release --locked --no-default-features \
  --features native-cog,native-hdf5,authenticated-http --bin tile-ripper
```

Cargo's default feature set uses system GDAL. This complete native build omits it;
see [installation](installation.md) for other supported configurations. No package
registry or prebuilt binary release is available yet.

## Inspect the selection

The bundled request selects `2m_temperature` from the specific WeatherBench2 ERA5
64×32 store: two UTC times on 1 January 1959, four latitudes and three longitudes.
It is a coarse regridded product. Native cells intersecting the rectangle are
selected, so some outer centers can lie beyond it.

```sh
./target/release/tile-ripper plan examples/weatherbench2.json --human
./target/release/tile-ripper fetch examples/weatherbench2.json \
  --gdal-free --output ./first-era5 --human
```

Use a new destination each time; existing directories are rejected. Planning
performs metadata and identity requests. Fetch writes `first-era5/era5.zarr` and
`first-era5/manifest.json`. Process success and manifest `status: "complete"`
authorize use. Partial or interrupted output is not a completed acquisition.

## Read the output

With xarray and Zarr v2 support in an optional Python environment:

```python
import xarray as xr
ds = xr.open_zarr("first-era5/era5.zarr", consolidated=False, chunks=None)
print(ds)
print(ds["2m_temperature"].values)
```

Values remain Float32 temperatures in Kelvin; no Celsius conversion occurs.
For packed integer products, use `decode_cf=False` to inspect original stored
types/codes rather than xarray's automatic CF decoding.

Next, try [named products](products/workflow.md), [GOES/SMAP/NISAR arrays](products/wave3/qualification.md)
or [SWOT jobs](swot-job/workflow.md). Consult the [exact support contract](alpha/contract.md)
before changing providers, product versions, geometry or quality policies.
