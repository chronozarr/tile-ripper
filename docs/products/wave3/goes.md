# GOES-19 ABI CMIPC qualification

Two independent NOAA operational specimens from 2026-10-24 are admitted: C13 (1500 × 2500, brightness-temperature codes) and C01 (3000 × 5000, reflectance-factor codes). Exact names, ETags, object sizes and physical metadata are in the inspection receipts. This admits GOES-19 Mode 6 CONUS CMI/DQF, not other ABI L1b/L2 layouts or sweep conventions.

Original CMI Int16 and DQF Int8 codes are preserved, including `_Unsigned=true`, `_FillValue=-1`, scale/offset, units, valid ranges, original projection, band identity/wavelength, scan midpoint and original acquisition/processing timestamps. Neither unsigned reinterpretation nor calibration is applied. CMI and DQF use different row-stripe chunks; both use legacy v1 B-trees and shuffle/DEFLATE. Their original packed Int16 axes retain their source dtype and CF radian packing. The index-space chunk transform is explicitly not geolocation.

The narrow sweep-x navigation follows [NOAA fixed-grid equations](https://www.star.nesdis.noaa.gov/atmospheric-composition-training/satellite_data_goes_imager_projection.php). Full stored native axes are inspected under a bounded reservation. Geographic selection inverse-navigates source centers and returns their smallest native index envelope, then preserves a separate center-inclusion mask. Space has no geographic coordinate and is represented by `earth_visible=0` and derived NaN longitude/latitude. Explicit native selections include every requested index in `aoi_mask`, including space. No reprojection/resampling occurs. The source time-bounds link is retained in original attribute provenance; it is omitted from active destination CF metadata because that bounds array is outside the exported subset. Geographic polygons are closed, may contain holes, and reject antimeridian edges.

`source_valid_CMI`, `source_valid_DQF`, `aoi_mask`, `earth_visible` and `quality_goes_dqf_good` are independent. The named quality mask accepts raw DQF exactly 0. It does not change raw values or infer another band's validity.

The eight-case [native acceptance receipt](goes-validation.json) and separate [live receipts](goes-live-validation.json) cover two live anonymous NOAA acquisitions, native center/space/strided selections and independent geographic selections for both bands. Every selected stored sample and source-validity/quality value matches h5py and independently opened NetCDF-C with automatic decoding disabled. PROJ navigation agrees within 5.2e-12 degrees; Zarr-Python and xarray reopen complete outputs. Captured bytes and SHA256 extents are separate from logical object sizes; sparse source files are incomplete and guarded replay rejects holes. No performance-leadership claim is made.

Build and use:

```sh
cargo build --locked --release --no-default-features --features native-cog,native-hdf5,authenticated-http
target/release/tile-ripper discover --product goes19-abi-cmipc-c13 --bbox '-80,34,-79.9,34.1' --start 2026-10-24T16:00:00Z --end 2026-10-24T16:05:00Z --bands CMI,DQF --limit 2
target/release/tile-ripper plan examples/product-goes-abi.json
target/release/tile-ripper stream examples/product-goes-abi.json --values
target/release/tile-ripper fetch examples/product-goes-abi.json --output NEW.zarr
cargo run --locked --release --no-default-features --features native-hdf5 --example array_product -- examples/product-goes-abi.json
uv run --no-project --python 3.12 --with-requirements docs/products/wave3/reference.txt python bench/wave3_arrays.py goes
```

Discovery is bounded to one hour and at most 32 observations, and errors rather than truncating. Acquisition admits one explicitly selected observation per job. Provider selection occurs before physical preparation; automatic discovery uses NOAA's anonymous public bucket. Supplied reference mirrors require `provider=pinned`. Prepared state stays bound to its actual source and validated version; no fallback occurs. Output reuses the existing deferred-flush Zarr store and atomic no-replace publication transaction.
