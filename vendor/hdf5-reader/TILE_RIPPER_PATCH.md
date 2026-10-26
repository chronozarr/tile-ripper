# Scoped hdf5-reader experiment

This is the published `hdf5-reader 0.9.3` source, from upstream commit
`3d708fd40c6809f5d215bd2d9f15e9a05de84bd0` of
`https://github.com/roteiro-gis/netcdf-rust`. `src/btree_v1.rs`, `src/dataset.rs`,
`src/messages/datatype.rs` and `src/messages/fill_value.rs` differ from the
crate's original source files.

The [current complete patch and file audit](../../docs/consolidation/vendor/vendor-audit.json)
supersede the earlier traversal-only diff for maintenance. Keep this pinned fork
for the current candidate; rebase or remove it only after equivalent upstream
behavior passes the same independent science and failure gates. The standalone
suite passes 356 tests under Rust 1.91 during consolidation.

The additive API experiment implements selected legacy v1 traversal inside
the existing parser crate. Default full traversal remains unchanged. See
`../../docs/hdf5-selective/audit.md`, the unpublished `upstream.patch`, source
hashes and performance/science/fault receipts. The crate is pinned by the root
manifest; no general HDF5 parser was added to Tile-Ripper. The standalone
Cargo.lock records the dependency set used for the crate's own unit/Clippy
checks; the root lock remains authoritative for acquisition builds.

MIT and Apache-2.0 license texts were obtained from that exact upstream commit.
No upstream contribution has been published. Rebase or remove this source
copy when an accepted upstream API becomes available.

## Canonical integer admission (Version D quality milestone)

The fixed-point parser previously discarded HDF5 bit offset and precision.
An independently written 24-bit integer in a 32-bit container with bit offset
4 was consequently decoded as ordinary UInt32, disagreeing with h5py. The
local parser now rejects nonzero offsets, non-full-width precision and
unrepresentable storage widths with an
explicit unsupported-encoding error. Ordinary UInt8/UInt16/UInt32 and existing
canonical integer metadata retain their behavior. This is a rejection guard,
not support for arbitrary HDF5 numeric encodings. Parser and end-to-end tests
cover the boundary. No upstream submission is authorized or performed.

## Storage initialization and fill write time (Wave 3)

`dataset.rs` no longer substitutes the CF `_FillValue` attribute for HDF5 storage
initialization. Default numeric initialization remains zero; explicit HDF5 fill
bytes remain authoritative. Scientific CF markers stay attributes. Both endian
variants, allocated zeros/NaNs and unallocated default/explicit fills agree with
h5py and separately opened NetCDF-C. Actual SMAP v006/R19240 and NISAR GCOV
provisional products expose this distinction.

`messages/fill_value.rs` now decodes HDF5 fill write time as allocation=0,
never=1, if-set=2, matching the HDF5 specification. The previous table swapped
all three meanings. Undefined storage fill is explicitly rejected rather than
conflated with default zero. Tile-Ripper also rejects never-initialized storage.
These are deliberately unsupported states; no arbitrary initialized bytes are
claimed as scientific values. This correction
is local and has not been submitted upstream.
