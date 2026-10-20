# Durable scientific output decision gate — 20 October 2026

**Adopt the optimized uncompressed Zarr v2 writer.** For four native
2048×2048 Sentinel-2 windows, matched median end-to-end time fell from **4.846
to 0.527 s (9.2×)** on warm local HTTP and **7.566 to 3.294 s (2.3×)** on the
simulated 8 ms / 100 Mbit/s link. The native stream lower-work baselines were
0.369 and 3.089 s. Values, masks, native grids, timestamps and provenance remain
independently verified. This continues `82e21b1` on `codex/acquisition-planner`.

The main improvement replaces repeated macOS `F_FULLFSYNC` requests with two
transaction barriers, while retaining synchronization on every original writer
handle. Linux keeps per-file `sync_all`/`fsync`; no Linux speedup is claimed.
Immutable inspection metadata is shared across prepared-request clones, and
the manifest takes ownership of its inspection value rather than copying it.

Lossless compressed v2 output is useful when storage matters. **Blosc/Zstd level
1 with byte shuffle reduced the large output from 67.73 to 24.23 MB (64.2%)**, at
some write CPU and read cost. Ordinary fetch retains uncompressed v2 and its
existing chunk layout. V3 sharding is **not ready**: its experimental gzip
candidate failed all 12 medium/large matrix attempts in the pinned zarrs encoder,
with no destination published. Successful small, live and structured large-grid
v3 outputs do not override that failed real-data gate.

## 1. Measured bottleneck

The comparison uses the preserved, unmodified release binary from `82e21b1`,
whose SHA-256 matches the previous scientific-output verification receipt:
`a44a42df44908ef93d3c895e4022fe28a6f7690acb0d575167227d5333b6be62`.
The same frozen original Sentinel-2 ranges, requests, two reader workers,
per-host limit of two, 16 MiB source budget and 256-square source deliveries
are used for each mode. No competing build or validation job runs in the
timing matrix. Independent verification and reading probes follow each child.
The large runs all make the same 20 HEAD and 24 GET requests and transfer
25,134,367 response-body bytes, including metadata and validation traffic.

Warm-local medians, seconds:

| Per-observation window; four observations | Original wall | Original sync calls, timed | Sync share of wall | Optimized raw wall | Optimized sync calls, timed |
| --- | ---: | ---: | ---: | ---: | ---: |
| 96×96 | 0.485 | 0.399 | 82.2% | 0.097 | 0.013 |
| 512×512 | 1.031 | 0.796 | 77.2% | 0.109 | 0.017 |
| 2048×2048 | 4.846 | 4.449 | 91.8% | 0.527 | 0.056 |

This attributes the bottleneck to actual synchronization calls, rather than
assuming all store time is fsync. For the original large case, file open/create
and write calls took another 0.146 and 0.112 s. For optimized raw, those were
0.059 and 0.079 s. `mkdir` calls took about 0.002 s. The file-count and encoding
experiments below expose additional costs once full-flush latency is removed.

The optimized large raw profile separately measures:

| Component | Median seconds | Interpretation |
| --- | ---: | --- |
| Source data HTTP retrieval | 0.0511 | Existing native transport counters; metadata/validation remain separately recorded |
| Native read/decode including retrieval | 0.2532 | Sum of worker durations; overlaps output work |
| Native decode/copy residual | 0.2023 | Per-run read/decode minus retrieval; includes reader bookkeeping, not a pure codec CPU measurement |
| Zarr chunk encode/library work | 0.00423 | Explicit encode-call timer minus storage I/O; includes coordinates/time/scalar calls |
| Output setup | 0.0244 | Metadata, coordinates and admission; includes its storage work |
| Manifest generation / serialization / write | 0.000034 / 0.000609 / 0.000315 | Three distinct stages |
| Final directory walk and synchronization | 0.00348 | Bottom-up staging directories and destination ancestors |
| Pre-publication device flush | 0.00872 | Checked macOS `F_FULLFSYNC` |
| Atomic no-replace rename | 0.000186 | Commit operation |
| Publication parent synchronization | 0.000036 | Checked after rename |
| Post-publication device flush | 0.00440 | Checked macOS `F_FULLFSYNC` |
| Complete publication phase | 0.0178 | Includes ordering/check/bookkeeping overhead |

These rows are **not additive wall-time partitions**: worker sums overlap the
sink, setup contains I/O, and component medians need not sum to a median total.
Directory traversal and unwrapped metadata lookups remain inside stage/residual
time; the interposer is not an exhaustive filesystem syscall tracer. The old
writer's encode-only field was uninstrumented, not measured zero. The raw
receipts and derived [profile](profile.json) retain that distinction.

