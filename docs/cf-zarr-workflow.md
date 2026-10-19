# Conventional geographic Zarr v2

Tile-Ripper can now acquire a small, independently produced WeatherBench2 ERA5
subset without adding custom metadata to the source. The same Rust prepared
lifecycle and CLI handle custom-grid UInt16 Zarr, conventional Float32 Zarr and
existing STAC/COG requests.

## Copyable real-data demonstration

Requires the documented Rust/system-GDAL build environment. No Python is needed
to plan or fetch. Public HTTPS requires no account, token, Google SDK or cloud
configuration. Choose a new output directory.

```sh
cargo build --release --locked --bins --examples
./target/release/tile-ripper plan examples/weatherbench2.json --human
./target/release/tile-ripper plan examples/weatherbench2.json --json
./target/release/tile-ripper fetch examples/weatherbench2.json --output ./era5-subset
```

The source is WeatherBench2 ERA5
`1959-2023_01_10-6h-64x32_equiangular_conservative.zarr`, variable
`2m_temperature`, on public Google Cloud Storage HTTPS. The request selects
1959-01-01 00:00 and 06:00 UTC and native bounds `[10,30,25,50]` in EPSG:4326.
It retrieves one 580,139-byte scientific chunk plus metadata/coordinates, writes
`era5.zarr` and a completion `manifest.json`, and returns 24 raw Float32 values
in kelvin. The native chunk contains 100 times and the full 64×32 geographic grid;
Tile-Ripper delivers just two times and a 3×4 spatial subset. This transfer
amplification is intrinsic to this source layout.

Bounds include cells with positive-area overlap, using centers ± half the regular
spacing, not just centers inside the rectangle. The resulting cell edges can
extend beyond the requested bounds. This is tested independently against xarray.
Coordinates and time values retain their native order and encoding; no interpolation,
calibration, compositing, mosaicking, reprojection or longitude wrapping occurs.
`plan` downloads metadata/coordinate chunks and HEADs selected data keys, but
never GETs scientific data chunks. Saved plan JSON is inspection-only; `fetch`
prepares from the request again. Library preparation can be retained across runs.

## Library and native samples

```rust
let prepared = tile_ripper::acquisition::prepare(
    &request, &mut tile_ripper::cog::LayoutCache::default()
)?;
println!("{}", prepared.human());
let manifest = prepared.fetch()?;
```

For a bounded synchronous consumer, use `prepared.execute(|chunk| { ...; Ok(()) })`.
`chunk.pixels` is now `Pixels::UInt16(Vec<u16>)` or `Pixels::Float32(Vec<f32>)`.
Use `as_u16()` / `as_f32()` to borrow native samples; `pixel_center()` and
`validity()` provide spatial coordinates and validity. Float NaN is invalid;
other raw values are retained. This is an intentional prototype API migration
from a UInt16-only `Vec`; existing UInt16 examples/tests use the typed accessor.
No format-specific public request, parallel planner or downloader was added.
A callback consumes each owned chunk before the next one arrives. Retaining
chunks is the consumer's responsibility and outside the executor's bound.

For a measured example that prepares once and fetches twice:

```sh
cargo run --release --locked --example remote_zarr -- examples/weatherbench2.json warm
```

The example renames complete output directories to `-0` and `-1` to avoid
reusing an existing destination. Use a fresh sink path when repeating it.

## Exact conventional profile

- Zarr v2 group; consolidated or individual metadata; C order; no filters.
- Explicit requested top-level data arrays, little-endian Float32, NaN storage
  fill; `_FillValue`, if present, must agree. Source data dimensions must be
  `(time,latitude,longitude)` or `(time,longitude,latitude)` only.
- One-dimensional little-endian Float64 `latitude` and `longitude` coordinate
  arrays, with matching `_ARRAY_DIMENSIONS`. Exact semantic names establish
  geographic roles; `units`, `standard_name` and `axis`, when present, must agree
  with degrees_north/degrees_east, latitude/longitude, Y/X. No inference from
  arbitrary x/y names. Values must be finite, regular, strictly monotonic,
  have at least two centers, lie within the supported geographic ranges and
  have no duplicated/wrapped longitude endpoint. Either axis may descend.
- Names/ranges/spacing define a regular EPSG:4326 center grid. This profile does
  not accept projected or curvilinear coordinates, auxiliary CF object references,
  extra dimensions, cyclic windows or arbitrary calendars.
- Time is ascending unique little-endian Int64, `proleptic_gregorian`, with
  verified `hours since 1959-01-01`; existing exact Unix-seconds encoding is also
  accepted. Conversion to timestamps is checked for overflow/range. Native Int64
  values, units and calendar are preserved in output.
