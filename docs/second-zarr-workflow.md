# Satellite precipitation through the shared acquisition lifecycle

A second independently produced public Zarr store now works without custom
Tile-Ripper metadata: NASA POWER's IMERG daily spatial store. It is the provider's
original published Zarr, not a Tile-Ripper repackaging. The same Rust library and
`plan` / `fetch` commands also retain WeatherBench2, local Zarr and STAC/COG support.

## Retrieve a small real subset

From the repository root, with the established Rust and system GDAL environment:

```sh
cargo build --release --locked --bins --examples
./target/release/tile-ripper plan examples/imerg.json --human
./target/release/tile-ripper plan examples/imerg.json --json
./target/release/tile-ripper fetch examples/imerg.json --output ./imerg-subset
```

Choose a new output directory. No Python, credentials, paid resources or cloud
configuration is required. The JSON request names the public HTTPS endpoint,
`IMERG_PRECTOT` and `IMERG_PRECLIQUID_PROB`, bounds `[-80.02,35.02,-79.68,35.28]`,
and 2020-06-01 UTC. Output contains `imerg.zarr` and the schema-3 completion
manifest. It has one date, three latitude centers, five longitude centers and
two separate variables, in native `mm/day` and `%` units: 30 Float32 values.
This store uses separate named arrays, so no artificial band stacking or new
band-axis selector is needed.

`plan` downloads bounded metadata/coordinate objects and HEADs selected data keys;
it never GETs the scientific data chunks. Exactly two source chunks are needed
for this request, one per variable. They transfer 1,347,071 encoded bytes and
materialize 12,960,000 logical Float32 bytes, delivering 120 value bytes. This
large ratio comes from the source's 900×1800 spatial chunks, not excessive chunk
selection. Use the existing [WeatherBench2 example](cf-zarr-workflow.md) for the
contrasting global atmospheric product.

Both paths use the same prepared library lifecycle:

```rust
let prepared = tile_ripper::acquisition::prepare(
    &request, &mut tile_ripper::cog::LayoutCache::default()
)?;
let inspection = prepared.explain();
let manifest = prepared.fetch()?;
```

`prepared.execute` retains the bounded synchronous native-chunk callback.
`Chunk::pixels` remains the native UInt16/Float32 enum. An additive `coordinates`
field carries actual rectilinear center vectors for Zarr chunks; `pixel_center()`
uses them so Float32-rounded source coordinates remain exact. COG continues to
use its affine. Code constructing Chunk literals must supply the new field
(`None` for affine-only buffers). Keeping received chunks is the consumer's
responsibility. Callback results remain provisional until execute returns
success; already-consumed chunks cannot be retracted after a later failure.

## Metadata and scientific decisions

