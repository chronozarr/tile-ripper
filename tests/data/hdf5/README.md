# Independent native HDF5 golden inputs

Generated with h5py/libhdf5, without the Rust reader or an HDF5 writer crate.
`bench/hdf5_golden.py OUTPUT_DIRECTORY` reproduces their scientific contents.
Physical headers can vary between HDF5 releases.

- `v1-be.h5`: big-endian UInt16, 8 × 11, 3 × 4 chunks, gzip + shuffle +
  Fletcher32, packed-value attributes and big-endian one-dimensional axes.
- `unsupported-fixed.h5`: equivalent values with a modern fixed-array chunk
  index, deliberately outside the admitted profile.
- `storage-fill-valid.h5`: unallocated chunks initialized to 7, with no CF
  missing marker. These are valid stored values.
- `distinct-cf-fill.h5`: storage initialization 7 and CF `_FillValue` 65535.
  Initialized values remain valid; the distinct roles must be preserved.

Wave 3 `storage-cf-{le,be}-{None,7}.h5` fixtures are independently written by
h5py. Each has one allocated 4×4 Float32 chunk (including stored zero, NaN and
1.25) and three unallocated chunks initialized to default zero or explicit 7.
A separate CF NaN marker never changes those initialization values.
