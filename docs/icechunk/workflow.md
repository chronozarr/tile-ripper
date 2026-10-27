# Snapshot-pinned Icechunk acquisition

The opt-in `icechunk` build adds a read-only Icechunk source to the existing
`plan`, `stream` and `fetch` commands. It resolves an explicitly selected
repository version, reads the selected native values, and writes ordinary
scientific Zarr output. This is source-alpha work, not a published binary or a
promise of generic Icechunk, arbitrary-rank Zarr, or GRIB support.

The two supplied requests select separate Dynamical GFS analysis products:

- [Materialized Float32](../../examples/icechunk-gfs-materialized.json): time-optimized
  sharded Zarr chunks inside a snapshot-pinned Icechunk repository.
- [Virtual Float64](../../examples/icechunk-gfs-virtual.json): a snapshot mapping
  to original GRIB messages, decoded through the maintained Gribberish library.

They are explicit scientific source choices. The selected values differ because
the materialized product has documented binary rounding; the implementation does
not silently substitute one for the other based on speed. See the
[independent source comparison and limits](reference.md).

## Build and acquire

Use Rust 1.91.1 or newer for this optional dependency set. The ordinary default
build is unchanged. For an Icechunk-only build without GDAL:

```sh
cargo build --locked --no-default-features --features icechunk --bin tile-ripper

target/debug/tile-ripper plan examples/icechunk-gfs-materialized.json --gdal-free --human
target/debug/tile-ripper fetch examples/icechunk-gfs-materialized.json \
  --gdal-free --output ./gfs-materialized-subset --json

target/debug/tile-ripper plan examples/icechunk-gfs-virtual.json --gdal-free --human
target/debug/tile-ripper stream examples/icechunk-gfs-virtual.json --gdal-free
```

The examples select four cells at 2026-10-25T00:00Z, retaining the source WMO
sphere CRS, latitude ordering, Celsius units and native Float32/Float64 type.
Choose a new destination for each fetch. Results are usable only after successful
command completion and a complete provenance manifest; stream chunks are
provisional until the final completion event. Ordinary CLI `stream` emits chunk
descriptors; the Rust streaming API supplies native values.

A small selection can still require a large decode. The materialized example's
logical source chunk is 14,400,000 decoded bytes, even though its four values
occupy 16 bytes. The virtual example reads one complete model-grid GRIB message
and produces 8,305,920 decoded Float64 bytes before spatial slicing. A shard is a
physical container of logical chunks; the materialized reader requests its
selected encoded subchunks rather than downloading the entire shard.

## Select a version

An Icechunk asset uses `"format": "icechunk"`. The locator is the repository's
anonymous HTTPS root plus one optional version selector:

```text
https://example.org/data.icechunk#snapshot=SNAPSHOT_ID
https://example.org/data.icechunk#tag=RELEASE_NAME
https://example.org/data.icechunk#branch=main
```

Without a selector, a fresh preparation resolves `main`. Branches and tags are
resolved once into a snapshot; reads from that prepared acquisition keep using
it. The resolved snapshot, selected physical payload identities and reference
ranges appear in provenance. A source identity failure requires a fresh explicit
preparation; execution does not silently switch to a newer branch tip or another
product. Repository credentials and query-bearing signed URLs are outside this
public anonymous source profile.

The installed feature is necessary even when the selected Icechunk chunks are
ordinary Zarr data. A plain Zarr reader cannot resolve Icechunk snapshots and
manifests by treating the repository URL as a Zarr directory.

## Scope and accounting

The admitted path follows the existing bounded time-first three-dimensional
scientific Zarr selection, with native spatial bounds and explicit source CRS.
Array metadata must declare default `/` chunk keys; other encodings fail before
data planning rather than being interpreted as missing chunks.
Use the JSON requests rather than assuming the simple direct-selection flags
admit an Icechunk URL. Unsupported dimensions, codecs, GRIB templates and
selection/grid layouts fail; catalog membership alone is not qualification.
Forecast cubes with separate `init_time` and `lead_time`, vertical levels,
curvilinear grids, arbitrary calendars, reprojection and temporal compositing
are not established by these two analysis examples.

Virtual GRIB admission is narrower than the underlying decoder. It accepts one
GRIB2 field with packing template 0, 2, 3, 41 or 42, after checking section framing,
grid and packed point counts, bitmap length and group descriptor bounds. Complex
packing reference and group widths are at most 31 bits; descriptor alignment
products are at most 2²⁴ bits. PNG packing requires a bounded noninterlaced sample
layout and IHDR/IDAT/IEND chunks only; ancillary and APNG chunks are refused.
CCSDS scratch dimensions must fit the admitted grid. Unsupported packing fails
before decode; this is not qualification of arbitrary GRIB files or templates.

The existing scheduler, source-completion checks, physical conditional HTTP
transport and resource admission remain in use. The maintained Icechunk reader
resolves repository metadata and references. Its internal HTTP and allocations
are not fully represented by Tile-Ripper's physical-payload transport counters;
`metadata_http_bytes: null` means unmeasured, not zero. The payload budget is not
a process RSS ceiling. The independent forwarding benchmark records complete
response-body traffic for its own reference reads, with its measurement limits
stated separately.

Virtual references can outlive the external file they name. The qualified
sample's reference includes no originating object checksum. Runtime ETag checks
protect the selected object's identity during acquisition but do not prove that
an external GRIB object still matches its contents at snapshot creation. That
limitation stays explicit in the source evidence.

## Use a materialized subset in Chronozarr

The materialized output is ordinary Float32 Zarr. Chronozarr's existing converter
can turn it into a playback store, retaining its Celsius units and WMO sphere
grid. From a Chronozarr checkout with its `geo` extra installed:

```sh
uv run --no-sync chronozarr convert /path/to/gfs-materialized-subset/gfs.zarr \
  ./gfs-playback --variable temperature_2m --nodata nan
uv run --no-sync chronozarr preview ./gfs-playback
```

A local 24-hour, 2 × 2 subset passed this conversion with exact Float32 values
and source CRS semantics. The virtual output remains Float64 in Tile-Ripper;
Chronozarr requires an explicit precision conversion for that type. Its direct
Icechunk import offers that conversion and records it in provenance.

## Evidence

- [Independent reference sources, values, read granularity and reproduction](reference.md).
- [Python source comparison receipt](reference-receipt.json).
- [Matched selected logical payload in legacy Zarr and materialized Icechunk](legacy-comparison.json).
- [Independently parsed virtual reference and checksum limitation](virtual-reference.json).
- [Implementation acceptance record](implementation-acceptance.json), when the
  implementation checks have completed.

No general provider speed advantage follows from these examples. A useful
comparison uses the same selected values, snapshot and physical-source
identities and reports preparation, payload transfer, decode, validation and
output costs separately. The materialized/virtual comparison instead describes
the cost and precision tradeoff between two deliberately selected products.