[The provider's documentation](https://power.larc.nasa.gov/docs/services/aws/)
recommends the public analysis-ready Zarr datastore. [Inspected metadata and
candidate assessment](second-zarr-inspection.json) record the exact v2 layout:
`(time,lat,lon)` Float32 variables, Float64 geographic centers, Int64 days since
1998-01-01, proleptic Gregorian, Blosc/Zstd bitshuffle and a Float32-to-Float32
fixed-scale-offset storage filter. The existing pinned zarrs library already
implements these codecs; no dependency, codec algorithm or downloader was added.

New profile rules are limited to this evidence:

- `lat`/`lon` geographic aliases alongside existing `latitude`/`longitude`, with
  compatible units/axis and case-insensitive semantic standard names when present.
- The verified daily time encoding, with checked timestamp conversion. Native
  coordinate values, calendar and units are preserved.
- Declared positive geographic spacing can admit producer Float32 rounding in
  Float64 centers. Residual tolerance is bounded by Float32 precision and capped
  at 0.1% of a cell. Real irregular grids remain rejected. Native coordinate
  midpoints determine cell intersections; approximate affine arithmetic does not
  decide these boundaries. Both directions are supported. Stored coordinates and
  callback centers are authoritative; the affine is explicitly approximate.
- Blosc LZ4/Zstd, shuffle 0/1/2, automatic block size, admitted compression levels,
  and one finite, positive-scale Float32-to-Float32 `fixedscaleoffset` filter.
  Other filter chains, type conversions and auxiliary grid conventions fail.

The source storage filter is decoded **once**. Output stores those exact logical
Float32 values without applying the lossy filter again; its filter list is null.
This is codec decoding, not an added calibration or unit conversion. NaNs remain
NaNs, including sparse/missing scientific chunks. Original array codec metadata
and group attributes are retained in `manifest.source_metadata` for provenance.
Other attributes and variable identities/units are retained in the native group.
Subset geographic extents and time coverage are recalculated; stale global bounds
and ambiguous source coverage duration are not asserted on the subset. Source
history/creation metadata remain source provenance, not a claim of output age.

The geographic CRS label remains the documented EPSG:4326 profile interpretation,
not a source-declared precise datum. There is no datum transformation, reprojection,
compositing, mosaicking or arbitrary curvilinear support. The previous custom-grid
UInt16 profile remains compatible. Generic CF, arbitrary calendars, additional
spatial/dimensional conventions, packed band axes, signed-int16 scientific variables,
Zarr v3, authentication and HDF5 remain outside this milestone.

## Validation efficiency without weakening v2 snapshots

Metadata/coordinate bodies were already cached per prepared instance. They are
not repeatedly downloaded during execution. Actual v2 metadata and coordinates
remain ETag-pinned and revalidated before/after each execution. Scientific objects
remain conditional GETs; missing scientific keys remain pinned and revalidated.
No TTL, provider immutability assumption or cross-prepared speculative cache exists.

The removed requests were repeated HEADs for **admission-only v3-marker absences**.
Execution explicitly constructs v2 arrays from pinned metadata; it never invokes
a resolver that could select a newly appeared v3 marker. Initial v3/hybrid stores
still fail admission. Adding an unrelated v3 marker after preparation cannot alter
this fixed v2 read, and does not invalidate it. Changes to actual metadata,
coordinates or scientific objects still fail. This is per-dependency snapshot
safety, not an atomic snapshot of every object under a prefix. Local filesystem
snapshot behavior remains its established contract.

Transport observations now include per-key attempts, bytes, statuses, retries and
seconds grouped by planning, preparation validation, pre-execution validation,
execution, output finalization and post-execution validation. Finalization has no
source HTTP requests. Warm observations are cumulative over a prepared object's
lifetime; subtract consecutive reports for individual warm traffic.

WeatherBench2 preparation plus two fetches drops from 75 to 50 requests, removing
25 redundant HEADs with identical 1,231,715 response-body bytes and two scientific
GETs. The matched xarray baseline makes 46 requests; its redundant root-array
admission check is likewise excluded from revalidation. Content hashes or a stable
metadata ETag alone cannot remove checks of mutable coordinate/data objects.
Cached metadata is reused only alongside the remaining dependency checks.

Gzip fixed-output allocation checks and Blosc size/type/encoded-length/blocksize
checks remain. Stream codec and bitshuffle flags must match the admitted metadata;
memcpy blocks invoke no compression algorithm. The selected filter keeps encoded
and logical element widths equal. There is no new unbounded decompression path.
`buffer_bytes` still bounds admitted payload components, not process RSS or all
runtime/codec scratch. Blosc lacks a checksum; versions do not detect all arbitrary
bit corruption or create an atomic dataset version. Caught failures clean up a
newly owned destination; completion manifests are written last.

## Reproduce checks and benchmarks

Use the existing frozen Zarr oracle environment; no new Python packages:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
prototype-output/zarr-env/bin/python bench/second_zarr.py checks
prototype-output/zarr-env/bin/python bench/second_zarr.py benchmark
prototype-output/zarr-env/bin/python bench/cf_zarr.py checks
prototype-output/zarr-env/bin/python bench/remote_zarr.py checks
```

For before/after measurements, build commit `407e642` in a separate temporary
source directory (for example with `git archive`), retaining its release CLI and
`remote_zarr` example. Then:

```sh
prototype-output/zarr-env/bin/python bench/validation_efficiency.py \
  --before /path/to/407e642/target/release/tile-ripper \
  --before-library /path/to/407e642/target/release/examples/remote_zarr
# Same local fixture, interleaved binaries, fixed 10 ms request latency:
prototype-output/zarr-env/bin/python bench/validation_efficiency.py \
  --before /path/to/407e642/target/release/tile-ripper \
  --before-library /path/to/407e642/target/release/examples/remote_zarr \
  --controlled-only
```

The public comparison has three fresh-process repetitions per reader, each
preparing once and fetching twice. Cold is preparation + first fetch, warm is a
second validated metadata snapshot fetch with no scientific data-body cache.
Provider/TLS/OS caches are not purged; before/after public batches are sequential.
The fixed-latency control interleaves binaries against one source/server.

The independent xarray/Numcodecs baseline uses consolidated metadata, persistent
HTTPS, conditional identities, the same logical values/coordinates/units and
unfiltered destination Float32 arrays with matching chunks/compression/durable
object writes. It materializes the selected array; Rust uses bounded buffers.
A shared diagnostic manifest template matches serialization work. Actual file
counts/bytes are measured, not assumed equal. Reference/template acquisitions are
outside timing; no full dataset is downloaded. Nested stage timers overlap;
read/decode minus retrieval includes library assembly and is not pure codec CPU.
All bytes are HTTP entity-body bytes, excluding headers/TCP/TLS. See the
[decision gate](second-zarr-decision.md) for exact measurements and limits.
