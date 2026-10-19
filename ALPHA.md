# Tile-Ripper 0.1.0-alpha.1 — private source candidate

For scientists who need small native subsets with inspectable acquisition plans
and provenance. No consistent speed advantage over tuned GDAL/xarray is established.
**Not distributed yet:** ownership/license approval is pending. Proposed license
files are review material, not an active grant. No human trial has completed.

Use the supplied source folder at the maintainer's fixed revision. Start with the
[self-contained newcomer trial](docs/alpha/human-trial.md): platform prerequisites,
exact source install/uninstall, two small acquisitions, output inspection and
feedback are all there. Acquisition requires Rust and system GDAL, not Python.
Optional xarray output inspection uses a separate Python environment.

| Capability | Alpha contract |
| --- | --- |
| Tested installation | macOS arm64 + Homebrew GDAL 3.12.4; Ubuntu 24.04 arm64 container + system GDAL 3.8.4; Rust 1.92.0 |
| Other platforms | Untested; not excluded by design |
| Sentinel-2 | Earth Search C1 L2A, named UInt16 native bands, bounded dates/AOI, separate GeoTIFFs |
| Scientific Zarr | Specific WeatherBench2 ERA5 64×32 and NASA POWER IMERG daily v2 stores, native Float32 subsets |
| Advanced local inputs | Narrow UInt16 GeoTIFF/custom-grid Zarr profiles; see exact contract |
| Plan | Network metadata/identity checks, native selection and physical-read estimates; saved JSON is inspection-only |
| Fetch | Native output plus complete provenance manifest; source-version checks; no atomic whole-dataset snapshot |
| Scientific tools | GDAL/rasterio/QGIS for TIFF; xarray + Zarr v2 for Zarr |

Read the [exact layout restrictions and known limits](docs/alpha/contract.md).
No arbitrary STAC/CF/Zarr, Zarr v3, HDF5, remote credentials, cloud masking,
reprojection, mosaic, aggregation, longitude wrapping, Python bindings or scheduler.
Native S2 scale/offset must be interpreted; Zarr units and NaNs are preserved.
Payload budgets do not cap total memory or GDAL wire bytes. SIGINT may leave a
partial directory; success requires normal process completion and a complete
manifest. Existing outputs are never overwritten. API/schema may change.

[Installation/troubleshooting](docs/installation.md) ·
[Tester feedback](docs/alpha/feedback-template.md) ·
[Licensing approvals](docs/alpha/licensing-closeout.md) ·
[Current decision/procedure](docs/alpha/closeout.md)
