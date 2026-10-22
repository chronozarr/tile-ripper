# Quality-aware SWOT Raster acquisition contract

Continue locally from `dbafecc` on `codex/acquisition-planner`. The referenced
conversation supplies the full original specification. The subsequent user
instruction, **“we want to use version D”**, makes a real Version D product the
qualification target. Preserve the existing Version C WSE regression.

Required: inspect WSE, summary quality, detailed flags, uncertainty, contributing
counts, coordinates, fills, filters, physical chunk layouts and official PDD;
research the SWOT/PO.DAAC Cookbook before selecting policies. Add only native
UInt8/UInt32 support needed alongside existing UInt16/Float32. Preserve stored
values, byte-order interpretation, source attributes, calibration, projection,
coordinate centers, independent missingness and version identity. Reject
unsupported types and ambiguous implicit initialization/CF missingness.

One request must acquire all five arrays over one logical window using each
variable's own physical index. Stream bounded, deterministic aligned raw tiles
and a separate reproducible quality mask. Supply raw, documented summary, and
named required/excluded bitwise policies. Never define validity by finite WSE
alone or discard raw auxiliary values because a derived mask excludes a pixel.

Independently compare every selected value, coordinate, source mask and derived
mask with separately executed h5py and NetCDF-C. Cover different chunk shapes,
strides, boundaries, dense/sparse/missing regions, integer high bits, payload
corruption, cancellation, mutation and small budgets. Reproduce an explicit
quality-aware count, mean and pixel-uncertainty summary independently.

Measure WSE-only versus multi-variable metadata/index/coordinate/science traffic,
request counts, duplicates, CPU, RSS, reservations, first output and completion;
compare mature controls with access and validation differences stated. Reuse
version-bound ranges and metadata safely. Full index traversal stays the default;
selective traversal remains experimental with reduced structural validation.

Study authenticated official HEAD and GET routes separately. Live native access
requires legitimate caller credentials, confined authentication/cookie scope,
explicit safe redirects, no secret logging or HTTPS downgrade, and unchanged
conditional source identity checks. If these invariants are not demonstrated,
retain the validated bounded mirror and deliver a detailed design; never label
reference HTTP access or mirror replay as live native access.

Repeat native COG, Zarr v2/v3, HDF5, streaming, cancellation and publication
regressions. Keep GDAL/libhdf5 optional or absent from the native core. No general
HDF5/CF/geolocation framework, river-specific processing or new output system.

Report the ten decision gates in `report.md` and commit validated work locally.
No merge, push, publication, tag, deployment, ChronoZarr changes or upstream HDF5
submission. No whole-product download within native acquisition.
