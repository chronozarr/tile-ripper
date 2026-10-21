# Legacy v1 chunk-index audit

The audited implementation is the published `hdf5-reader 0.9.3`, from
[`roteiro-gis/netcdf-rust` commit `3d708fd40c6809f5d215bd2d9f15e9a05de84bd0`](https://github.com/roteiro-gis/netcdf-rust/tree/3d708fd40c6809f5d215bd2d9f15e9a05de84bd0/hdf5-reader).
The local extension changes only `src/btree_v1.rs` and `src/dataset.rs` in that
crate. Tile-Ripper continues to delegate format parsing and decoding to it.

## Original behavior

`collect_btree_v1_leaves_storage` reads a node header, computes its used-record
length, reads that node, and recurses through **every** internal child. Only
leaf emission checks `chunk_bounds`. `Dataset::collect_btree_v1_entries` uses
this collector for legacy chunk layouts. Tile-Ripper calls it per selected
logical chunk; its bounded metadata cache eliminates repeat GETs, but not
repeat parsing. The frozen `fe58a2c` executable and the current `full` control
both issue 32 index GETs / 23,800 bytes on the large control, for every tested
selection. There is no full-file retrieval fallback in either native reader.

## Keys and intervals

Type-1 keys contain a compressed byte count, filter mask and `rank + 1` fixed
64-bit offsets. Dataset offsets are element coordinates, not file addresses.
The extra dimension is the byte position within an element. Chunk origins
normally have zero in that dimension. Child addresses use the superblock's
offset width, independently of those 64-bit key coordinates. The four-byte
address regression remains covered.

Keys sort lexicographically over all dimensions. Internal `child[i]` covers
`[key[i], key[i+1])`. At level zero, `key[i]` describes the one allocated chunk
at `child[i]`; the last key has no child. The rightmost sentinel can include a
nonzero trailing byte offset. The extension compares that component and never
treats the sentinel as an allocated chunk. These semantics agree with the
[HDF Group format description](https://github.com/HDFGroup/hdf5/blob/develop/docs/doxygen/dox/H5.format.1.1.dox)
and the reference implementation's
[`H5D__btree_cmp2`, `H5D__btree_cmp3` and `H5D__btree_found`](https://github.com/HDFGroup/hdf5/blob/hdf5_1_14_6/src/H5Dbtree.c).
Physical payload addresses do not determine traversal order.

An undefined root denotes no allocated chunks. A sparse tree omits records;
a search interval can point to the record preceding an absent origin. The
extension returns a record only if **its own origin** belongs to the selection.
Tile-Ripper retains its existing HDF5 initialization and separate CF missingness
rules when no record exists. The larger GOES object has 548 allocated records
in a 24×24 logical grid, so its noncontiguous check also exercises holes.

## Pruning proof and stride handling

Let `G` be the Cartesian product of selected chunk-index bounds, multiplied
componentwise by chunk dimensions, with trailing byte coordinate zero. For a
child interval `[L,U)`, compute the lexicographically least member of `G` that
is greater than or equal to `L`. Visit the child exactly when that successor
exists and is less than `U`.

The successor rounds a component to an allowed aligned origin. If it exceeds
that dimension's maximum, it carries to the previous dimension and resets the
suffix to its minimum. If it first exceeds the corresponding component of
`L`, the minimum remaining suffix is already optimal. The exhaustive unit
check compares this construction to enumeration in ranks one through three,
including overflow at the largest representable coordinate.

For example, rows 1–3 and columns 8–9 select `(1,8),(1,9),(2,8),(2,9),(3,8),(3,9)`.
Treating that box as one key interval would also admit intervening columns in
row 2. The successor test preserves the Cartesian bounds without enumerating
the entire rectangle or reading every intervening branch.

The reusable collector accepts an inclusive box of **chunk indices**. It is
not a general point-union API. The explicit dataset slice method uses the
existing slice bounding box and may visit stride gaps inside that box.
Tile-Ripper already computes the exact unique chunk rows and columns for a
strided request. It queries their Cartesian product as individual origins,
reuses metadata across those queries, and thus prunes between skipped chunk
rows/columns too. The noncontiguous benchmark compares exact physical chunk
sets, native bits, validity and coordinates with full traversal and independent
readers. A necessary predecessor leaf may still be visited to establish that
a selected origin is absent.

## Structural validation and its limit

The selective collector retains the original storage parser, signature check,
bounded reads and visited-address protection. It additionally checks:

- Raw-data node type; valid rank, bounds and nonzero chunk dimensions.
- Strict lexicographic key order and aligned allocated origins.
- Every visible child's defined address, extent and local uniqueness, including
  pointers whose intervals will be skipped.
- Nonzero allocated leaf lengths; selected child level exactly one below its
  parent, and exact lower/upper boundary agreement with that parent.
- Ancestor/revisit rejection. Strictly decreasing byte-sized levels also limit
  a selected path to at most 256 nodes, matching the inherited depth bound.

Used-record lengths are checked before storage reads; Tile-Ripper also imposes
its byte and node budgets. This is not a new global HDF5 validator: neither the
old API nor this extension validates every legal configured B-tree capacity,
sibling relationship or arbitrary semantically plausible corruption. Numeric
decoding, filter checks, object identity, conditional framing, cancellation and
terminal source validation retain their existing owners and checks.

**Unread descendants cannot be validated without reading them.** A deliberately
damaged unrelated leaf makes the full scan fail while an independent selected
branch succeeds; selecting that damaged branch fails. This behavior is tested
in the crate and the HTTP fault harness. False parent boundaries could hide a
record in an unread subtree of a malformed file. Ordering checks in visited
nodes cannot establish the contents of skipped nodes. An ETag pins a version;
it does not authenticate the file's logical structure.

Accordingly, full traversal remains Tile-Ripper's default. Selective traversal
is an explicitly selected experiment, not a replacement for whole-index
structural traversal. Do not adopt it as the default where the inherited
whole-index failure behavior is required. No automatic fallback silently
changes that choice. Unordered, misaligned, wrong-type or inconsistent visited
layouts fail explicitly instead of attempting unsafe pruning.

## API and maintenance decision

The added APIs are `collect_btree_v1_leaves_storage_selective(...)` and
`Dataset::read_slice_selective_v1(...)`. Existing collector signatures,
`OpenOptions`, `Dataset::read_slice` and default behavior remain unchanged.
The new slice method clones its view and isolates entry caches; a selective
lookup cannot seed a later full lookup with validation of a smaller scope.
There is an explicit regression for this cache separation.

Recommend an upstream **opt-in API proposal**, accompanied by the scope test,
performance receipts and unpublished patch. Recommend against changing
upstream or Tile-Ripper defaults to selective traversal under the current
validation contract. A bounded parsed-node cache could reduce repeated CPU
work later, but is outside this extension. The pinned vendor copy is a
temporary integration mechanism; rebasing means reviewing two source files,
reapplying the patch and rerunning these gates. No upstream issue or PR was
published.
