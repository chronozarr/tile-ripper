# Native COG feasibility and GDAL-free core decision

2026-10-19. Continued locally from `911a268` on `codex/acquisition-planner`.
The default GDAL reader is retained. No merge, publication, service deployment,
ChronoZarr integration, projection rewrite or new source format occurred.

## 1. Library choice

Use the **geotiff-reader family**, specifically `tiff-reader=0.9.0` for TIFF
parsing, predictor reversal, bounded codec decoding, typed native windows and
block caching, and `geotiff-core=0.9.0` for GeoKeys/CRS/native affine metadata.
The high-level GeoTiffFile constructor cannot accept our custom source because
`from_tiff` is private; using the underlying public TiffSource avoids a fork.
Tile-Ripper adds versioned range transport, a narrow profile gate, calibration
XML interpretation and scheduling. It does not implement a TIFF decoder.

[Library assessment](libraries.md) compares actual published implementation code
for async-tiff 0.4.0, geotiff-reader 0.9.0, cogrs 0.0.4 and cogrs-eg 0.2.0,
including HTTP correctness gaps, allocation limits, masks, float conversion,
codecs and object_store dependencies. [Source hashes](library-audit.json) pin the
audited files. Other candidate libraries were inspected, not benchmarked.

## 2. Feasibility and integration

**A genuinely GDAL-free direct-asset/native-streaming core is feasible and works.**
The internal SelectedCogReader enum switches between the retained GDAL reader and
the new native reader. Public Request, Chunk, Pixels, stream ordering,
backpressure, cancellation and completion contracts are unchanged. Native values
remain UInt16; scale/offset are carried as metadata and never applied.

| Build | Reader and workflow |
| --- | --- |
| Default | Existing GDAL behavior and default; compatibility retained |
| `--features native-cog` | Both readers; select native with `TILE_RIPPER_COG_BACKEND=native`; executable still links GDAL |
| `--no-default-features --features native-cog` | GDAL-free direct COG bounds/streaming plus existing direct Zarr acquisition/output |
| `--no-default-features` | GDAL-free direct Zarr workflows; no COG reader |

The GDAL-free COG path uses **bounds in the exact source CRS**. Longitude/latitude
AOI transformation to UTM, polygon clipping/rasterization, STAC geometry discovery
and GeoTIFF writing remain in the optional GDAL extension. These operations fail
explicitly when GDAL is disabled. No GDAL-free projection or GeoTIFF-output claim
is made. See [installation](../installation.md) and [consumer instructions](../../consumers/native-stats/README.md).

## 3. Admitted and unsupported profiles

Admitted: self-contained tiled, single-band MinIsBlack UInt16; top-left
orientation; classic TIFF and BigTIFF; either byte order; uncompressed, LZW or
Deflate (8/32946), predictor 1/2; finite north-up pixel scale plus one 2D tiepoint;
horizontal EPSG authority metadata; PixelIsArea; absent nodata or representable
UInt16 nodata; ordinary GDAL scale/offset XML. Non-full edge tiles are covered.
All real-scene evidence here is Sentinel-2 B04/B08, 1024×1024 Deflate/predictor-2,
UTM 17N, nodata 0, scale 0.0001 and offset -0.1. The synthetic codec/byte-order
matrix also uses UTM 17N; other CRS combinations have no real-data validation.

Rejected or outside this adapter's validated contract: strips; RGB, palette,
alpha/multiband and planar-separated samples; signed/float/packed samples;
JPEG/JPEG2000, ZSTD, LERC, WebP, LZMA and other codecs; explicit internal masks
and SubIFDs; PixelIsPoint; sparse blocks; transformation matrices/rotation;
custom/compound/vertical CRS semantics and unsupported calibration roles.
External masks, PAM, worldfiles and other sidecars are not consulted: only
self-contained TIFF metadata is supported. Use GDAL for those sources rather
than treating this profile as a general GeoTIFF reader.

