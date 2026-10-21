# Scoped hdf5-reader experiment

This is the published `hdf5-reader 0.9.3` source, from upstream commit
`3d708fd40c6809f5d215bd2d9f15e9a05de84bd0` of
`https://github.com/roteiro-gis/netcdf-rust`. Only `src/btree_v1.rs` and
`src/dataset.rs` differ from the crate's original source files.

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
