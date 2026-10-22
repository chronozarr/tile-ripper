# Bounded SWOT Raster WSE: decision gate

**Scientifically correct native WSE subsets and matching compressed coordinates
are qualified on a bounded, provider-identity-pinned mirror. Live native archive
acquisition remains blocked by the authentication/redirect contract.** The
provider itself supports authenticated selective ranges; the native transport
still requires a directly addressable HEAD/conditional-GET object and rejects
the archive's method-specific routes. This distinction is part of the result.

## Inspected product and physical profile

The exact object is
`SWOT_L2_HR_Raster_100m_UTM19T_N_x_x_x_011_035_115F_20240216T025153_20240216T025214_PIC0_01.nc`,
**92,416,806 bytes**, in the Version C / 2.0 archive. Its opaque strong ETag is
`"33f44ccac2c294e8ece323b1850c49b3-1"`; it is not treated as a file checksum.
This is a specific older product, not qualification of the current Version D
family. [Fresh physical inspection](product-profile.json) and the
[native metadata record](native-profile.json) identify:

| Dataset | Stored type / dimensions | Physical storage | Scientific meaning |
|---|---|---|---|
| `/wse` | little-endian Float32, `(y,x)` = 1580×1580 | Four 790×790 chunks; legacy v1 B-tree; shuffle + DEFLATE level 4 | Metres above the product geoid; no scale/offset packing |
| `/x` | little-endian Float64, 1580 | One 1580-sample chunk, 1,094 encoded bytes; v1; shuffle + DEFLATE 4 | Stored pixel-center eastings, 217300–375200 m, increasing by 100 m |
| `/y` | little-endian Float64, 1580 | One 1580-sample chunk, 634 encoded bytes; v1; shuffle + DEFLATE 4 | Stored pixel-center northings, 4646200–4804100 m, increasing by 100 m |
| `/crs` | scalar character metadata container | Contiguous | CF transverse-Mercator attributes and original WKT: WGS 84 / UTM zone 19N, EPSG:32619 |
| `/wse_qual` | UInt8, 1580×1580 | One compressed chunk | Summary good/suspect/degraded/bad flag; unsupported native science dtype |
| `/wse_qual_bitwise` | UInt32, 1580×1580 | 790×790 compressed chunks | Detailed quality bits; unsupported native science dtype |
| `/wse_uncert` | Float32, 1580×1580 | 790×790 compressed chunks | WSE uncertainty, not used in the calculation |
| `/n_wse_pix` | UInt32, 1580×1580 | 790×790 compressed chunks | Contributing pixel count; unsupported native science dtype |
| `/illumination_time` | Float64, 1580×1580 | 527×527 compressed chunks | Per-pixel timing; unsupported native science dtype |

