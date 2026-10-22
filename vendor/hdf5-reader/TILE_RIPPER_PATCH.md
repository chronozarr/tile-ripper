# Scoped hdf5-reader experiment

This is the published `hdf5-reader 0.9.3` source, from upstream commit
`3d708fd40c6809f5d215bd2d9f15e9a05de84bd0` of
`https://github.com/roteiro-gis/netcdf-rust`. `src/btree_v1.rs`, `src/dataset.rs` and
`src/messages/datatype.rs` differ from the crate's original source files.

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
