# Zarr v3 acquisition decision gate — 20 October 2026

The scoped v3 input path works through the existing planner, scheduler,
`ChunkStream` and successful-`finish()` contract. Ordinary and sharded arrays
match independent scientific readers, including a real public GFS subset.
Sharded acquisition downloads one index per selected physical shard and only
selected compressed inner chunks. The next engineering step should improve
bounded range coalescing and expand tested Zarr profiles. Dense shaped-network
reads show a clear performance gap against current Zarr-Python coalescing.

## 1. Dependency upgrade and existing regressions

The separate dependency commit is **7d14342**, following f885e50. It pins zarrs
0.23.14, migrates the moved/generic APIs and declares Rust 1.91. The
[upgrade receipt](../zarr-v3-upgrade/report.md) includes the dependency/license/
publication-date audit, the deterministic 98-case reproduction, all inherited
gates and the 144-cell matched writer comparison. The original gzip failure is
upstream's underestimated encoded-size bound inside sharding; the
[released fix](https://github.com/zarrs/zarrs/releases/tag/zarrs-v0.23.14)
resolves all 12 formerly failing medium/large application cases. No encoder was
replaced.

The upgraded optimized default writer retains large-window medians of 0.3559 s
local and 3.4208 s shaped, compared with 0.4302 / 3.4583 s for the actual preserved
optimized 0.22.10 binary. These matched preservation results keep the preceding
v2 improvement; they do not substitute a new comparison for the original
9.2× warm-local / 2.3× shaped result against the older writer.

After input integration, [Rust tests and strict Clippy](regressions/rust.json)
pass for every configuration:

| Configuration | Passed | Existing ignored |
| --- | ---: | ---: |
| Default GDAL | 71 | 2 |
| GDAL-free native COG + Zarr | 49 | 3 |
| Zarr-only | 34 | 1 |
| All features | 89 | 4 |

The [inherited native COG/consumer gate](regressions/regressions.json),
[remote v2](regressions/remote-zarr-checks.json),
[CF v2](regressions/cf-zarr-checks.json),
[scientific output](regressions/candidates.json) and
[31 default publication faults](regressions/faults.json) pass again. Native COG
faults were exercised with a native-feature consumer, keeping GDAL's different
admission behavior distinct. No native COG or v2 scientific semantics changed.

## 2. Experimental output

The separate output commit is **fda7c43**. Its
[report](../zarr-v3-output/report.md) records 90 real-source matrix cells,
155 publication-fault cases, independent Zarr-Python/xarray/GDAL validation and
64-fold volume memory checks. Tested profiles are ordinary raw and sharded raw,
gzip, Blosc/Zstd and Zstd, with checked CRC32C shard indices. All pass. This is
confidence in those profiles, not a general v3 compatibility guarantee.
Experimental output remains feature gated with an explicit encoding; normal
output stays uncompressed v2.

## 3. Supported input and integration boundary

[The exact profile and example](workflow.md) cover time-first UInt16/Float32,
ordinary or regular sharded chunks, explicit Float64 spatial axes, Int64 CF time,
regular monotonic coordinates in either direction, native bounds, noncontiguous
instants and common lossless little-endian bytes/gzip/Zstd/Blosc pipelines.
Float32 NaNs and CF missing markers determine validity. V3 storage fill alone
remains a valid value. Explicit UInt16 mask arrays can be selected; applying
external ancillary masks is outside this profile and is rejected.

The implementation reuses generic object transport, the logical plan, bounded
workers and native chunk assembly. zarrs owns metadata interpretation, index
decoding, encoded-subchunk extraction and inner bytes/Blosc decoding. Established
flate2/Zstd decoding uses fixed output bounds before library bytes interpretation.
No format specification, encoder, sharding decoder, input transformation or
NetCDF/HDF5/downstream integration was introduced.

There is one necessary defensive integration guard: zarrs 0.23.14's encoded
subchunk helper computes `offset + length` without testing the standard
`MAX/MAX` missing-inner sentinel. The adapter uses the library-decoded index to
handle missing entries before calling that helper. It also rejects mixed
sentinels, overflow, invalid lengths, overlaps, index intersections and ranges
outside the pinned object. Missing-inner and absent-shard science tests pass.
This does not require a library fork. Future upstream changes to that API should
be rechecked before removing the guard.

## 4. Selective physical traffic

The prepared context distinguishes inner logical chunks from outer physical
shards. It downloads one fixed index range per present selected shard, validates
it with the library and retains it within the shared metadata admission budget.
Each selected present inner chunk is read once. Unsupported outer codecs,
transformers or index profiles fail explicitly; full-shard fallback is disabled.

[Two executions of one prepared acquisition](prepared-reuse.json) download
**12 indices total for 12 physical shards**, rather than retrieving them again
for each worker or execution. Payloads are retrieved again for the second call.
All successful controlled profiles and benchmark cells show zero duplicate data
ranges within an acquisition and zero full-shard GETs.

The [public archive](https://dynamical.org/catalog/models/noaa-gfs/), accessed at
its direct anonymous S3 URL, has strong ETags and suitable standard v3 metadata.
The [public receipt](public.json) preserves URL, root identity/hash, per-object
versions, exact request ranges and the tested timestamps. Two times × four
pixels match independent Zarr-Python 3.4.1 bit for bit, with coordinates and masks.

For its selected **141,648,404-byte** physical temperature shard, Tile-Ripper
reads a **1,028-byte index and one 3,985,993-byte compressed payload**. That is
2.815% of the shard, with no whole-shard body download. The inner chunk is
`[1440,50,50]`: decoding its 14.4 MB is required even for this tiny subset. The
native sphere WKT is preserved, rather than relabeled WGS84. This is a bounded
public correctness/traffic example, not a provider speed claim.

## 5. Scientific and failure validation

[Forty codec/axis profiles](correctness.json) cover ordinary/sharded, raw, gzip,
Zstd, Blosc/LZ4 and Blosc/Zstd, both spatial dimension orders and orientations,
start/end indices and inline consolidated metadata. Each validates raw bits,
validity, original coordinates, exact nanosecond timestamps and unique coverage.
[Three extra shape/key/fill profiles](edges.json) exercise asymmetric chunks,
multiple shard factors, both default/v2 key encodings, edge padding, absent
shards, retained all-missing indices and distinct finite CF/storage fills.
Independent Zarr/xarray reopens verify native v2 materialization too.
[Additional codec/admission cells](extras.json) verify Blosc bitshuffle/no-shuffle,
bytes-only shard indices, and preparation-time rejection of outer compression
and non-time-first data.
[All-NaN chunks with noncanonical payload 0x7fc0beef](nan-bits.json) preserve
those bits through both stream and materialization. The four benchmark
layout/selection combinations also pass [full independent bit/coverage checks](benchmark-correctness.json).

[Twenty-seven fault/retry cases](faults.json) cover index/payload retries and
exhaustion, interrupted bodies, ignored or misframed ranges, weak ETags,
HTTP content encoding, cancellation, mutations before/during execution to data,
metadata and coordinates, CRC failures, truncated indices, mixed sentinels,
invalid/overflowing/overlapping offsets, zero lengths and gzip expansion limits.
Rust tests also cover bounded Zstd frames, corrupt/trailing frames, missing
coordinate chunks, CF-missing coordinates and late local shard mutation.
No failed acquisition returns successful completion. Delivered chunks remain
provisional until final validation, including when mutation follows delivery.

Native group materialization follows the existing Zarr input writer's
completion/error-cleanup behavior. It is separate from the COG scientific sink's
atomic durable publication protocol; [that boundary is explicit](workflow.md).

## 6. Tuned reader comparison

The isolated [48-cell comparison](benchmark.json) uses current Zarr-Python
**3.4.1**, xarray **2026.9.0**, four requests/workers, four Python threads and
standard coalescing (1 MiB gap / 16 MiB merged range). The reference uses xarray
native indexing and the library's shard coalescer; a transport wrapper adds
strong ETag pinning, conditional reads and three metadata/data validation rounds.
It performs no replacement index parsing or codec work.

Both readers select the same three variables, three noncontiguous times,
coordinate vectors and native values; both consume validity, sums and raw-bit
checksums. Native output bytes, valid counts and sums agree in every cell.
No file output is timed. Tile-Ripper emits bounded chunks; Python retains the
selected arrays, and their result/metadata checksum granularity differs. Process
wall time (including startup) and internal completion are both retained. Native
first-chunk time and Python first-variable-ready time are separately named,
rather than treated as equivalent streaming events.

Three-repetition median completion seconds, including preparation and final
validation; warm HTTP mirror on one macOS workstation:

| Layout / selection | Local Tile-Ripper | Local xarray/Zarr | 8 ms, 100 Mbit/s Tile-Ripper | Shaped xarray/Zarr |
| --- | ---: | ---: | ---: | ---: |
| Ordinary / sparse | 0.0131 | 0.0229 | 0.2461 | 0.2873 |
| Ordinary / dense | 0.1316 | 0.2184 | 5.6038 | 6.0626 |
| Sharded / sparse | 0.0147 | 0.0249 | 0.2886 | 0.3352 |
| Sharded / dense | 0.0606 | 0.0823 | 1.7098 | 0.6575 |

The sparse/ordinary differences are modest and are not evidence of a universal
speed advantage. Python performs 11–13 additional requests in those cases
because its metadata discovery probes differ. Counts are disclosed rather than
claiming equal transport work from equal scientific selections.

Dense sharding exposes the important limitation. Both fetch 24 indices once.
Tile-Ripper makes **382 payload requests / 172,799 payload bytes**; Python
coalesces them to **24 requests / 310,026 payload bytes**. Including metadata,
Tile-Ripper uses **519 requests / 196,252 body bytes**, versus Python's
**172 / 333,479**. Both materialize exactly **1,572,864 native value bytes**.
Python is **2.60× faster** on this shaped cell despite transferring more bytes.
Zero full-shard GETs occur in either path; Python's coalesced ranges can include
unselected gaps. Requiring literally minimum bytes can cost more latency than
bounded coalescing.

Native `read_decode_seconds`, `retrieval_seconds`, their codec/inner-construction
residual, first delivery, total wall/RSS, byte counts and declared reservations
are retained per cell. Shaped sparse/dense sharded first-chunk medians are
0.2382 / 0.4008 s; the corresponding aggregate codec/inner-construction residuals
are 0.00065 / 0.01089 s across workers. These worker-time aggregates are not
wall-time decompositions. The residual includes library buffer construction and
admission overhead; it is not a pure codec CPU measurement. An exploratory
comparison with fewer Python validation rounds is excluded from these results.

## 7. Resource and distribution limits

The [64-fold selected-volume gate](memory.json) grows native output from 0.5 to
32 MiB for ordinary/sharded raw/gzip/Zstd/Blosc. All 16 cells pass the 192 MiB RSS
and under-64-MiB growth screens, with measured consumer peaks approximately
13–18 MiB. The benchmark consumer retains diagnostic chunk records and peaks at
16.8–53.7 MiB; Python peaks at 103.7–121.9 MiB. These include different runtimes
and retention strategies, not an equal-process-budget promise.

The public GFS consumer peaks at **61.5 MiB RSS**, with **36.3 MiB** declared
scheduler reservation inside its 256 MiB request budget. Preparation charges
coordinates, root attributes, retained indices and metadata estimates; aggregate
HTTP caches/retained preparation are admitted within half the working buffer.
Per-task admission accounts for native and encoded buffers and output planes.
Plan/node bookkeeping, transport/library/runtime overhead and caller retention
remain distinct from payload reservations. No hard RSS ceiling is promised.

[Clean Linux/aarch64 builds](distribution.json) pass from separate empty target
directories for native COG + Zarr, Zarr-only and combined all-feature builds.
Native and Zarr-only test/build/startup/stream checks run before GDAL is installed;
`ldd` has no GDAL/PROJ/TIFF dependency. Combined tests pass after installing GDAL.
The macOS native binary also has no GDAL/PROJ/TIFF dependency. The tested main
compiler is Rust 1.92; the five v3 integration tests pass on Rust **1.91.0** too.
The separate path-dependent consumer remains buildable with a locked dependency
graph. Input adds no registry package versions; the current Python reference is
isolated and frozen separately from acquisition/runtime dependencies.

## 8. Next milestone and durability qualification

Proceed with **more Zarr integration**, especially bounded adjacent-range
coalescing with the same index/version/finish safeguards, before adding another
format or downstream product integration. Expand layouts/CF mask references only
with independent fixtures and actual-user datasets. The public GFS profile is
useful already, while its large time chunks and the dense benchmark show why
layout-aware access matters. NetCDF/HDF5 and downstream integration are deferred;
no such implementation is included.

The optimized scientific writer's synchronization/publication sequence is
unchanged. The [synchronization audit](../zarr-v3-diagnosis/synchronization-audit.md)
distinguishes original-descriptor per-file host synchronization, directory/
ancestor synchronization, two checked macOS F_FULLFSYNC transaction requests,
atomic no-replace sibling rename and final parent/barrier checks. Accepted
requests and process fault tests are not measured physical device flushes or
power-loss recovery certification. Post-rename sync failure preserves complete
published data and reports unconfirmed durability. No synchronization was
removed for any input/output speed measurement.

## Replay

Build commands and binary/source hashes are in [verification.json](verification.json).
Use `pip install -r docs/zarr-v3-input/reference.txt` in a fresh Python 3.13
virtual environment. Existing older reference environments remain separate.

```sh
cargo build --release --locked --no-default-features --features native-cog \
  --target-dir target/zarr-v3-input --bin tile-ripper \
  --example zarr_subset_oracle --example chunk_stream
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_v3_input.py checks
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_v3_input.py faults
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_v3_input.py edges
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_v3_input.py extras
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_v3_input.py nan_bits
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_v3_input.py memory
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_v3_public.py
# Run timing after every build/other gate has completed:
prototype-output/zarr-v3-input/reference-env/bin/python bench/zarr_v3_compare.py benchmark 3
# Native-feature consumer is required for native COG fault admission:
cargo build --release --locked --no-default-features --features native-cog \
  --manifest-path consumers/native-stats/Cargo.toml --target-dir target/scientific-output
```

`bench/zarr_v3_regressions.py` redirects inherited gates into separate receipts;
use fresh generated destinations. `bench/zarr_v3_linux.sh` documents its Docker
invocation. Generated sources, binaries, rejected exploratory measurements and
raw per-cell HTTP logs remain under ignored prototype-output. Only validated
source changes, reproducible harnesses and research receipts are committed.
No merge, tag, release, publication or deployment was performed.