Limits: ≤2 MiB decoded tiles, ≤65,536 base tiles, ≤16 top-level IFDs, ≤256 tags/IFD,
≤1 MiB/tag, ≤2 MiB aggregate encoded metadata, ≤512 KiB metadata page cache and
≤8 MiB decoded-block cache. Encoded tiles are further limited relative to the
decoded block. Larger or scattered metadata can fail these budgets. Unknown
GeoTIFF combinations need further conformance testing; the admitted fixture
profile is not a claim of universal TIFF semantic validation.

## 4. Performance against both controls

Accepted matrix: 72 fresh-process trials (three readers × four workloads × two
links × three repeats), plus 32 independent GDAL tuning trials at 1/2/4/8 workers.
The preserved baseline consumer hash matches the 911a268 milestone. The frozen
mirror index is verified. Requests, windows, native output semantics and source
versions match. Native and baseline chunk records/order are compared directly.
The [protocol](protocol.md), [all remote rows/ranges](benchmark.json) and
[local decode isolation](local-decode.json) retain every measured result.
MB is decimal; RSS is MiB. Tables are medians; JSON includes minima/maxima.

8 ms latency, shared 100 Mbit/s:

| Workload | Completion s: native / current / control | First chunk s: native / current / control | RSS MiB: native / current / control | CPU s: native / current / control |
| --- | --- | --- | --- | --- |
| many-small | 2.920 / 2.948 / 2.464 | 0.745 / 0.736 / 1.128 | 38.0 / 98.2 / 149.9 | 0.294 / 0.475 / 0.557 |
| several-large | 4.830 / 6.398 / 4.845 | 0.364 / 0.450 / 0.851 | 45.9 / 124.0 / 202.0 | 0.596 / 0.855 / 0.864 |
| overlap-a | 4.608 / 6.040 / 4.897 | 0.361 / 0.440 / 0.596 | 37.7 / 133.5 / 181.6 | 0.328 / 0.519 / 0.647 |
| overlap-b | 4.713 / 3.507 / 3.383 | 0.874 / 1.202 / 3.053 | 41.8 / 126.4 / 201.8 | 0.348 / 0.447 / 0.597 |

Unshaped loopback:

| Workload | Completion s: native / current / control | First chunk s: native / current / control | RSS MiB: native / current / control | CPU s: native / current / control |
| --- | --- | --- | --- | --- |
| many-small | 0.132 / 0.335 / 0.276 | 0.032 / 0.132 / 0.089 | 31.7 / 92.4 / 137.3 | 0.188 / 0.409 / 0.451 |
| several-large | 0.442 / 0.540 / 0.313 | 0.025 / 0.066 / 0.074 | 42.8 / 113.9 / 168.1 | 0.439 / 0.584 / 0.597 |
| overlap-a | 0.297 / 0.400 / 0.309 | 0.022 / 0.064 / 0.071 | 27.6 / 110.2 / 179.0 | 0.280 / 0.426 / 0.633 |
| overlap-b | 0.316 / 0.328 / 0.336 | 0.056 / 0.101 / 0.116 | 32.8 / 133.2 / 167.3 | 0.288 / 0.419 / 0.516 |

Shaped transfer accounting (HEAD counts and physical-block details are in JSON):

| Workload | Body MB: native / current / control | GETs: native / current / control |
| --- | --- | --- |
| many-small | 25.829 / 26.526 / 26.362 | 80 / 37 / 32 |
| several-large | 37.747 / 56.918 / 56.820 | 48 / 33 / 30 |
| overlap-a | 37.747 / 56.918 / 56.820 | 48 / 33 / 30 |
| overlap-b | 37.747 / 38.109 / 38.044 | 48 / 20 / 18 |

