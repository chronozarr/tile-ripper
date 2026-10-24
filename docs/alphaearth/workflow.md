# Bounded AlphaEarth GeoZarr prototype

The dedicated `alphaearth` profile retrieves existing learned Int8
representations through a standalone native stream. It is separate from the
rank-three scientific Zarr profile and does not change that profile's admission.
It reuses the existing exact-object transport, zarrs shard-index decoder and
bounded Zstd decoding. No new decoder, normalization, semantic labeling,
model inference, embedding database or whole-shard download is introduced.

```sh
cargo build --locked --release --no-default-features --example alphaearth_stream
target/release/examples/alphaearth_stream docs/alphaearth/request.json
```

The sample pixel is centered near longitude −79.00247036°, latitude 34.93682823°, in the 2017 observation.

The example emits NDJSON metadata, cropped native Int8 chunks and a final
completion report. Chunks are provisional until completion. The Rust API is
`alphaearth::prepare(Request)` followed by repeated
`Prepared::next_chunk(&AtomicBool)`. Errors permanently invalidate that
prepared stream. Source changes, invalid offsets/indices, cancellation and
insufficient budgets cannot produce a successful completion.

This is an index-window prototype. Its metadata includes the independently
checked affine, CRS and selected stored coordinate evidence. No generic
geographic request/reprojection interface or durable embedding writer is
claimed. The existing durable writer assumes spatial COG observations or
rank-three scientific arrays; extending its semantics to a channel axis is
deferred. Native signed bytes and original positional channel order are
available directly without forcing a Float32 conversion.

## Actual source, qualified 2026-10-24

The [tutorial](https://geoembeddings.org/tutorials/xarray_geospatial_embeddings_intro.html)
describes the store `s3://us-west-2.opendata.source.coop/tge-labs/aef-mosaic/`.
The bucket is accessed through this canonical path-style AWS HTTPS endpoint:

```text
https://s3.us-west-2.amazonaws.com/us-west-2.opendata.source.coop/tge-labs/aef-mosaic/
```

The bucket name itself is not a resolvable HTTPS hostname. The live metadata
differs materially from the tutorial's displayed example:

| Property | Observed live value |
| --- | --- |
| Format/dtype | Zarr v3, stored Int8, fill −128 |
| Array | `embeddings`, shape `(9,64,1859584,4009984)` |
| Original dimension names | `time, band, y, x` |
| Stored annual coordinates | 2017–2025, nine little-endian Int32 values |
| Channel identifiers | No channel coordinate labels supplied; preserve positions and the original name `band` |
| Logical chunk | `(1,64,256,256)`, 4,194,304 decoded bytes |
| Physical shard | `(1,64,4096,4096)`, 1,073,741,824 decoded bytes |
| Payload pipeline | bytes little-endian → Zstd, level3/checksum false |
| Index | End of shard, 4,100 bytes, little-endian UInt64 offsets/lengths plus CRC32C |
| CRS/registration | EPSG:4326, pixel registration, north-up affine |
| x/y | Each a single uncompressed little-endian Float64 chunk; 32,079,872 and 14,876,672 bytes respectively |

No `A00`–`A63` labels are inferred from the tutorial. Source root metadata
retains the learned-representation model and signed-square quantization
description; the prototype leaves the original signed stored values unchanged.

The root corner affine is
`[0.00008983111749910169,0,-180,0,-0.00008983111749910169,83.68570533713473]`.
Pixel centers use `origin + (index+0.5)*step`. Shape, bbox, axis direction,
registration and source coordinate formulas must agree. Preparation samples
eight-byte values at endpoints, interior positions, the requested window and
logical/physical boundaries. It never materializes either coordinate axis.

## Source-version and resource gate

The public endpoint exposes strong ETags, object sizes and S3 version IDs.
Separate correct-ETag range probes returned HTTP 206 with exact Content-Range;
deliberately wrong ETags returned HTTP 412 for time, both axes and an actual
700,572,783-byte shard. This satisfies the existing conditional-source
contract; no check was bypassed. Metadata, coordinates, time and selected
shards are individually pinned and finally revalidated. This is per-object
version consistency, not an atomic whole-dataset snapshot guarantee.

The sample request declares a 32 MiB network-body budget, 128 requests and
64 MiB working budget before source inspection. Metadata bodies are capped
at 1 MiB, selected encoded ranges at 8 MiB, and individual windows at 512×512
with at most nine logical chunks. Admission reserves bounded retries and a
conservative 32 MiB working allocation. Budget checks precede physical reads.
All ordinary storage users retain their existing metadata/range limits.

The selected one-pixel direct acquisition fetched a 4,100-byte shard index
and a 2,717,444-byte compressed logical chunk. It transferred 2,727,450 body
bytes across 55 native HTTP requests, including metadata/coordinate probes
and version checks. The final direct receipt reports actual wall time, CPU,
RSS and reservations. The 700.6 MB physical shard was never retrieved whole.
One channel or one pixel still requires the entire 64×256×256 logical chunk;
there is no per-channel compressed storage selectivity. The prototype uses
individual selected ranges, with zero coalescing overfetch or duplicate
payload ranges. Decoded selection overfetch is reported separately.

## Independent comparisons and faults

[Qualification receipt](qualification.json) includes the exact metadata,
canonical object identities, encoded range hashes, stored coordinate samples,
native reports and independent reference values/hashes. Zarr-Python 3.1.3,
obstore 0.8.2 and NumPy 2.3.4 run in a separate Python 3.12 environment.
Their ObjectStore reads original source bytes through a guarded proxy that
rejects whole shards and large coordinate GETs, enforces exact conditional
ranges, preserves original offsets and caches individual captured responses.
Native and Python decoders are independent. No Tile-Ripper decoded values are
used to generate the reference.

Complete Int8 equality passed for:

- One pixel, all 64 channels.
- A 3×4 window, all 64 channels.
- A 2×2 window crossing logical chunks.
- A 2×2 window crossing four physical shards.
- Positional channels `[63,0,17]`, preserving that order.

All five comparisons used 21,817,356 upstream body bytes and 81 upstream
requests under a shared 32 MiB/128-request cap. Local proxy requests are
distinct from upstream requests, and cached cases are not presented as
independent cold-provider timing. The [separate direct receipt](direct-validation.json)
executes the optimized native example against S3 itself and compares its 64
values to the saved independent reference. It reports process RSS/CPU and
source metrics without the proxy cache.

Offline tests cover missing shards/chunks with source fill, reordered channels,
logical and physical crossings, corrupt/truncated/overlapping/overflowing
indices, offsets into the index, malformed payloads, decoded-size bombs,
changed metadata/axes/shards, inconsistent affine/bbox/registration, cancellation
and insufficient budgets. Existing v2/v3, COG, HDF5/SWOT and authentication
regressions remain in the [shared acceptance matrix](../providers/acceptance.json).

Reproduce independent qualification after creating an isolated environment:

```sh
uv venv --python 3.12 prototype-output/alphaearth-env
uv pip install --python prototype-output/alphaearth-env/bin/python zarr==3.1.3 obstore==0.8.2 numpy==2.3.4 requests==2.32.5
prototype-output/alphaearth-env/bin/python bench/alphaearth_probe.py --binary target/release/examples/alphaearth_stream
```

Python dependencies are reference tooling only. The native prototype adds no
Cargo dependency and does not require GDAL or Python at acquisition time.
