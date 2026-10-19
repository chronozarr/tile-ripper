# Zarr decision gate

The local Zarr workflow works for its documented regular dataset layout. The
shared planner holds at the logical request/read/callback boundary after one
concrete correction: layouts and callbacks needed a variable selector rather
than a mandatory raster-band number. The callback now owns a plain UInt16
vector, removing the GDAL buffer type from the common API. This is evidence for two native regular
raster formats, not a universal scientific acquisition engine.

A scientist with a supported local Zarr group can select bounds, dates and
variables and obtain native subsets without writing Python. Python appears in
the deterministic demo/oracle, not the acquisition implementation. This is **not**
a remote/cloud Zarr release or compatibility claim for arbitrary xarray stores.
The input contract is explicit in [the workflow guide](zarr-workflow.md).

`zarrs` 0.22.10 and its filesystem storage backend own chunk retrieval, codecs,
fill and writes. The shared `Request`, `plan`, `execute`, `fetch`, native spatial
math, DAG dependencies, budgets, variable identity and fallible callback apply
to COG and Zarr. No TIFF decoder, Zarr decoder, downloader, mosaic or third format
was added. Zarr uses the existing dataset-slice read variant, which permits the
library to retrieve relevant chunks without prescribing transport behavior.
The original root-array chunk-object planning probe still passes its tests.

Format-owned details remain format-owned: coordinate validation, time-axis
lookup, chunk layout/fill, metadata and output encoding. Native raster buffers
are still UInt16 spatial planes and the affine model is still regular/north-up;
these are declared capability limits shared by both implemented paths. Mixed
selector semantics in one request, arbitrary rank/dtype and mutable-store
snapshots remain unsupported. There is no reason to generalize them speculatively.

## Evidence and performance interpretation

[zarr-benchmark-results.json](zarr-benchmark-results.json) records individual
runs and gates. The fixture includes gzip, two variables, irregular dates,
nodata, an absent chunk, different variable chunk shapes, dot/slash keys, nonaligned spatial bounds and source-edge clipping.
The independent xarray/Zarr oracle checks raw stored values, dtypes, coordinates,
attributes, fill, changed affine origin, and CF-decoded scale/offset/nodata/dates.
Five main extracts validate 149,520 native data values in total; additional
cases test other dates, windows and failures. This is deterministic scientific
correctness evidence for the contract, not validation against a public archive.

The bounded Rust path revisits source chunks at output-window boundaries;
xarray's larger subset read retrieves fewer objects and bytes. See the exact
medians recorded below, generated from the final runs. RSS includes each runtime
and GDAL dependencies. Neither the lower Rust RSS nor any individual timing
establishes a scaling advantage; startup dominates much of this tiny local case.
There is no demonstrated speedup over a tuned xarray/Zarr workflow. Transfer
and decoding are fused in the underlying APIs; the combined time is reported
honestly. Network throughput, provider limits and remote request efficiency
were not measured because remote stores are outside this implementation.

## Recommendation

**Unify the CLI request/discovery boundary before a third format.** Direct COG
and Zarr already share `plan`/`fetch`; STAC still has `stac-plan`/`stac-fetch` and
a separate request/prepared wrapper. The next small milestone should make
source discovery produce the same format-neutral request/prepared lifecycle,
retain explicit sink/capability checks and distinguish axis selection from
source discovery time. Keep the current narrow readers underneath it.

Do not add HDF5/NetCDF next. First remove front-door duplication, then decide
whether remote Zarr support or measured chunk reuse is the more valuable next
small task. Local Zarr limitations are material: no remote authentication,
mutable-store snapshot protection, polygons/reprojection, additional dtypes,
CF auxiliary coordinates, v3/shards or unbounded coordinate axes. No large
ecosystem study or generalized caching/mosaicking engine is warranted.

Stop here. No merge or publication was performed.

## Final local benchmark medians (five runs per reader)

| Reader | Wall (s) | Peak RSS (MiB) | Data get calls | Encoded bytes | Read + decode (s) | Data write callback (s) |
|---|---:|---:|---:|---:|---:|---:|
| rust | 0.434 | 36.2 | 110 | 297646 | 0.0101 | 0.1740 |
| xarray | 0.338 | 78.6 | 41 | 109903 | 0.0037 | 0.0105 |

These local storage calls exclude metadata/coordinate retrieval. Rust counts
missing get calls as well; Python counts DirectoryStore retrieval attempts.
Write phases have different boundaries: Rust reports data callbacks, xarray
reports its complete output call. Coordinate/metadata setup and startup are
therefore included in wall time but are not an additive phase decomposition.
The output representations share native values and metadata, but differ in
chunking and encoding. Treat the counts and timings as workload observations.