Large-window traffic drops **33.7%**, from 56.918 to 37.747 MB, with 48 versus 33
GETs. Shaped completion improves **24.5%** against current Tile-Ripper and is
approximately tied with the tuned GDAL control. Many-small is a tie against
current Tile-Ripper under shaping. Overlap-b is **34.4% slower** than current
Tile-Ripper and **39.3% slower** than the tuned control, despite slightly fewer
bytes. It is a real loss, retained in the decision.

Native selected-block duplicate payload is **zero in all 24 remote native trials**.
Data-phase GETs request only intersecting native tiles. Bounded 4-KiB metadata
pages also touch 127,388 bytes of unselected image payload for many-small and
47,764 bytes for each six-source workload; they are explicitly counted, not
called metadata or hidden. This is not a zero-overfetch claim. GDAL control traffic
varies in unshaped overlap-a: median 65.339 MB includes 14.706 MB of duplicate
selected payload under the independently tuned/global-cache configuration.

The cache is min(selected decoded tile footprint, 8 MiB), shared across every
window in a selection. The exploratory 4-MiB cache reread crossing tiles and was
replaced before the accepted matrix. With the identical 16-MiB working budget,
many-small admits two native readers, while four-tile selections reserve 8 MiB
and admit one. Wider selections can exceed the cache and reread; there is no
unbounded cache or universal one-fetch-per-tile promise.

Decode cost is material, even where completion wins:

| Workload | Local read/decode worker s: native / current / control | Native HTTP residual s, unshaped / shaped |
| --- | --- | --- |
| many-small | 0.135 / 0.110 / 0.180 | 0.134 / 0.194 |
| several-large | 0.220 / 0.164 / 0.286 | 0.201 / 0.323 |
| overlap-a | 0.201 / 0.159 / 0.265 | 0.195 / 0.225 |
| overlap-b | 0.202 / 0.160 / 0.262 | 0.198 / 0.241 |

Local read/decode includes warm local I/O and window assembly, and sums worker
durations. Native residual is read/decode minus measured synchronous range time;
it estimates decode/allocation/copying, not an isolated codec microbenchmark.
Several-large local native read/decode is about 34% higher than current GDAL.
Lower startup and transfer can nevertheless reduce complete wall time.
CPU seconds/percent, opening, planning, transport and buffer reservations are in
JSON. First-chunk timers are internal: Rust includes preparation but excludes
process startup; GDAL starts after Python imports and records first computed
chunk. They are not an equivalent external first-byte latency benchmark. Wall
time includes fresh-process startup and completion. Oracle work is excluded.

## 5. Scientific correctness, faults and resources

All accepted trials pass the independent scientific oracle: exact coverage,
checksums, UInt16 dtype, validity, native sums, timestamps, variable identity,
CRS/affine, scale/offset and completion. A second standalone GDAL-free consumer
compares **32,391,168 UInt16 samples directly, bit for bit**, against independently
generated rasterio bytes across all four workloads; it does not rely on checksum
collision assumptions. See [direct equality receipts](exact.json).

The **61-case** [profile/fault suite](validation.json) passes classic/BigTIFF in
both byte orders, the three codecs/predictors, partial tiles, nodata None/0/65535,
invalid profile rejection, malformed counts/IFD cycles/GeoKeys/XML, bad compression,
corrupt/truncated payloads, decoded-length mismatch, a Deflate output bomb,
BigTIFF count overflow, wrong HTTP status/range/total/length/encoding, missing or
changed response ETags, 412, one-shot 503/truncation recovery, exhausted retries,
late remote/local mutation, cancellation and consumer failure. Errors never emit
a successful completion receipt. The retained GDAL control's own snapshot/failure
regression also passes. Rust tests and warning-free checks cover all feature
combinations; GDAL-only fixture tests are gated when that feature is absent.
A dedicated test exercises late local mutation through the older execute API.

