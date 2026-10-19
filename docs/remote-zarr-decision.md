# Remote Zarr decision gate

The existing unified Rust library and `plan`/`fetch` CLI now acquire the supported
native Zarr v2 layout through anonymous HTTP(S). This is a controlled-fixture
success, not arbitrary public-Zarr support. No cloud resources were configured,
ChronoZarr changed, package published or branch merged.

The prerequisite output-buffering preservation gate was committed separately as
`0ea7af1`. Its [measurements](output-write-decision.md) remain intact.

## 1. Planner architecture

No public Request/Plan schema or chunk-selection mathematics changed. Private
prepared storage bindings supply local filesystem or reqwest HTTP objects to the
same Zarr adapter, planner and executor. Consolidated metadata interpretation
stays in the format adapter. COG retains GDAL transport and provider resolution.
Execution observations add transport, decoded/fill counts and first-output time.
One remote store per request keeps this milestone narrow. Metadata is cached per
prepared acquisition; data bodies are not cached. Independent prepared instances
cannot exchange snapshots. The callback remains synchronous and bounded by one
native buffer, without introducing a stream/plugin framework.

## 2. Minimum source chunks and scientific correctness

Each small extraction independently requires 42 logical keys. The planner selects
exactly those keys; execution makes 41 data GETs and 41 successful decodes, once
each, plus one native-fill chunk whose absence was pinned and revalidated by HEAD.
Only fault-injection retries repeat a GET. Planning GETs no pixel chunks.
Existing data transfers total 109,903 encoded bytes per extraction. Native chunk
materialization is 153,600 bytes for 59,808 delivered value bytes: unavoidable
spatial/temporal chunk amplification, not extra source keys.

The 72 benchmark acquisitions validate 2,153,088 values against independent
xarray/Zarr selection, including raw dtype, nodata and decoded masks, coordinates,
irregular dates, partial windows and attributes. A larger case validates 650,760
values and exactly 112 selected keys: 111 GETs/decodes and one fill.
[23 controlled checks](remote-zarr-checks.json) cover consolidated-only metadata,
uncompressed data, missing chunks, corrupt/oversized decoded data, empty space/time,
changed metadata/data/absence, snapshot isolation, cancellation, 429/503, interrupted
transfers, permanent failures, malformed metadata, oversized HEAD sizes, weak ETags,
redirects, hybrid v3 and credential redaction. Failures cannot leave a complete
manifest and caught failures remove the newly owned output directory.

All 29 ordinary Rust tests pass; the HTTP harness also drives the ignored Rust
snapshot/cancellation integration test in five scenarios. Existing local Zarr
independent validation and COG geometry, pixel, failure and snapshot checks pass.
The COG regression is a one-repetition functionality check, not evidence of a
performance change. See [regression evidence](remote-zarr-regression.json).
The [CLI and library demonstration](remote-zarr-demo.json) independently validates
three additional output groups.

## 3. Equivalent baseline and measured performance

Median seconds, three fresh-process repetitions per reader/regime/metadata mode.
Each process prepares once and fetches twice. Cold = preparation + first fetch;
warm = second fetch using version-checked metadata, still rereading every data
object. Neither is a purged OS-cache experiment. Near has no injected delay;
latency has 10 ms/request and one server slot; bandwidth has 2 ms/request,
256 KiB/s body throughput and two slots. Both clients issue serial requests.

| Network | Metadata | Tile-Ripper cold / warm | xarray cold / warm |
|---|---|---:|---:|
| near | individual | 0.287 / 0.270 | 0.307 / 0.290 |
| near | consolidated | 0.261 / 0.264 | 0.289 / 0.279 |
| latency | individual | 2.416 / 1.316 | 2.433 / 1.367 |
| latency | consolidated | 1.796 / 1.018 | 1.818 / 1.044 |
| bandwidth | individual | 1.275 / 0.990 | 1.306 / 1.044 |
| bandwidth | consolidated | 1.150 / 0.946 | 1.197 / 0.966 |

The tuned xarray baseline performs independent selection/read/decode and writes
the same native values, coordinates, chunk shapes, compression level, attributes
and durable completion semantics. It materializes the selected array in memory;
Tile-Ripper uses bounded buffers. A shared manifest template equalizes diagnostic
serialization work, not scientific selection. Output is scientifically equivalent,
not byte-identical across libraries. Rust writes 52 files and 220,143 bytes versus
xarray 52 / 187,446 in the first individual-metadata run; diagnostic manifests and
codec serialization differ. Those actual bytes are included in timing.

