# Anonymous HTTP Zarr v2 acquisition

Use the same `plan` and `fetch` commands as local Zarr and STAC/COG. This milestone
supports the existing, deliberately narrow georeferenced UInt16 layout over
anonymous HTTP(S), with per-object versions. It does not make arbitrary public
Zarr datasets readable; see the [public-data assessment](remote-zarr-public-compatibility.md).

## Copyable controlled demonstration

Build the CLI/examples with system GDAL available. Use the existing frozen
[Zarr Python environment](zarr-workflow.md) only to create/serve/check the fixture;
the acquisition implementation is Rust, reqwest and zarrs.

```sh
cargo build --release --locked --bins --examples
# Terminal 1: stays running until Ctrl-C; loopback only, no cloud deployment.
prototype-output/zarr-env/bin/python bench/remote_zarr.py serve --port 8765
```

In terminal 2, from the repository root:

```sh
./target/release/tile-ripper plan examples/remote-zarr.json --human
./target/release/tile-ripper plan examples/remote-zarr.json --json
./target/release/tile-ripper fetch examples/remote-zarr.json --output ./my-http-subset
```

Choose a new output directory. The fixture is synthetic, not a public scientific
product. It includes different native chunk grids, gzip compression, an absent
data chunk, nodata, radiometric attributes and irregular acquisition dates.
`plan` reads metadata/coordinates and HEADs selected data objects; it never GETs
pixel-data chunks. It is a no-pixel-download planning mode, not zero network I/O.

The source URL can be replaced with a matching anonymous HTTP(S) store. A plain
`python -m http.server` is insufficient because the supported contract requires
strong ETags and conditional requests. This harness supplies those semantics.
No Cloudflare/R2 configuration, deployment or tileripper.com change is needed
or was performed.

## Library lifecycle and output

```rust
let prepared = tile_ripper::acquisition::prepare(
    &request, &mut tile_ripper::cog::LayoutCache::default()
)?;
println!("{}", prepared.human());
let manifest = prepared.fetch()?;
```

`examples/acquire.rs` demonstrates the same prepared fallible callback for
consuming buffers immediately. `examples/remote_zarr.rs` demonstrates repeated
fetches from one prepared, version-checked metadata snapshot. It renames each
successful output directory to `-0`, `-1`, etc. so the unchanged prepared request
can be exercised again without overwriting anything:

```sh
cargo run --release --locked --example remote_zarr -- examples/remote-zarr.json warm
```

The API is synchronous; asynchronous callers should run it on a blocking thread.
There is no background prefetch or accumulation of the extraction. One source
object is retrieved/decoded at a time. Native Zarr output uses the existing
bounded destination buffer, preserving coordinates, selected dates, raw values,
nodata, scale/offset and source attributes. No temporal composite, mosaic or
reprojection is performed. See [native layout/window semantics](zarr-workflow.md).

Each asset produces a native `.zarr` group plus the common completion manifest.
The manifest contains actual timestamps, variables, CRS/affine, source URL,
per-object ETag/size identities, processing decisions and observed transport/
decode/output counters. Native output is scientifically complete only when the
schema-3 manifest says `status: complete`. A first readable output chunk does
not certify the entire extraction. Caught failures remove the newly owned output;
a killed process can leave partial files without a completion marker. Existing
directories are never overwritten or removed as part of cleanup.

## Supported contract

- One HTTP Zarr store per request, explicit named top-level variables.
- Zarr v2 group, C-order `(time,y,x)` UInt16 arrays; none or gzip compression;
  no filters; explicit integer fill; compatible coordinate dimensions.
- Float64 x/y centers matching explicit north-up `tile_ripper_grid` CRS/affine;
  ascending unique Int64 time in the documented Unix-seconds/calendar encoding.
- Native bounds and existing inclusive interval/instant selection. Array chunk
  shapes may differ between variables; dot and slash chunk separators work.
- Consolidated `.zmetadata` or individual v2 metadata. Consolidated-only stores
  work even when individual `.zarray`, `.zattrs`, `.zgroup` objects are absent.
  The aggregate must fit the configured budget. Malformed or inaccessible
  consolidated metadata is rejected rather than silently bypassed.
- Public HTTP(S) prefixes without userinfo, query strings or fragments. Both a
  trailing slash and an unadorned store prefix are accepted. Object addresses
  are resolved with the existing URL library and cannot escape that prefix.
- Strong ETags, HEAD with Content-Length and honest conditional HTTP semantics.
  Redirects, weak/missing ETags, signing, authentication, `s3://`, v3/hybrid stores,
  sharding, additional codecs/dtypes and automatic CF interpretation are rejected.

STAC/COG retains its existing provider-specific resolution and GDAL transport;
these HTTP Zarr restrictions do not remove its established URL handling.

## Versions, retries and limits