All 24 native remote acquisitions stay under the 192-MiB process gate: maximum
**48.59 MiB**. Admission reserves decoded cache plus conservative encoded,
decode and output working buffers; metadata, runtime and caller retention remain
separate. This is a tested-fixture gate, not a universal RSS guarantee. Independently
tuned GDAL controls can exceed 192 MiB; those rows remain visible and are not
called resource-equivalent wins. HTTP retries are bounded; cancellation is
cooperative and an in-flight request can wait for its timeout.

Every native HTTP range requires 206, exact Content-Range/size/body length,
identity encoding and the pinned strong ETag, with If-Match and no redirects.
Parser/decode limits precede large allocations. Reader caches are selection-local
and discarded between versions. Existing initial/pre/post version validation is
retained; chunks remain provisional until finish succeeds. As before, strong ETags
and local size/mtime are trusted identities, not independent whole-object hashes.

## 6. Deployment benefit

Actual macOS ARM64 file sizes: CLI **13.460 MB** versus **13.996 MB**
(3.8% smaller); public consumer **12.910 MB** versus
**13.321 MB**. The main benefit is removing the GDAL installation,
not a dramatic reduction in executable bytes. Recursive Mach-O inspection finds
**179 non-system dynamic files / 161.66 MB** behind the baseline and **zero
non-system dynamic files** behind the native executable. This excludes OS libraries,
GDAL/PROJ data files, plugins and headers; it is not total installation size.

Median warm process-to-help startup (21 randomized trials): native
**4.24 ms**, baseline
**116.28 ms**. These are
startup probes, not cold-install or acquisition latencies; ranges are recorded.

A clean official Rust 1.92 Linux ARM64 container built CLI and consumer with no
GDAL/PROJ/libtiff installed. Empty-target builds took 58.60 s and 49.88 s. A separate
clean cargo-install test took **64.36 s**, installed the CLI, ran help and a native
16-asset plan. A separate Python slim runtime container, with GDAL/PROJ/libtiff
absence checked, ran the actual native consumer on all sixteen real-source
windows and verified the independent expected samples. Linux startup median is
**2.15 ms**; CLI/consumer sizes are
**15.614/15.009 MB**.
`ldd` shows only libstdc++, libgcc_s, libm, libc and the loader. Full actual
linkage, image digests, OS/glibc, build/install logs and dependency tree are in
[deployment evidence](deployment.json).

Rust alone was insufficient: the first slim-container attempt failed without a
C++ compiler. Existing vendored Zarr Blosc/Snappy and TLS code still needs C/C++
build tools, CMake and pkg-config. TIFF Deflate/LZW are Rust decoders. No system
GDAL, PROJ, libtiff or codec library is mandatory for the native build; the OS
C++ runtime remains mandatory. Build peak RSS is about 1.5 GiB and is distinct
from the acquisition gate. A reused cross-container target produced a stale
proc-macro dependency failure; the successful clean recipe uses separate empty
CLI/consumer targets. Both failures and successful proofs are retained.

## 7. Remaining risks

Pre-1.0 upstream APIs, narrow metadata interpretation, untested GeoTIFF combinations,
external masks/sidecars, a capped cache for wide windows, extra range requests,
conservative admission and the existing native codec build toolchain remain costs.
Only macOS ARM64 and Linux ARM64/glibc in the recorded containers were exercised.
Windows, Linux x86_64, Intel macOS, musl, WASM and portable binary bundles are
unverified. Supporting a codec upstream does not automatically admit it here.
Further conformance/fuzzing, CRS/mask coverage and cancellation/timeout work should
precede a broad-reader promise. Keep exact dependency pins and the frozen oracle.

## 8. Default decision

**Integrate as an opt-in alternate reader; keep GDAL as the default.** The narrow
native core passes science, fault and resource gates and removes the GDAL runtime
for direct native paths. Its large-request byte/RSS/startup advantages justify
keeping it. The overlap-b loss, request/admission tradeoffs and incomplete profile
and platform coverage do not justify changing defaults. No further integration,
format expansion or deployment is part of this decision.
