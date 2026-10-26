# Tile-Ripper private source candidate

The current candidate combines the qualified workflows through Wave 4. It is
unreleased, retains Cargo version `0.1.0-alpha.1`, and has no stable API/schema
promise. Project ownership and license approval remain open. No human trial has
completed. [Consolidation status](docs/consolidation.md) records the local checks
and remaining owner decisions.

Start with the [scientist trial](docs/alpha/human-trial.md): install an isolated
GDAL-free build, fetch two small public datasets, inspect raw output and uninstall.
Rust and native C/C++ build tools are required; Python packages are optional
output-inspection tools. Cargo's default build still uses system GDAL.

| Build | Scope |
| --- | --- |
| Default | Established GDAL GeoTIFF and Zarr paths; GDAL must be installed separately. |
| `--no-default-features` | Admitted Zarr-only acquisition. |
| `--no-default-features --features native-cog,native-hdf5,authenticated-http` | Qualified native COG, scientific array products, SWOT and explicit Earthdata access; recommended complete native candidate. |
| `--all-features` | Validation/comparison build, includes GDAL and experimental output choices. |

[The candidate contract](docs/alpha/contract.md) maps each supported profile to its
exact scientific qualification, geographic limits, quality policies and access
route. Coverage includes selected Sentinel-2, Landsat, HLS, OPERA, GOES, SMAP,
NISAR, SWOT and Zarr workflows. A mission name alone does not establish support.
AlphaEarth remains a separate streaming prototype; point/track products remain
outside the candidate.

Native values, grids, calibration metadata and separate masks are retained.
Successful use requires completed source validation and a completion manifest or
stream receipt. Per-object versions do not provide an atomic dataset snapshot.
No consistent speed advantage over tuned GDAL/xarray is established.

The source includes modified `hdf5-reader` and `zarrs_filesystem` crates with their
original notices and documented patches. Use the [current source-handoff review](docs/consolidation/source-handoff.md),
which supersedes the earlier non-vendored procedure. No package, binary, runtime
or container is distributed by this candidate.

[Installation](docs/installation.md) · [Validation](docs/consolidation/validation.md) ·
[Human feedback](docs/alpha/feedback-template.md) · [Owner decisions](docs/alpha/closeout.md)
