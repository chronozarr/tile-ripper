# First scientist trial for the current candidate

This trial needs a real person; the automated installation/smoke receipt does not
count as human feedback. Use the fixed source revision supplied by the maintainer
from [the published source](https://github.com/chronozarr/tile-ripper). Record the revision, UTC start time,
`uname -sm`, Rust version and which tools were already installed. Copy the
[feedback template](feedback-template.md) before starting.

The complete native build avoids GDAL. You need Rust, a C/C++ compiler, CMake,
pkg-config, public HTTPS access and several GB of temporary build space. On macOS,
use Apple's command-line tools and `brew install cmake pkg-config`. On Ubuntu,
install `build-essential cmake pkg-config ca-certificates`. Use your normal trusted
Rust/rustup installation; then run from the source folder:

```sh
rustup toolchain install 1.92.0 --profile minimal
rustup run 1.92.0 sh scripts/check-environment.sh --native
mkdir -p .alpha
cargo +1.92.0 install --path . --locked --jobs 2 \
  --no-default-features --features native-cog,native-hdf5,authenticated-http \
  --root "$PWD/.alpha/install" --target-dir "$PWD/.alpha/build"
cargo +1.92.0 install --list --root "$PWD/.alpha/install"
rustup run 1.92.0 sh scripts/check-environment.sh --native "$PWD/.alpha/install/bin/tile-ripper"
```

The listing should show `tile-ripper v0.1.0-alpha.1`. Use this exact executable
path below so an older installation cannot supply the commands. Do not use an
existing `.alpha/install` prefix or overwrite an earlier output when repeating.
Stop and record a failed step before repairing it.

## Plan and fetch two public datasets

Write down what you expect each request to select before running it. Keep the
bundled region, dates and variables unchanged for this first trial. ERA5 is a
coarse regridded product, and GOES C13 contains packed brightness-temperature
codes with a separate quality field. Neither request needs Earthdata credentials.
The requests are small subsets, but physical chunk/layout reads amplify traffic;
this trial is not a strict network-byte cap.

```sh
.alpha/install/bin/tile-ripper plan examples/weatherbench2.json --human
.alpha/install/bin/tile-ripper fetch examples/weatherbench2.json --gdal-free --output .alpha/era5 --human
.alpha/install/bin/tile-ripper plan examples/product-goes-abi.json --human
.alpha/install/bin/tile-ripper fetch examples/product-goes-abi.json --gdal-free --output .alpha/goes --human
```

Record fetch start/end times and time to the first success including installation.
Inspect each `manifest.json`: process success and `status: "complete"` are required.
The ERA5 result is `.alpha/era5/era5.zarr`: two times, four latitudes, three
longitudes, native Float32 `2m_temperature` in Kelvin. GOES output is a Zarr root
at `.alpha/goes`, with the original observation ID under `observations/`, original
Int16 `CMI`, Int8 `DQF`, coordinates and separate masks. Scan coordinates are angles,
not metres. Scale/offset must be interpreted explicitly.

Optional Python inspection uses a separate Python 3.12 environment; it is not an
acquisition dependency. Use an existing Zarr-v2/xarray environment, or install the
frozen reader dependencies from `docs/products/wave4/reference.txt` in a new venv.
If uv is already installed, this self-contained command supplies the reader:

```sh
uv run --no-project --python 3.12 --with-requirements docs/products/wave4/reference.txt python - <<'PY'
import xarray as xr
import zarr
era5 = xr.open_zarr('.alpha/era5/era5.zarr', consolidated=False, chunks=None, decode_cf=False)
print(era5)
print(era5['2m_temperature'].values)
assert dict(era5.sizes) == {'time': 2, 'latitude': 4, 'longitude': 3}
root = zarr.open_group('.alpha/goes', mode='r')
for observation in root['observations'].group_keys():
    ds = xr.open_zarr('.alpha/goes', group='observations/' + observation,
                      consolidated=False, chunks=None, decode_cf=False)
    print(ds)
    assert ds.CMI.dtype.name == 'int16' and ds.DQF.dtype.name == 'int8'
    print(ds.CMI.attrs, ds.CMI.values, ds.DQF.values)
PY
```

In your own words, explain estimates versus measured traffic, packed values versus
calibration, source validity versus quality, provisional output versus completion,
and per-object versions versus a dataset snapshot. Unclear instructions are useful
feedback. This small trial exercises two workflows; the other profiles retain
their separate [qualification contracts](contract.md).

## Feedback and uninstall

Complete the feedback template and use the channel agreed with the maintainer;
there is no automatic upload. Inspect logs/manifests for private local paths before
sharing them. Uninstall only this trial's executable:

```sh
cargo +1.92.0 uninstall --root "$PWD/.alpha/install" tile-ripper
```

Outputs, logs and build files remain in `.alpha/`; remove that exact trial folder
after preserving feedback. System build tools and Cargo's shared registry cache
remain installed. Interrupted processes may leave incomplete output; inspect it
and choose a new destination rather than treating partial files as success.

Maintainers can automate the two live acquisitions and output reopening with
`scripts/trial-smoke.py`. Its receipt is automated evidence, not a completed human trial.
