# Unpublished upstream proposal

Suggested title: **Add explicit selection-aware legacy v1 chunk-index traversal**

In `hdf5-reader 0.9.3`, bounded chunk selection filters leaf results after the
storage collector traverses every branch. On the archived GOES-19 cloud-top
temperature control, selecting one allocated chunk visits 16 nodes and makes
32 exact index GETs / 23,800 bytes. An explicit selective collector visits two
nodes and makes four GETs / 3,040 bytes, with the identical payload range and
independently verified values/coordinates. Dense and strided results, including
cases that visit the whole index, accompany the patch.

The [patch](upstream.patch) adds an explicit bounds-based collector and
`Dataset::read_slice_selective_v1`. It reuses the existing node parser and
compares Cartesian selection successors with lexicographic child intervals.
The trailing byte dimension and absent-record behavior remain significant.
There is no second HDF5 parser in Tile-Ripper. Existing signatures and default
`read_slice` behavior remain unchanged; selected entry caches are isolated from
subsequent full reads.

The new collector validates ordering, aligned origins, visible child pointers,
child levels and parent boundaries. It cannot validate descendants it does not
read. A test deliberately corrupts an unrelated leaf: full traversal fails,
selected lookup succeeds, and selecting that damaged branch fails. This is why
the proposal is an **opt-in API**, not a default behavior change. Recommend
against replacing the inherited full-traversal contract with pruning by default.

The pinned source and patch provenance are in
[upstream-provenance.json](upstream-provenance.json). `git apply --check`
succeeds against the two original files at the recorded upstream commit.
The vendor crate passes all 269 library unit tests and warning-denying
all-target Clippy. Five new unit tests cover Cartesian successor correctness,
full/selected equivalence, missing records, reversed physical addresses,
four-byte pointers, structural faults, scope and cache isolation. Independent
h5py/NetCDF-C comparisons and detailed timing/I/O/resource receipts are linked
from the [report](report.md).

Maintenance implications: keep this proposal additive, make validation scope
part of the API documentation, preserve the full collector, and rerun the
selection and corruption gates on rebases. The changes use APIs compatible
with the crate's declared Rust 1.81 minimum; that toolchain was not separately
qualified here. A bounded parsed-node cache could be a later upstream
optimization. Correct handling of implicit HDF5 initialization versus CF fill
is a separate issue exposed by the inspected NISAR sample.

Nothing has been submitted upstream. Publication requires the user's approval.
