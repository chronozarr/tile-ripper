# Actual product layouts and remaining coverage

[product-access.json](product-access.json) contains bounded observations of
specific files, not claims about every version in a product family.
`H5Dget_chunk_index_type` returned zero for the inspected chunked datasets:
the [HDF5 public enum](https://github.com/HDFGroup/hdf5/blob/hdf5_1_14_6/src/H5Dpublic.h)
identifies this as the legacy v1 B-tree. These files do **not** establish a
requirement for modern index families.

| Actual object | Observed science/axes | Current restriction |
|---|---|---|
| NOAA GOES-19 ACHTF, 28,545,639 bytes | `/TEMP`: UInt16, 5424×5424, 226×226 chunks, v1; shuffle/DEFLATE. Contiguous stored rank-one axes. | Independently qualified in this experiment, within the existing profile. |
| GOES-19 CMIPF C01, 55,166,576 bytes | `/CMI`: stored signed Int16, 10848×10848, 226×226 chunks; shuffle/DEFLATE. | Stored Int16 is outside `Pixels::{UInt16,Float32}`. Requires an explicit stored Int16 pixel contract. |
| GOES-19 L1b RadF C01, 49,693,652 bytes | `/Rad`: stored signed Int16, 10848×10848, 226×226 chunks. | Same numeric-type restriction. |
| Inspected SWOT Raster Version C object, 92,416,806 bytes | `/wse`: Float32, 1580×1580, four 790×790 chunks, v1, shuffle/DEFLATE. `/x`,`/y`: Float64, one compressed chunk each, also v1. | Coordinates must currently be contiguous or compact; these actual axes are chunked/compressed. The native HTTP source also rejects the archive's redirects. |
| Inspected NISAR simulated GCOV sample, 171,966,464 bytes | `.../frequencyA/HHHH`: Float32, 6220×4545, 117 allocated 512×512 chunks, v1, shuffle/DEFLATE. Float64 x/y axes are contiguous. | CF `_FillValue=NaN` accompanies default HDF5 initialization of zero. The current upstream reader can substitute CF fill for implicit storage initialization, so Tile-Ripper deliberately rejects this case. Archive-access handling is also outside the native transport contract. |

The NISAR observation is a specific **simulated sample**, linked from
[NASA's sample-data page](https://science.nasa.gov/mission/nisar/sample-data/).
It is not qualification of production GCOV. The
[current ASF access guide](https://nisar-docs.asf.alaska.edu/earthaccess/)
requires Earthdata authentication for production streaming. The inspected SWOT
object was accessed through an existing Earthdata identity. NISAR sample GETs
used the public sample route. No credentials or signed URLs were recorded.

The archive routing distinction matters. Official HEAD routes resolve to
strong ETag/size identities, but issuing GET against the URL returned by HEAD
failed. Conditional range GETs through the **official GET redirect chain**
succeeded. Conversely, HEAD against the GET-resolved URL failed. Bounded
reference inspection therefore used the archive's legitimate method-specific
routes and revalidated the original identity. Tile-Ripper's current HTTP layer
requires one directly addressable object with strict HEAD and conditional GET
semantics and rejects redirects. This research helper is not a native adapter.
It does not refresh signed URLs inside an acquisition or establish a new
authentication contract.

Other anonymously inspected GOES candidates and rejection details are retained
in [candidate-products.json](candidate-products.json). Several smaller UInt16
products fit the important storage criteria; matching those criteria alone is
not complete scientific qualification. Newer fixed-array indexing remains an
explicit tested rejection, as do unsupported numeric/filter profiles. No actual
inspected SWOT/NISAR file here needs a modern index family. Complex covariance
terms, RSLC/SLC, higher-rank auxiliary data and multidimensional coordinates
remain outside the rank-two scalar science/rank-one-axis profile.

The next narrow product milestone should support **bounded chunked rank-one
coordinate reads**, then qualify the inspected SWOT Raster selection through
a deliberate archive-access contract. In parallel with an upstream contribution,
correct separation of implicit HDF5 initialization from CF missingness is the
specific prerequisite exposed by NISAR. Stored Int16 is a separate, observed
GOES coverage opportunity. None of these adapters, numeric variants, transport
changes or modern index families was implemented in this experiment.