The stored manifest is finalized before its own write and publication. Its
`observed` counters exclude those later operations. Full publication timings are
external diagnostics, recorded through `TILE_RIPPER_STAGE_TRACE` and the
interposer, so profiling does not require changing a synchronized published
manifest afterward.

## 2. Synchronization protocol and safety

The existing zarrs filesystem dependency, 0.3.13, synchronizes each write through
`File::sync_all`. On macOS this invokes `F_FULLFSYNC`. Apple distinguishes host
`fsync` from the additional request to flush the drive's buffered data:
[Apple fsync](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html),
[Apple fcntl](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html).

A scoped copy of that same filesystem crate adds one default-off option and a
small synchronization branch; its encoding, storage operations, locking and
key mapping are unchanged. [Patch note and upstream licenses](../../vendor/zarrs_filesystem/TILE-RIPPER-PATCH.md).
The ordinary direct-Zarr adapter still uses the unchanged registry dependency.
There is no unsynchronized-write mode and no replacement Zarr store.

The COG scientific sink follows this order:

1. Reserve a unique hidden sibling of the destination; refuse existing files,
   directories and dangling symlinks. Source deliveries go directly to zarrs.
   Every write synchronizes contents and metadata **on its original writer
   descriptor before close**: POSIX `fsync` on macOS, unchanged `sync_all` on Linux.
2. Require successful stream `finish()`, final source-version validation, exact
   planned-delivery coverage and a cancellation check. Finalize root metadata
   and `manifest.json`; those writes also synchronize their original handles.
3. Synchronize staging directories bottom-up, then destination ancestors,
   including entries for newly created parent directories. Check cancellation
   throughout. Issue a checked device barrier on the regular manifest descriptor
   on macOS. Unsupported/failed `F_FULLFSYNC` aborts; it does not silently fall
   back to weaker synchronization.
4. Check cancellation immediately before the atomic no-replace sibling rename
   (`renamex_np(RENAME_EXCL)` / `renameat2(RENAME_NOREPLACE)`). Then synchronize
   the destination parent and issue the post-rename device barrier using the
   same still-open manifest inode. Return success only after these checks.