The source has superblock version 2, CF-1.7 metadata, cycle 11, pass 35, scene
115, product version 01 and PGE 5.1.1. Coverage is
2024-02-16T02:51:54.231486Z–02:52:14.254914Z. Root attributes preserve both
granule and coverage times, processing identifiers, contributing file names,
projection and resolution. The file's `reference_document` names the older
December 2023 Revision C; this inspection also consulted the official
[February 2025 Raster PDD](https://archive.podaac.earthdata.nasa.gov/podaac-ops-cumulus-docs/web-misc/swot_mission_docs/pdd/D-56416_SWOT_Product_Description_L2_HR_Raster_20250224a_RevC_clean_sig_final.pdf)
and [Version C collection documentation](https://podaac.jpl.nasa.gov/dataset/SWOT_L2_HR_Raster_2.0).
Physical details above come from the actual object, not a presumed schema.

WSE's explicit HDF5 storage fill and CF `_FillValue` agree at the stored
Float32 value represented as `9.969209968386869e36`. Axes have explicit
Float64 fill markers, with no selected missing coordinates. No calibration is
applied automatically. Original attributes, marker bytes, units and projection
are retained. These coordinates are projected metres; they are not longitude
and latitude. No resampling, reprojection, mosaicking or geographic AOI mapping
was introduced.

## Minimum implementation

The product already fitted the rank-two Float32, legacy-index, supported-filter
and explicit-source-fill contracts. Compressed rank-one axes were the missing
storage capability. The adapter now uses the same `hdf5-reader` collectors and
numeric/filter decoder for rank-one coordinate chunks. It enumerates chunks
containing requested indices, reads and decodes one at a time, and retains only
selected stored coordinates, in source order with requested positive strides.
The decode view cannot fetch any unplanned metadata or payload range.

Missing selected coordinate chunks fail explicitly, including finite HDF5
initialization that cannot establish a stored coordinate. Holes outside the
selection are allowed. Nonfinite coordinates, selected CF missing markers,
corrupt compression, unsupported indexes/filters and bad pointers fail before
successful delivery. Contiguous and compact paths remain supported. There is
no persistent full-axis cache and no new HDF5 parser. The vendor library and
unpublished selective-index patch are unchanged.

Preparation reservations charge existing prepared state, selected coordinate
storage, metadata, encoded buffers and decoded/filter working memory before
reading each coordinate payload. Local-file reads also pin and validate size,
modification time and, on Unix, device/inode/change time before/after reads and
at final validation. HTTP retains its existing strong-ETag, length and exact
range checks. Stream chunks remain provisional until successful `finish()`.

## Legitimate access and capture provenance

[Access probes](access.json) used the caller's existing Earthdata identity via
Requests/netrc. The official GET redirect chain returned exact 206 ranges with
the pinned ETag and total length, and a deliberately incorrect ETag returned
412. Original-route HEAD revalidation agreed. A GET using the HEAD-resolved
resource and a HEAD using the GET-resolved resource both returned 403. The
native source rejected the original redirect before acquiring data.

Thus an authenticated provider route is feasible, but there is no qualified
native authentication, cookie/redirect or signed-route lifecycle in this
adapter. The research helper is not that implementation. The documented
[PO.DAAC direct S3 route](https://podaac.jpl.nasa.gov/cloud-datasets/about) is
in-region access; an authenticated native S3 acquisition was not tested here.
No version check was relaxed to obtain a live-native claim.

The [sparse capture](capture.json) obtained **4,057,894 provider body bytes in
48 logical range GETs**, including metadata for independent NetCDF-C open and
all four WSE chunks needed by the declared selections. Its cap was 8 MiB.
Separate fresh inspection used 59,801 bytes in 102 bounded metadata GETs;
access probes are recorded separately. Redirect hops are listed in receipts,
so logical GET counts must not be mistaken for total wire exchanges.
Capture was revalidated against the original ETag/length before sealing extent
hashes. No entire product was downloaded. No credentials, cookies, tokens,
authorization headers or signed redirect URLs were saved.

The backing file has holes and is not a complete local product. Replay refuses
uncaptured ranges, verifies captured extent hashes and makes no provider
requests. Each scientific/benchmark receipt binds its native mirror identity
to the original provider locator, ETag/length and capture-manifest hash. This
provides a reproducible acquisition chain; it does not establish native live
provider interoperability.

## Exact science and physical acquisition

[Eighteen real-product cells](science.json) compare three selections under
full/experimental selective traversal and exact/4/16 KiB metadata reads.
Every WSE stored-value bit and validity bit agrees with h5py and NetCDF-C.
Every streamed x/y coordinate agrees with h5py; separately executed NetCDF-C
workers independently match complete WSE and x/y hashes with automatic masking
and scaling disabled. Root, science, coordinate and projection attributes agree.

Default full traversal with exact metadata reads costs:

| Selection: start; count; step | WSE samples | WSE encoded bytes | Coordinate chunk bytes | Metadata/index bytes | Total bytes | GET/HEAD |
|---|---:|---:|---:|---:|---:|---:|
| Window: (600,400); (128,128); (1,1) | 16,384 | 682,681 | 1,728 | 22,575 | **706,984** | 73/5 |
| Boundary: (780,780); (30,30); (1,1) | 900 | 3,190,637 | 1,728 | 22,575 | **3,214,940** | 76/5 |
| Strided: (77,113); (7,6); (247,283) | 42 | 3,190,637 | 1,728 | 22,575 | **3,214,940** | 76/5 |

The first window transfers **0.765%** of the original object. The other two
selections intersect all four WSE chunks and transfer 3.479%. Returned WSE
buffers contain 65,536, 3,600 and 168 bytes respectively. Requested coordinate
counts are 128+128, 30+30 and 7+6; only those centers are retained/returned.
Because this product puts each entire axis in one compressed chunk, both
1580-sample axes must be decoded: 25,280 native bytes. Multi-chunk fixtures
independently prove skipping unselected coordinate chunks.

Full and selective policies visit the same three flat index nodes and fetch
identical bytes here. Exact reads have 399 overlapping response bytes and no
repeated identical GET ranges. For the window, 16 KiB metadata pages reduce
GETs to 17 but increase total bytes to 946,553, including 50,880 duplicate bytes
and 189,088 additional unique bytes relative to the exact-read union. Metadata
page overlap with coordinate/science payloads is counted, not hidden. Detailed
phase, range, duplicate and overfetch accounting is in [acquisition.json](acquisition.json).

## Performance, resources and independent controls

The [171-cell benchmark](benchmark.json) uses three fresh processes per cell,
exact/4/16 KiB native metadata policies, full/selective indexes, exact/16 KiB
h5py controls, and separate h5py/NetCDF-C local-file controls. No build or
validation workload ran concurrently. With 20 ms added delay per loopback
response, median completion seconds are:

| Selection | Native full/exact | Native full/16 KiB | Native selective/16 KiB | h5py/16 KiB |
|---|---:|---:|---:|---:|
| Window | 2.311 | 0.724 | 1.084 | 0.652 |
| Boundary | 2.901 | 0.812 | 0.854 | 0.747 |
| Strided | 3.212 | 0.836 | 0.980 | 1.517 |

Batching helps this latency model. Selective traversal saves no I/O for these
flat indexes; timing differences between equal-read policies are not evidence
of a pruning benefit. The measured h5py control is faster for the batched
window and boundary cases. These few policies/repeats do not establish an
optimization optimum or a general performance ranking, and they do not model
the real archive's authentication/redirect latency or network bandwidth.

At zero added delay, native full/exact completion medians are 24.90, 44.51 and
46.14 ms. Local sparse-file native medians are 9.08, 23.46 and 24.59 ms; local
h5py medians are 10.15, 28.99 and 29.41 ms; local NetCDF-C medians are 14.28,
32.93 and 33.26 ms. Local kernel byte ranges were not instrumented. First native
result is a provisional chunk; mature first result is a complete slice. Native
uses five HEADs versus two for range-backed h5py. Process CPU includes startup
and imports; mature API CPU is retained separately. Full tables of preparation,
first result, completion, CPU, RSS, counts and reservations are in
[performance.md](performance.md) and [performance-summary.json](performance-summary.json).

All product reservations fit the declared 32 MiB working budget. The largest
product reservation is 24.19 MiB and the maximum measured native benchmark RSS
is 28.23 MiB. The coordinate preparation peak is reported separately. The
[axis resource gates](axes-resources.json) consume 1 and 64 MiB of Float32 output
at 4/8 MiB working budgets, with peak native RSS 12.78–21.08 MiB and declared
stream reservations within budget. Oversized coordinate chunks and an encoded
extent within a larger file fail working-memory admission before payload read.
Working reservations are not a process RSS cap; runtime, inspection and caller
retention retain their documented scope. The native executable links no libhdf5.

## Scientific usefulness and remaining limits

The window spans stored easting centers 257300–270000 m and northing centers
4706200–4718900 m. Of its 16,384 WSE samples, **5,082 are finite and differ from
the exact source fill marker**. Their Float64-accumulated sum is
1,347,301.3108596802 m and arithmetic mean is **265.1124185083983 m**; the range
is 124.10143280029297–375.1822814941406 m. Rust streaming, h5py and separately
executed NetCDF-C reproduce the count and sum. These are unchanged stored WSE
values in metres above the product geoid, with no scale/offset or additional
geophysical correction applied.

This is a descriptive spatial summary, not a validated river elevation estimate.
Its only selection criteria are the requested indices, finite values and the
source CF fill marker. Basic fill/finite masking is not SWOT science-quality
filtering. More sophisticated work needs aligned `wse_qual` and
`wse_qual_bitwise`, WSE uncertainty, contributing pixel counts, water fraction,
ice/dark-water/layover context and appropriate waterbody/river association.
Per-pixel illumination times matter for time-sensitive analysis. UInt8/UInt32
quality data and Float64 time arrays need explicit science pixel contracts;
they were not silently cast into this workflow. No uncertainty-weighted mean,
profile slope, river connectivity, time aggregation or geographic AOI claim is
made here.

## Validation, generalization and next decision

The [verification receipt](verification.json) binds the final native executable
and production source hashes. Passed: 18 real SWOT comparison cells, three
independent NetCDF-C WSE/axis/hash summaries, 146 axis science cells, 36 axis
fault/policy cells and five axis resource cases. Axis cases include both byte
orders and six losslessly Float64-representable coordinate types, descending
order, partial chunks, strides, skipped chunks, compact/contiguous paths,
packing, source initialization versus CF fill, NaN bits and mutation/cancellation.
Fault-policy counts include deliberate acceptance of unselected coordinate holes.

Inherited full and selective HDF5 science/fault gates, full-versus-selective
structural scope checks, resource gates, COG, Zarr v2/CF, ordinary/sharded Zarr
v3, streaming and durable-publication/fault gates pass. Default/no-default/
native-cog/native-hdf5/all-features Rust tests and warning-denying all-target
Clippy pass, as do all 269 unchanged vendor unit tests and vendor Clippy.
The existing bounded GOES mirror also passes six repeated real-source
full/selective selections and all 123 inherited NetCDF-C coordinate audits,
with no additional provider traffic.
Supporting receipts are under [inherited/](inherited/) and reproduction commands
are in [workflow.md](workflow.md).

The axis extension generalizes naturally to other NetCDF-4 products with the
same restricted rank-two scalar science, rank-one coordinate, dtype/filter,
legacy-index and fill contracts. It does not establish general NetCDF-4/CF
support, arbitrary dimensions, curvilinear geolocation or modern indexes. Full
traversal remains the default; experimental selective traversal cannot validate
unread subtrees.

The next product milestone should make this SWOT workflow quality-aware through
explicit stored UInt8/UInt32 quality contracts, alongside a separately reviewed
caller-credential and archive-redirect transport contract before claiming native
live access. Keep NISAR GCOV separate: its inspected simulated sample still has
implicit HDF5 zero initialization and a CF NaN marker, which remains a deliberate
rejection. Resolve upstream initialization semantics before a NISAR qualification;
do not substitute zero or NaN by assumption. Stored GOES Int16 is another
separate numeric-contract opportunity. No modern index family is justified by
the inspected SWOT/NISAR evidence.

Changes are committed locally on `codex/acquisition-planner`. No upstream issue
or PR, merge, publication, release, deployment or ChronoZarr change is part of
this milestone.