- None, gzip levels 0–9, or Blosc/LZ4 with shuffle 0/1, clevel 0–9, automatic
  blocksize. Other codecs/dtypes/endianness are rejected; no custom algorithm.
- The existing `tile_ripper_grid` UInt16 `(time,y,x)` profile remains supported.
  The marker is not required or injected for the conventional geographic profile.
- Native bounds, time interval/instants and requested variables; output remains
  one group per selected source asset. Geographic data dimensions normalize to
  `(time,latitude,longitude)`; source order/indices remain in the plan. Coordinate
  values, raw samples, units, fill and other attributes are retained. This
  documented permutation does not alter the scientific values.

## Download and decompression safety

HTTP transport is unchanged: bounded request bodies, strong per-object ETags,
conditional requests, validated Content-Length, explicit timeouts, serial reads,
at most two retries, and pre/post snapshot checks. No redirect, signing,
authentication, S3 API or background download framework was added. Per-object
checks do not establish an atomic multi-object dataset version.

Before opening source arrays, checked shape/chunk products, dtype/rank/codec
validation and coordinate admission limits bound decoded chunks. Encoded bodies
and decoded source chunks each must fit half `buffer_bytes`; each coordinate
axis and its native chunk must fit one sixteenth. Output payload caching uses
one quarter. Graph/object counts and metadata cache/overlay sizes remain bounded.
The example uses 16 MiB, primarily because the full 93,544-entry time axis is
748,352 decoded bytes. This budget admits payload components; it is not a process
RSS ceiling or a complete allocator/library scratch accounting system.

Source gzip objects are passed through the established flate2 `GzDecoder` into
an exactly sized vector, followed by one-byte excess/EOF and trailer/checksum
validation. It cannot grow to the stream's advertised expansion. Concatenated
members and trailing garbage are rejected. The resulting bytes are handed to
zarrs through an in-memory uncompressed array view; original source/output codec
metadata is retained. This replaces the unsafe source read_to_end path without
implementing a compression algorithm or changing the dependency's source.

Blosc headers must match the independently validated expected chunk bytes,
encoded length, element width, stream codec/filter and bounded blocksize before the established
zarrs/c-blosc decoder can allocate. That library validates and decodes the stream
with serial codec options. Blosc/LZ4 has no checksum, so arbitrary bit flips
cannot all be detected as corruption; strong versions are not content checksums.
Native library vulnerabilities and hard process isolation remain outside this
payload-allocation guarantee. Missing scientific chunks supply NaN/fill; missing
coordinate chunks are errors. Malformed data cannot silently become missing data.

Caught failures remove only a newly owned destination. A successful schema-3
completion manifest is written last. Interrupted/killed processes may leave
partial files without a completion marker; pre-existing outputs are never
silently overwritten or removed.

## Reproduce validation and measurements

Use the same frozen [Zarr oracle environment](zarr-workflow.md). Numcodecs/Zarr
Python supplies independent codecs; xarray supplies independent indexing and
metadata comparison.

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
prototype-output/zarr-env/bin/python bench/cf_zarr.py checks
prototype-output/zarr-env/bin/python bench/cf_zarr.py benchmark
prototype-output/zarr-env/bin/python bench/remote_zarr.py checks
```

The benchmark performs one reference fetch and one manifest-template fetch,
then three fresh-process runs per reader, each preparing once and fetching twice.
This is a few MiB of scientific data, not a full dataset download. All remote
objects use a fixed body cap; selection and repetition count are fixed. No
concurrent fan-out or paid service is used. Reference/template requests are
outside timed comparisons and disclosed. Additional trial runs during development
are not included in final statistics.

The baseline uses consolidated metadata directly, persistent HTTPS, conditional
versions, independent Numcodecs selection/decode, the same output chunking/codec
and per-object durable synchronization. A shared manifest template equalizes
output diagnostic serialization. Native outputs are scientifically identical;
empty root-attribute file omission and diagnostic/codec serialization may differ.
Both actual files/bytes are measured. xarray materializes the selected array;
Rust uses bounded native chunks.

Cold means preparation plus first fetch; warm means a second fetch from the same
validated metadata snapshot, with no scientific data-body cache. Provider, TLS,
OS and network caches are not purged. HTTP entity-body bytes exclude headers/
TCP/TLS. Source-read counters, transport attempts/statuses and metadata traffic
are distinct. Read/decode minus store retrieval includes library assembly, not
just isolated codec CPU. Stage timers overlap; do not sum them indiscriminately.
See the [decision gate](cf-zarr-decision.md) for measured results and limitations.

The source does not declare a precise geodetic datum. EPSG:4326 is the accepted
geographic profile interpretation, explicitly disclosed in plan limitations and
the manifest, rather than a source-authority assertion. Coordinates undergo no
datum transformation. Datum-sensitive applications must verify the provider
convention before combining this grid with other observations.
