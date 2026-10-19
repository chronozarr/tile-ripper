# First real-world Zarr interoperability decision gate

The actual public WeatherBench2 ERA5 **64×32, six-hourly 1959–2023** Zarr v2
store now works through the existing Rust `plan`/`fetch` lifecycle. The tested
variable is `2m_temperature`. No custom source metadata, account, deployment,
cloud configuration, third format or second planner is required.

## 1. Actual dataset and selection

The [official data guide](https://weatherbench2.readthedocs.io/en/latest/data-guide.html)
lists the low-resolution product. [Observed metadata](cf-zarr-public-metadata.json)
was fetched before choosing support: public anonymous HTTPS, consolidated v2
metadata, `(time,longitude,latitude)` `<f4`, NaN fill, no filters, Blosc/LZ4 shuffle,
shape `[93544,64,32]`, chunks `[100,64,32]`. Coordinates are regular Float64;
Int64 time uses hours since 1959-01-01, proleptic Gregorian. Source geographic
coordinate attributes provide semantic dimension names, not explicit unit/CRS
markers. Exact latitude/longitude names plus validated ranges/spacing define the
narrow geographic interpretation; contradictory optional attributes fail.

The [copyable request](../examples/weatherbench2.json) selects bounds
`[10,30,25,50]` and 1959-01-01 00:00–06:00 UTC. Native-cell intersection returns
three longitude centers × four latitude centers × two times = **24 Float32
values in kelvin**. It selects and retrieves exactly one native scientific chunk,
580,139 encoded / 819,200 decoded bytes. Delivered values occupy 96 bytes. The
large unavoidable transfer ratio reflects a source chunk covering the full globe
and 100 times; there are no unnecessary source data keys or repeated data decodes.

## 2. Conventional metadata and shared architecture

Supported dimensions are time first, followed by latitude/longitude in either
order. Geographic Float64 coordinate axes may ascend or descend. `_ARRAY_DIMENSIONS`,
Float32/NaN, regular geographic centers, compatible optional units/standard_name/
axis, exact verified time units/calendar, none/gzip/Blosc-LZ4 and native attributes
are understood. Extra dimensions, conflicting conventions, curvilinear axes,
unknown calendars/codecs and big-endian or other data types fail explicitly.
See the [exact contract](cf-zarr-workflow.md).

`tile_ripper_grid` is **eliminated for this conventional profile**, retained for
the established UInt16 custom profile. Output does not inject the marker. Original
root attributes, coordinates, raw time encoding, fill and variable units/attributes
are preserved. Output data dimensions normalize to `(time,latitude,longitude)`;
source dimension order and indices remain visible in the plan. This explicit
permutation preserves numerical values and spatial alignment.

No public Request/Plan schema or source-selection mathematics changed. The same
Zarr adapter maps native dimension roles to chunk keys; the shared native window
helper now handles either affine axis direction. A general `Pixels::UInt16` /
`Pixels::Float32` public sample enum replaces the previous UInt16-only vector,
with typed accessors and validity. This is the necessary public prototype API
migration, not a WeatherBench2-specific callback. No format-specific request,
parallel planner, custom downloader, reprojection or mosaicking was added.

## 3. Numerical correctness

Independent xarray indexing and Numcodecs decoding produce **exactly identical
Float32 values** (maximum absolute difference 0), raw coordinates, time selections,
dtype, fill and attributes after the documented axis permutation. Decoded time
coordinates match both requested UTC instants. Twelve benchmark extractions each
validate all 24 values. Real selected values contain no NaN; NaN/missing-chunk
behavior is additionally verified on controlled independent fixtures.

Twenty-four fixtures cover both directions of both axes, both axis orders and
none/gzip/Blosc-LZ4. Their mathematical key sets, planner sets and observed reads
agree, with each existing source chunk decoded once. Seventeen additional cases
cover invalid dimensions/type/units/calendar, irregular coordinates, time overflow,
malformed size products, excessive chunks, truncated/oversized Blosc chunks,
falsely declared stream codecs,
missing data/coordinate chunks, empty space/time and a transient 503. Callback
and native output tests verify Float32 samples, coordinates and validity.

All **31 ordinary Rust tests**, all **25 controlled HTTP checks**, the existing
local xarray edge/determinism oracle, COG pixel/geometry/failure checks and both
server-driven snapshot test suites pass. Clippy/all-target warnings and formatting
checks pass. Native cold/warm output objects are deterministic per reader across
all repetitions; manifests intentionally contain varying observations/timings.
[Regression evidence](cf-zarr-regression.json), [profile checks](cf-zarr-checks.json),
[HTTP safety checks](cf-zarr-http-checks.json), [local oracle](cf-zarr-local-validation.json).

## 4. Download/decompression safety

The documented gzip growable-allocation gap is closed for source arrays. Shape,
rank, dtype and chunk products are validated before opening; arithmetic overflow
and over-budget chunks fail before allocation. Existing HTTP body limits remain.
The established flate2 decoder writes into exactly the admitted decoded size,
then checks one excess byte, EOF, trailer/CRC and trailing/concatenated data.
Only an in-memory codec view is uncompressed; source/output metadata is retained.
This is bounded use of an existing decoder, not a compression algorithm.

The 16 MiB expansion bomb is rejected after expected chunk bytes plus one probe;
truncated, corrupt-checksum, wrong decoded-size and concatenated gzip tests fail.
HTTP bomb rejection peaks at about 39.4 MiB process RSS on this machine and leaves
no completed output. Blosc headers must match validated decoded bytes, element
size, actual encoded length and bounded blocksize before zarrs/c-blosc decoding.
Oversized/malformed streams cannot dictate an unbounded destination allocation.
Missing coordinate chunks fail; malformed scientific chunks cannot become fill.

Metadata cache/overlay, encoded bodies, coordinates, native chunks, output buffers
and object counts have explicit admission bounds. `buffer_bytes` remains a
component payload budget, **not a hard process RSS ceiling**: vectors/copies,
allocator/runtime and bounded codec scratch add overhead. C/native dependency
vulnerabilities and process isolation are not solved. Blosc/LZ4 has no checksum;
not every arbitrary bit flip can be detected. Strong ETags preserve per-object
identity, not an atomic whole-dataset snapshot. Existing retry/timeouts, URL
redaction, cancellation and cleanup remain in force. See [implementation and
reproduction details](cf-zarr-workflow.md).

## 5. Equivalent remote performance

Three fresh-process repetitions per reader, each preparing once and fetching
twice from the actual public HTTPS source. Medians in seconds:

| Reader | Cold prepare + fetch | Warm prepared fetch | Full process, two fetches | Peak RSS |
|---|---:|---:|---:|---:|
| Tile-Ripper | 1.475 | 0.640 | 2.595 | 48.7 MiB |
| tuned xarray/Zarr | 0.780 | 0.378 | 1.438 | 88.7 MiB |

**Tile-Ripper is slower here. No speedup is claimed.** xarray uses consolidated
metadata directly, persistent HTTPS, conditional source versions, identical native
output values, codecs/chunks and durable synchronization. Both reread the selected
scientific object once per extraction; metadata caching is warm, data is not.
The baseline materializes its selected array while Rust uses native buffers.
A shared diagnostic manifest template equalizes serialization; the independent
scientific selection/decode is not reused from Tile-Ripper.

Per prepare/two-fetch process, both transfer **1,231,715 response-body bytes**,
including 71,437 metadata/coordinate bytes and two 580,139-byte data bodies.
Tile-Ripper makes **75 HTTP requests**, versus **51** for xarray. The difference
is four more absent format-probe identities and their five revalidations across
the lifecycle: Rust probes root/four array v3 markers; baseline probes root
`.zarray`. Five validation passes are preparation plus before/after each fetch.
This is measured safety/format-validation overhead, not extra pixel reads.

Source retrieval, fused read/decode, preparation, first-output and output timings,
requests/statuses, bytes, file counts, writes and memory are recorded in
[raw results](cf-zarr-benchmark.json). Read/decode exceeds retrieval by roughly a
millisecond in representative runs. Durable native output is tens of milliseconds;
metadata/version round trips dominate this small extraction. Nested stage timers
are not additive; subtraction is not a pure isolated codec CPU measurement.

Rust writes 15 files including the manifest; xarray writes 16 because it emits an
empty root `.zattrs` while zarrs omits it. Both represent identical root attributes.
Actual output byte counts include different diagnostics/serialization and are
measured, not assumed equal. The comparison is semantically equivalent, not
byte-identical between libraries.

Provider/TLS/OS/network caches were not purged. Three repetitions on one workstation
support neither statistical nor general performance superiority. One reference
fetch and one template fetch are outside timing; all scientific downloads remained
small/bounded. This COG regression run establishes functional parity, not a
statistically measured COG speed change.

## 6. Limitations and next milestone

The profile is deliberately narrow: exact named regular Float64 geographic axes,
time-first 3D Float32/NaN, two exact time encodings and three admitted compressor
choices. No generic CF discovery, arbitrary dimensions/calendars, longitude
wrapping, curvilinear grids, projected conventional axes, additional dtypes,
v3/sharding, authentication or S3 API. The full time axis is inspected; an oversized
axis requires a larger explicit budget. Source chunk organization can make very
small subsets transfer much more than their delivered values.

Recommend **one additional demand-driven Zarr profile before a third format**,
preferably a concrete dataset that tests generalized coordinate roles or projected
CF grid mapping, rather than adding speculative permutations. First consider
reducing avoidable version/format round trips while retaining correctness, now
that their public-network cost is measured. Do not start HDF5/NISAR/SWOT, v3 or
a broad ecosystem study on the strength of this one success.

The milestone stops here. No tileripper.com, Cloudflare/R2 deployment, paid
infrastructure, ChronoZarr modification, merge or publication occurred. A read-only
Sonnet review was attempted but its runner had an expired OAuth token; no
independent agent review is represented as completed.

The source does not declare a precise geodetic datum. EPSG:4326 is the accepted
geographic profile interpretation, explicitly disclosed in plan limitations and
the manifest, rather than a source-authority assertion. Coordinates undergo no
datum transformation. Datum-sensitive applications must verify the provider
convention before combining this grid with other observations.
