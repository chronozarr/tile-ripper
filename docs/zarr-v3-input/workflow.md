# Native scientific Zarr v3 subsets

Use the existing `acquisition::prepare` / `Prepared::execute` / `into_stream`
interface, or the CLI with a request JSON declaring `format: "zarr_v3"`.
The native and Zarr-only builds require no GDAL or Python runtime:

```sh
cargo build --release --locked --no-default-features --features native-cog
./target/release/tile-ripper plan examples/zarr-v3-gfs.json --human
./target/release/tile-ripper stream examples/zarr-v3-gfs.json --gdal-free
./target/release/tile-ripper fetch examples/zarr-v3-gfs.json --gdal-free \
  --output ./gfs-native-subset
```

The example selects two timestamps and a small native rectangle from the public
GFS archive. Its explicit CRS describes a sphere of radius 6,371,229 m. It is
preserved verbatim; substituting EPSG:4326 would change that scientific metadata.
Direct geographic CLI selection flags remain the existing v2/STAC convenience
interface. Use JSON to provide v3's exact native CRS and selectors.

## Admitted source profile

| Property | Supported |
| --- | --- |
| Group | v3 `zarr.json`; ordinary child metadata or inline consolidated metadata |
| Data | Top-level UInt16 or Float32 arrays, rank three, time first |
| Dimensions | `(time,y,x)` or `(time,x,y)`, with `lat/lon` or `latitude/longitude` aliases |
| Spatial coordinates | Ordinary Float64 vectors, at least two finite, regular monotonic centers; either direction; explicit native CRS |
| Time | Ordinary Int64 vector, strictly increasing; proleptic Gregorian; seconds, milliseconds, microseconds, nanoseconds, hours or days since a UTC epoch |
| Epoch syntax | ISO date, or ISO date plus time using space/`T` and optional `Z`; no non-UTC offsets |
| Selection | Native rectangular cell intersection; inclusive interval or noncontiguous explicit instants; original coordinate/time values retained |
| Chunks | Regular grids; library-supported default or v2 chunk-key encoding and dot/slash separator |
| Shards | One `sharding_indexed` codec; regular inner shapes dividing outer shapes; start/end index |
| Inner codec | Little-endian bytes, optionally gzip level 0–9, Zstd, or Blosc LZ4/Zstd with no shuffle, byte shuffle or bitshuffle; matching native typesize, automatic blocksize |
| Index codec | Little-endian bytes, optionally followed by CRC32C |
| CRS | CF `grid_mapping` metadata with `crs_wkt`, `spatial_ref` or authority string; `coordinates: spatial_ref` also recognized; existing explicit native grid metadata accepted |
| Missingness | Native storage fill, CF scalar `_FillValue`/`missing_value`, Float32 NaNs; 4/8-byte floating base64 CF markers recognized |
| Mask arrays | Explicit UInt16 dataset selectors can carry mask values; derived `Chunk::validity()` follows CF/NaN semantics |
| Transport | Anonymous HTTP(S), strong per-object ETags, conditional requests, exact 206 range framing; local immutable stores also supported |

V3 storage fill is a stored value, not an automatic scientific missing marker.
An omitted UInt16 chunk with storage fill zero therefore emits **valid zeros**
unless CF declares zero missing. NaNs remain invalid. Finite missing markers
are normalized to the native dtype; conflicting CF markers fail. Coordinates
must have no missing samples. Scale/offset remain metadata; raw samples and
Float32 NaN payload bits are preserved without calibration or conversion.

External ancillary masks, auxiliary coordinates/bounds/cell measures, nonregular
or curvilinear grids, sharded coordinates, arbitrary calendars, other dtypes,
non-time-first data, storage transformers, nested sharding, outer compression,
lossy/filter pipelines, reprojection and polygon masks are rejected. A profile
requiring whole-shard decoding fails explicitly; there is no full-shard fallback.
Admission and the scheduler may reject otherwise valid chunks/indices that exceed
the supplied buffer or node limits.

## Physical reads and completion

The planner enumerates unique logical inner chunks and their physical objects.
It pins object sizes/ETags, fetches each selected present shard index once, and
uses zarrs to decode that index. Checked arithmetic rejects corrupt, overlapping,
index-intersecting or out-of-object payload ranges. The library's encoded
subchunk API retrieves selected compressed payloads; established bounded codec
libraries and zarrs decode the ordinary inner chunk. No replacement Zarr
encoder, decoder or whole-shard cache is used.

A prepared acquisition retains its index cache across workers and repeat calls.
Missing-inner markers are handled before invoking zarrs 0.23.14's encoded
subchunk helper, which otherwise adds their sentinel offset/length unchecked.
Valid missing chunks synthesize standard native fill through the library.

Every index/payload request uses the same pinned strong ETag. Preparation,
pre-execution and final validation cover metadata, coordinates, present data
objects and planned absences. Local shard identities are captured before index
inspection and checked through completion. These are per-object checks, not an
atomic dataset snapshot or a defense against a server misreporting versions.

Chunks remain provisional until successful `finish()`. Cancellation, source
changes, exhausted retries and corrupt data cannot produce successful completion.
HTTP requests in flight may finish before cancellation is observed.

`Prepared::explain()` records physical versus logical shapes and access mode.
The execution receipt's `transport` includes request attempts, retries, status,
actual response-body bytes, phases and exact range identities. Index traffic
occurs during preparation; `source_data_*` measures selected payload reads.
Those fields exclude HTTP headers, TLS overhead and bytes the client never read.
Counts are cumulative for a prepared acquisition; repeat executions legitimately
fetch payloads again. Indices are not downloaded again.

The scheduler reserves retained v3 metadata/coordinate/index storage, two native
chunk buffers, encoded payload buffers and output planes. Process/runtime,
plan bookkeeping, transport overhead and caller-retained chunks differ; this
is not an RSS ceiling. Indices have an aggregate half-buffer admission limit.
The [measurements and remaining limitations](report.md) include actual RSS.

## Output boundary

Streaming keeps native dtype, raw values, CF validity, coordinates and exact
RFC3339 timestamps. Native group materialization uses the established Zarr
input adapter's **uncompressed v2** subset writer. It normalizes data to time,
y/latitude, x/longitude order, copies mapping/coordinate metadata and records
original source metadata in the manifest. It uses CF missing markers for v2
fill; without such a marker, valid v3 storage fill remains valid in v2/xarray.

This group writer keeps its existing completion/error-cleanup behavior; a
consumer must require the successful completion receipt. It does not acquire
the separate COG scientific writer's atomic sibling-publication/durability
contract. That validated writer and its synchronization sequence are unchanged.
Experimental v3 output remains feature gated and requires an explicit encoding.
No output default, release, merge, publication or deployment is changed here.