Per-file synchronization was deliberately **not** replaced with closing every
unsynchronized writer and reopening files at the end. Linux's write-back error
tracking makes descriptor lifetime relevant; retaining the original-handle check
avoids introducing that error-reporting gap. Directory entries also need their
own synchronization. [Linux fsync](https://man7.org/linux/man-pages/man2/fsync.2.html),
[kernel error-sequence tracking](https://www.kernel.org/doc/html/latest/core-api/errseq.html).

The large case changes **941 observed F_FULLFSYNC requests into 952 host fsync
requests and two checked F_FULLFSYNC requests**. These are syscall observations,
not measured physical hardware flushes. It reduces device barriers, not the requirement
to synchronize every file. The extra host calls include directory ancestors and
checked publication synchronization. File writes and sizes remain comparable.

Before commit, failures/cancellation remove only the owned staging directory.
SIGKILL can leave `.NAME.partial-PID-ID`; retries neither adopt nor delete it.
A staged root may already say `complete` during final synchronization: the hidden
name still means uncommitted. Trust the requested destination and successful
completion, not a staging manifest in isolation.

After rename, a synchronization failure retains the **complete** destination
and returns an explicit `published ... durability is unconfirmed` error. It does
not claim success or delete committed data. SIGINT racing after the final
cancellation poll can commit; that path completes synchronization. An in-flight
filesystem/HTTP operation need not stop immediately when cancellation is set.

[31 injected cases](faults.json) cover EIO, SIGINT and SIGKILL during metadata
writing, file synchronization, pixel writing, pixel synchronization, final
directory synchronization, ancestor synchronization, the pre-publication device
barrier, rename, parent synchronization and the post-publication device barrier,
plus an actual destination race. Pre-commit kills retain only identifiable hidden
staging and retry safely; post-commit kills leave a fully populated dataset.
Rust tests separately cover empty-directory, regular-file and dangling-symlink
destination races, nested parent creation and owned-staging cleanup.

These are application-level transactional and syscall-failure tests, **not
power-cut tests**. Successful synchronization requests support an ordered local
filesystem durability protocol when the OS, filesystem and device honor them.
They do not certify arbitrary controllers, devices, network filesystems, Docker
mounts or behavior after actual power loss. The original writer's ignored parent
sync error is now checked, but universal power-loss guarantees are not claimed.

## 3. End-to-end performance and storage tradeoffs

Medians of three shuffled runs per cell; end-to-end seconds:

| Four-observation workload | Link | Original v2 | Optimized raw v2 | Gzip v2 | Blosc/Zstd v2 | Native stream, no dataset |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 96-square | Warm local | 0.485 | 0.097 | 0.087 | 0.090 | 0.058 |
| 96-square | 8 ms / 100 Mbit/s | 1.149 | 0.743 | 0.746 | 0.746 | 0.716 |
| 512-square | Warm local | 1.031 | 0.109 | 0.134 | 0.137 | 0.073 |
| 512-square | 8 ms / 100 Mbit/s | 1.264 | 0.775 | 0.894 | 0.759 | 0.728 |
| 2048-square | Warm local | 4.846 | 0.527 | 0.816 | 0.667 | 0.369 |
| 2048-square | 8 ms / 100 Mbit/s | 7.566 | 3.294 | 3.583 | 3.343 | 3.089 |

Large warm-local tradeoffs; MB means decimal MB, RSS means MiB:

| Mode | Wall s | CPU s | Max RSS MiB | Dataset MB | Files | File write calls | Host fsync / full device flush |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Original raw v2, edge 256 | 4.846 | 0.577 | 48.2 | 67.73 | 904 | 906 | 0 / 941 |
| Optimized raw v2, edge 256 | 0.527 | 0.387 | 44.0 | 67.73 | 904 | 906 | 952 / 2 |
| Gzip level 1 v2, edge 256 | 0.816 | 0.698 | 43.9 | 34.40 | 904 | 906 | 952 / 2 |
| Blosc/Zstd level 1 + byte shuffle v2, edge 256 | 0.667 | 0.473 | 42.0 | 24.23 | 904 | 906 | 952 / 2 |
| Raw v2, output edge 128 | 1.717 | 0.933 | 46.7 | 67.73 | 3,272 | 3,274 | 3,320 / 2 |
| Raw v2, output edge 512 | 1.154 | 0.466 | 44.0 | 67.73 | 296 | 874 | 920 / 2 |
| Native stream | 0.369 | 0.259 | 37.5 | 0 | 0 | 0 | 0 / 0 |

The 512-edge output reduces files but revisits destination chunks because source
deliveries remain 256-square. It reads **201.33 MB** of destination data and
writes **268.65 MB** through the output store, versus about **67.32 MB** of store
writes and no destination reads at edge 256. This read/modify/re-encode work
outweighs the file-count saving here. Edge 128 creates more files and encode
operations. Neither alternative becomes the default.

Independent warm Zarr-Python reads of every value/mask array in the large dataset:

| Output | Full read wall / CPU s | 64×64 subset wall / CPU s |
| --- | --- | --- |
| Raw 256 | 0.256 / 0.163 | 0.0284 / 0.0189 |
| Gzip 256 | 0.343 / 0.321 | 0.0420 / 0.0244 |
| Blosc/Zstd 256 | 0.332 / 0.277 | 0.0268 / 0.0207 |
| Raw 128 | 0.765 / 0.557 | 0.0448 / 0.0208 |
| Raw 512 | 0.105 / 0.0669 | 0.0522 / 0.0236 |

The raw 512 layout helps full reads while making small reads and this streaming
write pattern less attractive. Compression is lossless and applies to the three
spatial arrays; coordinate/time/scalar arrays remain uncompressed. Metadata,
sample semantics, source deliveries and the transaction protocol are otherwise
the same. Counts are storage/kernel-facing calls, not physical device IOPS.

[benchmark.json](benchmark.json) retains all **144 cells**: 108 verified v2
datasets, six verified small v3 datasets, 18 stream runs and 12 failed v3 attempts.
Failed attempts are not successful fast writes. The matrix was resumed once after
the first experimental error; completed cells were retained and the failed cell
was rerun with explicit failure recording. Binaries are unchanged across the
retained measurements. Variation, warm host caches, the interposer, three
repetitions and one macOS workstation limit inference. The shaped link is a
frozen local simulation, not a public-provider throughput claim. Source capture,
reference decoding and all independent validation are outside write timing.

## 4. Scientific interoperability and memory

The ordinary live CLI again creates **six Sentinel-2 observations, B04/B08/B11
on 2 and 12 January 2025, with native 10 m and 20 m grids**. Every one of the
46,376 samples and both masks is checked against the independent rasterio/GDAL
source oracle, with exact timestamps, affine centers, CRS reconstruction,
calibration metadata and source versions. The compressed v2 and experimental v3
candidates repeat that full live comparison. [Default](live-default/live.json),
[candidates](live-candidates.json).

Controlled checks cover two UTM zones, different resolutions/extents, polygon
holes, source nodata, partial source coverage and microsecond timestamps in
reverse discovery order. Empty selections, undersized buffers, interrupted
sources and late ETag mutation pass for every candidate. [Candidate receipt](candidates.json),
[inherited default cases](inherited/validation.json).

All successful matrix outputs are read with **Zarr-Python 3.1.3 and xarray
2025.6.1** and compared to independent GDAL references. The controlled v2
candidates are also fully reopened with **Zarr-Python 2.18.3/xarray**.
[V2 reopen](interoperability-v2.json). Linux ARM64 builds without GDAL,
PROJ or libtiff pass the transaction unit tests and independently reopen all four
live encodings against the separate GDAL hashes. [Linux receipt](linux.json).
V3 readers retain a redundant legacy `_ARRAY_DIMENSIONS` attribute beside v3
dimension names; the Linux oracle checks rank/dimension semantics separately
and compares all scientific CRS fields.

The public `ChunkStream` contract, rendezvous/backpressure behavior and source
version checks are unchanged. The sink retains no scene assembly or persistent
shard cache. Every encode call has an explicit admission reservation, including
one bounded shard if the experimental library path materializes it. Scratch is
released before the next delivery. At 256-edge output, declared writer
reservations are **2.375 MiB raw, 3.375 MiB compressed v2, and 9.375 MiB v3**.
They are separate from the source scheduler's reservations and include a
conservative buffer multiplier and codec workspace allowance, not an allocator
or RSS guarantee. Add the two reported reservations when considering memory;
16 MiB is not a combined process-memory ceiling.

Two timestamped observations grow from 1024-square to 8192-square, increasing
logical value+mask arrays from **8 to 512 MiB**. Every pixel/mask is independently
checked in bounded tiles. Writer reservations stay constant; final peak RSS is
**69.7 MiB raw, 70.4 MiB gzip, 71.9 MiB Blosc/Zstd and 72.8 MiB v3**, passing
the 192 MiB RSS screen and the less-than-64-MiB growth screen. Initial graph
copies produced roughly 93–95 MiB on the large case; sharing immutable inspection
metadata and moving it into the manifest removes that avoidable cost.
[Memory receipt](memory.json). Plan metadata and file-key bookkeeping still grow
with chunk/observation counts and are bounded by `max_nodes`, rather than being
flat RSS for unlimited datasets. Runtime, allocator behavior and OS page cache
remain outside explicit buffer reservations.

Inherited regressions pass **61 native COG profile/fault cases**, exact comparison
of **32,391,168 real-source samples**, and the seven public-consumer checks for
COG/Zarr/streaming semantics. [Regression receipt](regressions.json). Rust checks
pass 65 default, 43 GDAL-free native, 43 experimental native and 83 all-feature
tests, with their existing external-fixture ignores explicitly counted. Strict
clippy, Zarr-only compilation, separate locked consumer build, formatting and
diff checks pass. [Verification](verification.json).

## 5. Format decision

**Zarr v2 remains the appropriate default.** The aligned, uncompressed 256-edge
writer is fastest for the larger warm workloads, preserves broad reader support,
and now adds only about 0.16 s to the large local native-stream median and 0.20 s
on the shaped link. Blosc/Zstd is a justified explicit storage-saving candidate,
not a universally faster write/read default. Select candidate settings through
`Prepared::fetch_scientific_with_options` or the supplied benchmark example.

**V3/sharding warrants a separate, gated milestone**, but cannot be promoted from
these results. This candidate uses standard `sharding_indexed`, 2×2 inner chunks,
gzip level 1 and a bytes-encoded end index. It uses the pinned library's full-shard
encoding path; it can rewrite a shard on each inner delivery. Its index has no
optional CRC32C codec. [Sharding specification](https://zarr-specs.readthedocs.io/en/latest/v3/codecs/sharding-indexed/index.html).

The library's bounded encoder returns `Sharding did not allocate a large enough
buffer` on all medium/large real-source cases. The follow-up [v3 diagnosis](../zarr-v3-diagnosis/report.md)
confirms an upstream gzip size-bound defect, exposed on these inputs by the
native build's zlib-rs backend. It reproduces independently in memory; released
zarrs 0.23.14 passes the corresponding controls. The original pinned failure
[receipt](sharding-failures.json) remains unchanged. The next gate is a deliberate
upstream upgrade followed by integration, write-amplification, independent,
fault and memory validation. Smaller file counts or successful compressible
fixtures alone are insufficient grounds for adoption. The follow-up
[synchronization audit](../zarr-v3-diagnosis/synchronization-audit.md) separates
publication guarantees, conditional crash consistency and syscall observations;
no actual hardware flush or power-loss survival was measured.

Reproduce with the [protocol](protocol.md). Raw images, mirrors, generated
datasets, logs and exploratory failed screens remain local in ignored
`prototype-output/durable-output/`; compact receipts and harnesses are versioned.