Both readers transfer 224,390 response-body bytes across preparation/two fetches
with individual metadata, or 225,817 consolidated. Rust makes 261/189 HTTP requests
versus xarray 255/183, respectively: the extra root v3 probe and its validation
HEADs are explicit overhead, not hidden data reads. Counts include HEAD and 404;
bytes exclude headers/TCP/TLS overhead. Transport counters distinguish metadata,
selected data, retries and statuses. First-output time and per-stage data are in
[raw repeated results](remote-zarr-benchmark.json); stage timers overlap and must
not be added indiscriminately. Full process wall includes startup and both fetches.

There is no demonstrated general or statistically established speed advantage.
These are small synthetic loopback workloads on one machine. Cold/warm medians
are close to the baseline in every regime; a real network may behave differently.

## 4. What bounds execution?

Near-server cold fetch source retrieval is about 0.010 s, read/decode 0.011 s and
native storage writes 0.236 s (individual-metadata medians). Output persistence is
the bottleneck there. With latency, preparation is 1.087 s and fetch source
retrieval 0.519 s; with the bandwidth limit retrieval is 0.599 s. Validation HEADs
also incur network delay. Read/decode exceeds retrieval by only about 1–2 ms;
decoding and buffer assembly do not dominate. Consolidation reduces metadata
round trips substantially. No concurrency framework was added to chase this test.

## 5. Memory and failure guarantees

Peak process RSS is at most about 42.0 MiB for small Rust runs versus 83.9 MiB
for xarray, including language/runtime costs. The larger 1 MiB configured-buffer
case reaches about 44.8 MiB RSS, 262,144 bytes of cached output payload and
879,821 source data bytes. These measurements demonstrate the tested buffers,
not an RSS ceiling or a general scalability proof. [Resource evidence](remote-zarr-resource.json).

Encoded bodies, metadata/overlay bytes, identities/graph nodes and destination
buffer/object counts are admitted against explicit limits. One object is read
and decoded at a time, with bounded retries/timeouts and cleanup. A configured
buffer budget does not include every allocator, library, metadata or codec cost.
The pinned zarrs gzip decoder allocates its decoded vector before validating its
expected length: a malformed gzip stream can exceed declared decoded-chunk bounds.
The size-mismatch test rejects and cleans up, but this is **not a hard memory
sandbox for hostile compressed input**. Fixing the mature dependency or isolating
untrusted decoding is separate work; no custom codec was introduced.

Strong per-object ETags/conditional reads detect changes, including previously
missing keys. This does not establish an atomic snapshot across multiple objects;
a provider-level immutable dataset/version remains the stronger guarantee.

## 6. Unsupported layouts/backends

Only the existing explicit `tile_ripper_grid`, north-up Float64 x/y, supported
Int64 time, UInt16 `(time,y,x)`, none/gzip v2 profile is supported. HTTP requires
strong ETags, HEAD/Content-Length and honest conditional behavior. Authentication,
signing, redirects, S3 API, v3/sharding, arbitrary CF layouts/dtypes/codecs,
reprojection and mosaicking remain unsupported. See the [exact contract and
copyable commands](remote-zarr-workflow.md).

A bounded real HTTPS probe reached public WeatherBench2 ERA5 metadata and failed
clearly on the missing custom grid marker. Only metadata and three coordinate
chunks were downloaded, no temperature data. Marker removal alone would not
suffice: axis order/orientation, Float32/NaN, Blosc/LZ4 and CF time units also differ.
The [public compatibility assessment](remote-zarr-public-compatibility.md) records
the exact objects, blockers and smallest complete interoperability profile.

## 7. Recommendation and stop

Prioritize one real-data interoperability profile before another backend or
format: explicit coordinate mapping/geographic interpretation, axis permutation,
CF time conversion, native Float32/NaN and existing Blosc/LZ4 support for a bounded
WeatherBench2 `2m_temperature` extraction. Do not advertise general CF support.
Of S3/object-store, v3 or SWOT/NISAR HDF5, object-store access is the closest next
backend when an actual dataset needs its version/authentication features. This
public HTTPS dataset does not. Defer v3 and the third scientific format.

R2-hosted fixtures could later demonstrate public HTTPS reproducibility, but would
not resolve scientific interoperability. No Cloudflare/domain configuration or
deployment was performed. The remote transport milestone stops here.