Preparation caches bounded metadata/coordinate bytes and pins metadata versions.
Each mathematically selected data key is HEADed once to capture size/ETag or
absence; no data body is fetched. The prepared snapshot is then sealed.

Execution revalidates metadata and known absences before/after reading. Existing
data objects are GET with `If-Match`; the returned ETag and complete body size
must agree. Known-absent data keys supply native fill after absence validation,
without a redundant GET. Unexpected disappearance/appearance, changed ETags,
truncated bodies or corrupt data cannot silently become nodata.

These are per-object version guarantees, not an atomic transaction across an
independently mutable multi-object dataset. There is no global/shared URL cache.
A newly prepared acquisition gets fresh identities; an older prepared snapshot
fails if its relevant versions no longer exist. Warm metadata means reuse of
that prepared object, with HEAD validation. Separate CLI processes prepare anew;
`--cache` does not persist HTTP metadata or source chunks.

The reused reqwest client has one request in flight per source, 30 s request/
10 s connect timeouts and at most two retries. Connection failures, interrupted
bodies and HTTP 408/429/500/502/503/504 are retryable. Default backoff is 25/50 ms.
`Retry-After` seconds or HTTP dates are honored within a two-second wait window;
longer waits fail clearly so the caller can retry later. Version conflicts,
unsupported metadata/statuses and decode corruption are not retried.
These fixed initial settings are documented, not new request-schema options.

`buffer_bytes` bounds encoded object bodies and each metadata cache/view to half
its value, declared decoded source chunks to half, callback buffers and decoded
output payload to a quarter each. `max_nodes` bounds DAG/identity/cache entries
and destination-object bookkeeping. Coordinate admission limits remain intact.
These are working-payload/admission bounds, not a hard process RSS limit: graph,
indexes, serde/native libraries, allocator and codec scratch also use memory.

A concrete dependency limitation remains: zarrs 0.22.10's gzip codec calls
`read_to_end` before validating decoded length. A malformed stream can allocate
beyond the declared chunk allowance before it is rejected. The controlled 1 MiB
size-mismatch check fails and cleans output, but does not demonstrate a hard
malicious-input memory cap. We did not add a custom codec, silently change gzip
support or claim a process sandbox. Upstream bounded-decode support is a
prerequisite for a stronger guarantee on untrusted compressed input.

## Reproduce checks and measurements

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
prototype-output/zarr-env/bin/python bench/remote_zarr.py checks
prototype-output/zarr-env/bin/python bench/remote_zarr.py benchmark --reps 3
prototype-output/zarr-env/bin/python bench/remote_zarr.py resource
```

The HTTP checks run the ignored Rust snapshot/cancellation test in five controlled
scenarios. The ordinary Cargo suite skips it when no server is configured.
The separate existing STAC harness exercises its ignored GDAL snapshot test.

The benchmark uses an independent Zarr-Python decoder and xarray indexing with a
small standard-library HTTP mapping. Both readers use persistent serial HTTP,
conditional versions, HEAD validation and no data cache. The baseline knows the
same requested variable names and enumerates their metadata keys, avoiding any
nonportable HTTP listing API. Output chunks, gzip level, fill behavior and
per-object file synchronization match. A common diagnostic manifest template
matches serialization work; baseline observations/versions replace Rust's
observations. Python output uses its native gzip implementation and key layout;
encoded sizes are measured, not assumed byte-identical.

Three shuffled repetitions cover near-local, 10 ms latency/one-connection and
2 ms latency/256 KiB/s body-bandwidth regimes, both metadata forms. Server
connection limits are 4/1/2 respectively; the current client remains serial.
Each fresh process performs a cold preparation+fetch and one warm prepared fetch.
The OS file cache is not purged. Cold/warm describe client metadata state, not a
machine reboot. Full process wall/RSS includes both acquisitions and startup;
per-acquisition times inside each process exclude startup. No significance or
language-wide speed claim follows from these small workstation samples.

Independent server logs record method, relative key, status, conditional headers
and body bytes sent, separately from client counters. These are HTTP entity-body
bytes, not total TCP/TLS/header traffic. HTTP retries include partial bodies in
client byte accounting; source-read traces count successful library gets and
encoded bytes delivered to the decoder, which is a different quantity.
Metadata traffic is separated by key from selected scientific data keys.

Raw evidence includes preparation transport timings (metadata versus selected
object HEADs), remaining planning overhead, fused source read/decode, isolated
store-read timers, output setup/sink/finalization, decoded/missing counts, first
readable output-object time, files/bytes and peak RSS. Nested timers are not
added together. Small differences between fused decoding and store time include
library/materialization overhead, not just gzip CPU. The baseline retains its
full selected array in memory; it is an equivalent output/performance comparison,
not an identical streaming-memory algorithm.
