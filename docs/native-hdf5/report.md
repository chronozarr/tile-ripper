# Native NetCDF-4 / HDF5 decision gate

2026-10-21. Continued `codex/acquisition-planner` from `daa79e3` and `a683ba3`.

**Admit the opt-in `native-hdf5` profile for bounded rank-two scientific array
slices.** A real public NOAA GOES-19 NetCDF-4 acquisition now runs through the
shared planner, admission scheduler and provisional native chunk stream.
Independent h5py and netCDF4 checks compare every selected value and coordinate.
There is no whole-object acquisition fallback and no mandatory system HDF5
runtime. The default GDAL feature remains unchanged.

**Index discovery has an important limit:** hdf5-reader 0.9.3 scans the complete
v1 chunk B-tree before filtering entries. The retained metadata cache avoids
repeated network reads, and a hard buffer cap rejects oversized indices.
Scientific payload ranges are selective. This milestone does not establish
selective traversal of a large HDF5 index or a general performance lead.

## Real scientific demonstration

The source is one archived [NOAA GOES-19 ABI-L2-RRQPEF object](https://noaa-goes19.s3.amazonaws.com/ABI-L2-RRQPEF/2025/100/12/OR_ABI-L2-RRQPEF-M6_G19_s20251001200211_e20251001209519_c20251001210009.nc).
[NOAA documents the product](https://goes-r.noaa.gov/products/baseline-rainfall-rate-qpe.html)
as instantaneous rainfall-rate estimates. This is an archived product example;
no claim about current operational availability is needed.

| Item | Observed |
| --- | ---: |
| Complete source file | 1,268,526 bytes |
| Dataset | `/RRQPE`, UInt16, shape 5424 × 5424 |
| Storage | 226 × 226 chunks, shuffle + DEFLATE, 524 allocated chunks |
| Request | Rows/columns 2400:2784; 384 × 384 samples |
| Logical source chunks selected | 9 |
| Total successful native response bodies | 137,683 bytes, 10.85% of the file |
| Selected compressed payload | 100,867 bytes in 3 adjacent range GETs |
| Metadata/index bodies | 36,816 bytes |
| Native scientific values delivered | 147,456 UInt16 values, 294,912 bytes |
| Independent equality | Every value bit, coordinate and validity bit; h5py and netCDF4 |

Stored rainfall codes, fill markers, scale/offset, units, original axis codes,
axis packing and geostationary projection attributes remain available. The
Float64 `/t` value is 797558706.527315 seconds since 2000-01-01 12:00:00; an
independent CF reference gives 2025-04-10T12:05:06.527315Z. The Rust adapter
preserves the scalar and units rather than applying calendar inference.

The selected native chunks contain 15,863 positive-rainfall samples and yield
an unpacked mean of 2.9975115196116633 mm/hour. The maximum is
100.00009070616215 mm/hour, retaining the source's Float32 packing precision.
The analysis reads emitted subset chunks. The separate 1.27 MB full-file
h5py/netCDF4 reference control is explicitly labelled and never supplies bytes
to native acquisition. See [public receipt](public.json),
[Python verification](python-example.json) and [reproduction](workflow.md).

## Reusable components and adoption

No terrio-specific study was present in this checkout. The existing ecosystem
study and architecture notes already recommended a library-backed HDF5 chunk
boundary. Current owning sources were inspected again:

| Component | Actual usable boundary | Decision |
| --- | --- | --- |
| [hdf5-reader 0.9.3](https://docs.rs/hdf5-reader/0.9.3/hdf5_reader/) | `Storage::len/read_range`, storage-backed open, group/path navigation, parsed object messages, public v1 B-tree collector, typed strided slices, bounded established filters | Adopted exactly, with default Rayon/LZ4 features disabled |
| [netcdf-reader 0.9.3](https://docs.rs/netcdf-reader/0.9.3/netcdf_reader/) | Classic and HDF5-backed NetCDF; storage-backed opens, dimensions/scales, typed slices, optional CF unpacking/calendar helpers | Inspected; no runtime dependency added. Direct HDF5 APIs expose the physical index/range boundary needed here |
| [terrio manifest](https://github.com/opengeos/terrio/blob/main/Cargo.toml) | Uses these readers with default features disabled; also carries a pinned fork for specific reader fixes | Inspected as ecosystem evidence, not adopted as another acquisition layer; exact relevant manifest lines retained in [terrio.json](terrio.json) |

The reader family implements superblocks 0–3, older and newer object/group
metadata, multiple chunk-index families and several numeric/filter types.
Support in that library does not constitute admission in Tile-Ripper.
Its v1 collector's actual source traverses all internal children and filters
leaf records afterwards. Its storage interface can use Tile-Ripper's existing
conditional transport without loading a complete object. Local files also use
bounded range reads; preparation/completion validate local stat identities.

Classic CDF-1/2/5 files are distinct from NetCDF-4 over HDF5. This adapter rejects
classic magic. Arbitrary HDF5 is broader than the admitted profile, while
NetCDF-4 applies its own HDF5 conventions; see the
[Unidata format specification](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html).
No general HDF5 parser, codec or CF implementation was introduced.

## Exact admitted profile

| Property | Admitted behavior |
| --- | --- |
| Object | Closed ordinary single HDF5 file; base address zero, declared EOF matches object length, no active-write flags; local file or anonymous HTTP(S) |
| Remote identity | Strong ETag and source size; conditional reads and final validation; optional caller ETag must agree |
| Dataset | Explicit absolute group/dataset path; rank two, positive explicit array slices/strides |
| Data storage | Chunked layouts parsed to the legacy v1 B-tree representation; consistent two-dimensional chunk extents and element size; full index fits metadata budget |
| Data types | Standard UInt16 and IEEE Float32, both byte orders; native numbers and Float32 payload bits preserved |
| Filters | None, DEFLATE, shuffle, Fletcher32; at most three admitted stages; filter masks checked |
| Coordinates | Explicit one-dimensional contiguous/compact arrays matching each data dimension; Int16/UInt16/Int32/UInt32/Float32/Float64; finite stored values |
| Attributes | Library-parsed inline/dense attributes; type, shape and stored bytes retained; supported scalar strings/numbers also interpreted |
| Missingness | Explicit HDF5 initialization when `_FillValue` is present; one compatible CF scalar `_FillValue`/`missing_value`; NaNs invalid; storage initialization is retained separately and remains valid without a CF missing marker |
| Packing | Finite scalar scale/offset retained; consumers explicitly unpack data and coordinates |
| Space and time | Original coordinate paths and available grid-mapping attributes; optional root `/t` Float64 scalar and units; no inferred geographic raster or time filtering |
| Delivery | Callback/stream only, ordered source-chunk intersections, backpressure, cancellation, terminal failures and successful `finish()` required |

`Spatial::ArraySlice` is a new request selector. Public `Chunk` fields and
`Pixels` variants are unchanged. Stored coordinate centers accompany every
chunk. Index-space transform placeholders and the explicit stored-axis label
prevent an implicit north-up geographic interpretation. Axis choices are
explicit declarations checked against dimension lengths; automatic dimension-
scale association and multidimensional auxiliary-coordinate transport are not
implemented. The [workflow](workflow.md) specifies stride/window semantics.

Unsupported cases fail: modern fixed/extensible arrays, implicit/single-chunk
and v2 B-tree indices; contiguous scientific data; ranks other than two;
complex/compound/string/VLEN scientific values; additional filters; external
raw data/links; virtual datasets; chunked or multidimensional coordinate arrays;
conflicting/vector CF missing markers; CF `_FillValue` over implicit HDF5 initialization (the upstream reader can conflate these); time filters; file sinks and mixed
HDF5/raster selectors. Library parse failures propagate. Noncanonical numeric
bit layouts have not been independently admitted: the upstream datatype model
normalizes representation details, so this remains a standard-producer profile,
not a claim of correctness for every custom HDF5 numeric encoding.

## Physical planning and resource boundaries

All HDF5 structure interpretation stays in hdf5-reader. Preparation discovers
shape/type, explicit axes and metadata, enumerates intersecting logical chunks,
resolves validated compressed ranges, and binds them to source identity.
Readers subsequently open task views over frozen metadata and the admitted
compressed payload group. Any unexpected execution range fails. The library
performs typed slicing, fill synthesis, endian conversion, DEFLATE/shuffle and
Fletcher32 decoding. Its decoded-output limit rejects expanding corrupt chunks.

The real-product index control compares a single chunk with all 576 logical
chunks over the exact frozen NOAA bytes on a loopback server. **Both perform
22 index GETs and read 22,240 index bytes**, with zero scientific-payload GETs
in inspection mode. The public acquisition has 443 repeated header-prefix
bytes and zero repeated payload bytes (byte-union accounting). Index discovery
is a bounded full scan. This measured
limit explains part of native latency overhead and prohibits a selective-index
claim. Exact metadata ranges are reused across workers and prepared executions;
payloads are reacquired on each execution. See [resources.json](resources.json).

Policies are HDF5-specific: individual ranges, adjacent ranges by default, and
an optional bounded-gap policy. Merged spans are capped at 256 KiB, a fraction
of the request buffer and 32 consecutive members. Bounded mode admits up to
16 KiB gaps with at most 25% extra bytes. The gap-layout control transfers
65,578 selected bytes in eight individual GETs; bounded mode uses one GET and
73,258 bytes, including 7,680 overfetch bytes. All values remain equal.
No Zarr thresholds or global Zarr settings govern this policy.

Raw metadata cache admission is at most `buffer_bytes / 32` per file plus
aggregate prepared-state checks. Pending job/DAG construction is bounded before
allocation, including huge all-unallocated layouts. Prepared coordinates,
metadata, physical-plan records and conservative codec/output reservations
enter shared admission. Decoder/encoded buffers drop before their task charge
is released. A 4 MiB reservation supports a 32 MiB streamed output in the
retained control; process RSS is measured separately and includes runtime and
inspection bookkeeping. Full RSS and working-peak receipts are retained. Upstream file-size-bounded
metadata parsing does not establish a hard RSS ceiling for every adversarial
HDF5 structure; this remains an admitted producer/profile experiment rather
than a general untrusted-file service qualification.

## Scientific, integrity and regression checks

- 64 independent scientific cells compare all raw UInt16/Float32 bits, including
  both byte orders, signed zero, NaN payloads, chunk boundaries, positive
  noncontiguous strides, edge chunks, absent chunks and each admitted filter
  combination. Coordinate and validity arrays match h5py; netCDF4 independently
  compares every piece. Four Rust HDF5 tests include storage-fill/CF distinction,
  group identity, repeated prepared execution and cancellation.
- 33 format/HTTP/lifecycle controls cover unsupported indices/types/ranks,
  virtual/external layouts, truncated metadata, invalid chunk offsets, corrupt
  compressed/checksummed chunks, inconsistent framing/encoding/ETags, mutation,
  retry success/exhaustion, cancellation, memory limits and invalid requests.
  A separate 16 MiB inflate-bomb control fails through bounded library decode.
- Available stored Fletcher32 checks are verified. Strong ETags remain identity
  validators: a same-ETag raw uncompressed-byte mutation can complete, and the
  independent oracle detects the changed value. This inherited integrity limit
  is recorded rather than treated as a successful integrity guarantee. No
  publisher-authenticated range checksum was available. The explicit full-file
  control's MD5 agrees with this object's ETag; partial acquisition does not
  independently verify that whole-object digest.
- Default, Zarr-only, native-COG, native-HDF5 and all-feature Rust tests and
  warning-denying Clippy checks pass. Counts and commands are in [rust.json](rust.json).
  Existing independent COG faults/exact values/stream consumer, Zarr v2/CF/v3
  and durable-output checks were replayed under their respective reference
  environments, including 31 durable-output fault cases. Receipts live in
  [inherited/](inherited/) without replacing prior milestone reports.

The initial standalone experiment remains runnable as the ignored
`hdf5::tests::standalone_physical_probe`; its [receipt](standalone.json) records
value/packing equality. The integrated real-product receipt uses the public
API and `finish()`. Test-generated full-file controls and raw per-cell logs
stay in ignored `prototype-output/`; small h5py-written golden files are
versioned for ordinary Rust tests.

## Performance and deployment decision

The matched benchmark uses one native worker, independent h5py processes and
identical data/axis selections. It compares individual/adjacent/bounded native
ranges; h5py exact, 4 KiB and 64 KiB bounded read windows; and an explicit
whole-file h5py control. Each selection is repeated three times at zero and
40 ms added per-request latency. Source bytes, GET/HEAD counts, metadata/index
cost, first values, completion, CPU, RSS, reservations and exact equality are
recorded. Cache/page and metadata strategies differ explicitly. The native timed consumer also writes verification pieces; Python returns an
array and hashes it. Reader-versus-reader CPU/low-latency comparisons include
that downstream-work difference. Native policy comparisons keep it fixed.
Bandwidth is loopback and unconstrained, so this is a latency experiment, not a provider/WAN
throughput claim. The [summary](performance.md) contains the final medians.

Two optional Cargo packages, `hdf5-reader` and `hdf5-core` 0.9.3, are added to
the lockfile; their other dependencies were already present. Default reader
Rayon/LZ4 features are disabled. The project's Rust requirement stays 1.91.
The retained GDAL-free HDF5 oracle links no system HDF5, NetCDF or GDAL library;
its size/platform/hash and dynamic dependencies are in [build.json](build.json).
Python HDF5/NetCDF libraries are reference tooling only. This local milestone
has not established a new Linux distribution/release qualification.

SWOT was considered first. One unauthenticated HEAD on a documented protected
SWOT object returned a redirect, rather than a pinned public range identity;
[swot-access.json](swot-access.json) records it. Official sample links include
compressed archives. Those do not justify staging a whole object as native
selective acquisition. No credentials or large SWOT/NISAR downloads were used.

The best SWOT follow-up is
[L2 HR Raster `/wse` with x/y axes](https://podaac.jpl.nasa.gov/dataset/SWOT_L2_HR_Raster_D),
after obtaining a legal conditional-range route and inspecting the actual
index/type/filter profile. PIXC's point dimension and LR SSH's multidimensional
geolocation need further semantic work. NISAR
[GCOV `.../grids/frequencyA/HHHH`](https://nisar-docs.asf.alaska.edu/data-format/)
with explicit projected x/y coordinates is a plausible later scalar-array
target. Its real layout has not been admitted; complex RSLC/SLC and auxiliary
radar/geolocation cubes are outside this profile.

**Priority:** improve established-reader v1 index traversal and metadata request
locality before expanding product families. Native selective payload acquisition
is useful here; mature h5py and explicit larger-read controls remain important
alternatives. No general HDF5 implementation, persistent cache, distributed
scheduler, merge, tag, publication, deployment or ChronoZarr change is included.
