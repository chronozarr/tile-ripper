# Independent Icechunk source comparison

This bounded comparison uses maintained Python Icechunk 2.3.0, Zarr 3.4.1 and
Gribberish 1.8.4 readers. It establishes the selected source values and the
response-body cost of those reads. It is not a speed measurement of Tile-Ripper
or Chronozarr, and it does not establish archive-wide equivalence.

## Exact source specimens

| Property | Materialized GFS analysis | Virtual GFS analysis |
| --- | --- | --- |
| STAC collection | [`noaa-gfs-analysis`](https://stac.dynamical.org/noaa-gfs-analysis/collection.json) | [`noaa-gfs-analysis-virtual`](https://stac.dynamical.org/noaa-gfs-analysis-virtual/collection.json) |
| Repository | `https://dynamical-noaa-gfs.s3.us-west-2.amazonaws.com/noaa-gfs-analysis/v0.1.0.icechunk` | `https://dynamical-noaa-gfs.s3.us-west-2.amazonaws.com/noaa-gfs-analysis-virtual/v0.1.0.icechunk` |
| Snapshot | `6K6H23PYR6QXA9VM0C6G` | `SJSV5KG1GSWR2XQD2SW0` |
| Variable | `temperature_2m` | `temperature_2m` |
| Native type | Float32 | Float64 |
| Dimensions | time, latitude, longitude | time, latitude, longitude |
| Logical chunk | 1440 × 50 × 50 | 1 × 721 × 1440 |
| Physical shard | 1440 × 400 × 400 | One complete GRIB message per chunk |
| Codec | Indexed sharding; bytes + Blosc/Zstd3/shuffle; end index + CRC32C | Scale/offset with −273.15 offset, then Gribberish with longitude adjustment and north-up ordering |
| Units | degree_Celsius | degree_Celsius |

Latitude descends from 90 to −90 by 0.25°; longitude ascends from −180 to 179.75°
by 0.25°. Time uses Int64 seconds since 1970-01-01, starting on
2021-05-01T00:00Z, with a proleptic Gregorian calendar. The declared CRS is the
WMO sphere of radius 6,371,229 m, not the WGS84 ellipsoid. The full source WKT,
array attributes, codec configurations, object ETags and response hashes are in
[the machine-readable receipt](reference-receipt.json).

The materialized time length is 48,135; the virtual length is 48,138 at these
snapshots. Different latest times must not be treated as equivalent coverage.

## Scientific products differ

At time index 48,072 (2026-10-25T00:00Z), rows 224:226 and columns 316:318 are
latitude [34, 33.75] and longitude [−101, −100.75]. The selected values are:

| Representation | Four temperatures, °C |
| --- | --- |
| Materialized Float32 | 20.125, 21.0, 20.25, 21.375 |
| Virtual Float64 | 20.150996093750052, 21.04099609375004, 20.30099609375003, 21.420996093750034 |

The maximum absolute difference is 0.0509961°C. Over the selected 24 hours it is
0.0620898°C; over the selected 101 × 101 map it is 0.0609961°C. NaN locations
agree in these nonmissing samples. Casting virtual values to Float32 does not
make them equal to the materialized values.

This is expected to be relevant when choosing a source: Dynamical documents
[binary floating-point rounding](https://dynamical.org/catalog/noaa-gfs-analysis/#compression)
for its materialized product and links its
[reformatting configuration](https://github.com/dynamical-org/reformatters/blob/main/src/reformatters/noaa/gfs/analysis/template_config.py).
The measurements here demonstrate a difference; they do not independently prove
that every difference in every variable comes solely from that rounding.

**These are explicit source choices. Performance alone must not silently switch
between these products.** Tile-Ripper must preserve whichever representation
was selected. Chronozarr must either preserve its admitted type or disclose and
require an explicit conversion policy; reading Float64 as Float32 is not exact
native-value preservation.

## Read granularity and measurements

The three workloads use the same time start, 2026-10-25T00:00Z:

- `regional_one_time`: one time, rows 224:226, columns 316:318.
- `regional_24_hours`: 24 consecutive times, the same 2 × 2 area.
- `regional_map_101`: one time, rows 200:301, columns 300:401.

A fresh repository/session is opened once per product. The workload sequence
then retains that session and reads each selection twice. Therefore the second
and later selections are not independently cold runs. No extra application
payload or decoded-value cache is installed. Provider/CDN cache state is unknown.

The [receipt](reference-receipt.json) records every forwarded request, its Range,
response-body length, status, ETag, content range, SHA256 and wall time. Bytes
include metadata needed during each selection and any repeated reads. They
exclude HTTP headers, TCP/TLS overhead and DNS. Initial metadata/coordinates are
recorded separately. A loopback forwarding server adds latency, so absolute wall
times are observations about this harness, not a production performance promise.

The first complete run showed the expected reversal: a single virtual map read
used less data than the time-optimized materialized read, while a 24-hour tiny
area used much less data from materialized storage. Exact totals are retained in
the receipt rather than converted into a general speed ranking.

Virtual GRIB access reads the complete model grid even for the 2 × 2 request.
At the selected first time, that message is 872,896 encoded bytes and
8,305,920 decoded Float64 bytes. Small rendering tiles or a small requested AOI
do not change the encoded GRIB read granularity.

## Native CLI checks

The final debug executable passed 12 `plan` / `stream` / `fetch` trials: two
trials of each workload for each product. Reopened output matched the independent
reference's native values bit for bit, as well as its coordinates, units and CRS.
The [native receipt](native-receipt.json) records the binary hash, counters and
individual wall times. The [acceptance record](implementation-acceptance.json)
binds the implementation and validation checks.

| Selection | Materialized data bytes | Virtual data bytes | Materialized stream, s | Virtual stream, s |
| --- | ---: | ---: | ---: | ---: |
| One time, 2 × 2 | 1,364,783 | 872,896 | 4.51 | 3.86 |
| 24 hours, 2 × 2 | 1,364,783 | 18,408,814 | 4.76 | 14.43 |
| One time, 101 × 101 | 9,401,948 | 872,896 | 6.60 | 3.87 |

Times are medians of two fresh CLI processes, including preparation and final
validation. These are debug-build observations with uncontrolled provider/OS
caches, not release-build throughput claims. Data bytes are the native execution
source-data counter: coordinate/shard-index reads and SDK-owned Icechunk metadata
traffic are outside that counter. The products have different precision; these
measurements do not authorize automatic substitution.

The materialized path decoded one source chunk for the tiny 24-hour series;
the virtual path decoded 24 whole-grid GRIB messages. The map selection decoded
nine materialized chunks or one virtual message. This is the access-pattern
tradeoff the explicit source choices expose.

## Legacy plain-Zarr comparison

A separate, narrower comparison at time index 15,000 (2023-01-16T00:00Z), the
same 2 × 2 area, found equal Float32 values and an identical compressed logical
payload between current materialized Icechunk and the previously supported
plain-Zarr endpoint. See [the receipt](legacy-comparison.json).

- Both selected compressed payloads are 3,985,993 bytes, SHA256
  `24f4c376a54954227bd5644224f31c7eb7db2fbb0928e36c3e0dba5a646f8675`.
- The selected range is bytes 122,548,591–126,534,583 in each backing shard.
- The shard index hashes also agree. Physical URLs and ETags differ and are
  recorded individually.
- Icechunk reference resolution adds manifest reads; the physical data payload
  itself does not become smaller because it is addressed through Icechunk.

This supports a matched comparison for that specific logical payload. It does
not register the endpoints as interchangeable providers, establish archive-wide
identity, or prove which route is faster on another network or request.

## External-object version limitation

The independently parsed virtual reference for the selected first time supplies
no original ETag or last-modified checksum. See
[the parsed reference receipt](virtual-reference.json). Icechunk's snapshot pins
the mapping, not an immutable copy of the external GRIB file. A runtime ETag pin
can detect changes during acquisition but cannot establish that the external
bytes still equal those at snapshot creation when the reference supplies no
original identity checksum.

The metadata-only introspection helper uses icechunk-js 0.6.0, independently of
the Rust acquisition implementation. It intercepts the library's already parsed
payload before the data fetch; that instrumentation is not a production API.

## Reproduce

Use a separate Python environment. The meter enforces a 128 MiB total
response-body cap per product, admitting only the two exact public repository
prefixes, the legacy endpoint and the NOAA GFS bucket. Authentication is
explicitly anonymous; the HTTP forwarder disables ambient credentials and
proxy discovery. Only the selected variables and bounded windows are read.

```sh
uv venv --python 3.13 /tmp/tile-ripper-icechunk-reference
uv pip install --python /tmp/tile-ripper-icechunk-reference/bin/python \
  icechunk==2.3.0 zarr==3.4.1 gribberish==1.8.4 \
  numpy==2.5.4 requests==2.34.2 'fsspec[http]==2026.9.0'

/tmp/tile-ripper-icechunk-reference/bin/python bench/icechunk_qualification.py \
  --output docs/icechunk/reference-receipt.json \
  --repeats 2 --time-series --map-window --max-body-mib 128

/tmp/tile-ripper-icechunk-reference/bin/python bench/icechunk_legacy_comparison.py \
  --output docs/icechunk/legacy-comparison.json

npm --prefix /tmp/tile-ripper-icechunk-js-reference install \
  --no-audit --no-fund icechunk-js@0.6.0
node bench/icechunk_reference_payload.mjs \
  /tmp/tile-ripper-icechunk-js-reference/node_modules/icechunk-js/dist/index.js \
  docs/icechunk/virtual-reference.json
```

The Python script defaults to the recorded immutable snapshots. `--latest`
explicitly resolves current `main` instead and writes the new snapshot IDs;
results from different snapshots are separate evidence. No publishing, source
mutation, credential discovery or archive scan is performed.

## Chronozarr import cache experiment

The converter stages selected timesteps separately, but the materialized source
stores 1,440 timesteps in one logical chunk. Without a source payload cache,
reading adjacent selected timesteps downloads the same compressed subchunk
again. A controlled experiment through Chronozarr's actual `XarraySource.read`
compared maintained Icechunk cache budgets of zero, 32 MiB and 64 MiB, with the same
snapshot and a read order of time 0, time 1, time 0 in the two-time selection.

Both nonzero caches retained the selected encoded subrange and its index: the first
read fetched 1,372,181 response-body bytes and the next two needed no HTTP
operation. With the cache disabled, the second and third reads each fetched
1,365,811 bytes again. Values were exactly unchanged; no whole shard was read.
This supports enabling the maintained reader's bounded encoded-byte cache,
without introducing a custom chunk format or claiming a renderer speedup.

The complete request events and values are in
[the import-cache receipt](import-cache-receipt.json). Reproduce using a
Chronozarr checkout environment containing its `geo` and `icechunk` extras:

```sh
../chronozarr/.venv/bin/python bench/icechunk_import_cache.py \
  --chronozarr-source ../chronozarr/src \
  --output docs/icechunk/import-cache-receipt.json
```

This helper injects a metered repository opener only for the experiment. The
production reader still receives the original HTTPS repository URL. The cache
budget covers encoded chunk bytes, not decoded chunk scratch, metadata or total
process RSS; the selection and source-chunk admission limits remain separate.
