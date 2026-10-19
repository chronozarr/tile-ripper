# Fresh-context newcomer trial — 2026-10-19

Separate agent, no inherited task context, read README and user-facing installation,
STAC and paired-Zarr guides/examples only. No implementation access to fix failures.
This is an independent **agent** docs-only trial, not independent human validation.
FirePass was unavailable; Sonnet OAuth was not used or required.

Environment: macOS arm64, existing Rust/Cargo 1.92.0, Homebrew GDAL 3.12.4 and
Cargo download cache. New install prefix and empty build target. This is a clean
source build, not a fresh Mac provisioning test. Optional existing xarray was used
only to read output. No dependencies were repaired during the trial.

Commands from repository root (PREFIX is a fresh temporary directory):

```sh
cargo install --path . --locked --root "$PREFIX/install" --target-dir "$PREFIX/target"
"$PREFIX/install/bin/tile-ripper" plan examples/unified-stac.json --human
"$PREFIX/install/bin/tile-ripper" fetch examples/unified-stac.json --output "$PREFIX/sentinel" --human
"$PREFIX/install/bin/tile-ripper" plan examples/weatherbench2.json --human
"$PREFIX/install/bin/tile-ripper" fetch examples/weatherbench2.json --output "$PREFIX/era5" --human
```

Build took 47.67 seconds. All commands succeeded. gdalinfo opened both 91×113
UInt16 TIFFs (EPSG:32617, 10 m, nodata 0, scale 0.0001, offset −0.1). Schema-3
manifest was complete, 10,008 valid band samples. No cloud masking was inferred.
README's exact xarray pattern opened ERA5: 2 times × 4 latitudes × 3 longitudes,
24 valid Float32 Kelvin values. ERA5 measured 35 requests / 651,576 HTTP body bytes,
including one scientific chunk GET of 580,139 bytes. COG measured wire totals
were unavailable; estimated encoded scientific payload was 3,035,042 bytes.
No full source downloads were intended; a strict COG wire cap was not verified.

Tester explained: manifest estimates differ from observations; source IDs,
coordinates/units, codec decisions and versions support provenance; complete
status matters; ETags are per object, not an atomic snapshot. Native S2 values
require applying scale/offset for physical interpretation. Polygon masks use
pixel centers; Zarr rectangles include intersecting cells; dates include endpoints;
Zarr's geographic CRS label does not assert a precise provider datum.

One confusion: the historical STAC guide claimed Rust >=1.88, whereas current
installation only claimed tested 1.92. Remove the unverified minimum. No errors,
undocumented prerequisite encountered on this preconfigured host, or workflow
blocker. Trial used JSON selections; direct-flag verification is separate.

Tester removed the temporary install/build directories (786 MB). Small outputs,
plans and logs were initially retained under /tmp for parent review. Parent copied the compact reports into this directory and then removed the trial
tree, build fixtures and temporary Linux containers/image. They are not release fixtures. A human scientist should repeat the same tasks on a machine
without the developer's toolchain, explain one manifest and record all repairs.
