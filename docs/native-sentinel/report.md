# End-to-end GDAL-free Sentinel decision

**The success condition is met within the documented narrow profile.** A scientist
can supply a WGS84 Polygon, inclusive date range and supported Sentinel-2 bands,
discover Earth Search sources, acquire correct masked native chunks and consume
them incrementally using Tile-Ripper without installing GDAL, PROJ or libtiff.
Validated changes are local to `codex/acquisition-planner`, starting at `40518b5`.
GDAL remains the default build and compatibility reader. No merge, release,
publication, deployment, ChronoZarr integration or format expansion occurred.

## Decision gate

| Question | Finding |
| --- | --- |
| Ordinary polygon/date/band acquisition genuinely GDAL-free? | **Yes**, using `--no-default-features --features native-cog`, the shared STAC planner and public Rust stream. The clean Linux runtime executes this entire workflow. GeoTIFF writing is optional and still uses GDAL. |
| Supported CRS/geometry? | Exactly WGS84 `EPSG:4326` longitude/latitude queries to WGS84 UTM 32601–32660 / 32701–32760; nearby zone and matching hemisphere; simple closed 2D Polygons with holes, concavity, clipping and internal split pieces. Limits and rejections are explicit below. |
| Scientific agreement? | **95/95** independent checks: 70 acquisitions, 19 rejection cases, six chunk-size invariance checks. **755,798 UInt16 samples / 1,601 chunks** and metadata agree exactly with independent GDAL references. Live two-band Sentinel acquisition adds **20,566 exact samples**. |
| Installation/memory/runtime evidence? | Clean Rust 1.92 Linux ARM64 source install and separate minimal runtime pass with no GDAL/PROJ/libtiff. Actual dynamic dependencies are OS libraries/C++ only. Live consumer RSS: **29.4 MiB macOS / 20.4 MiB Linux**. Build tools and build memory remain separate requirements. |
| Native versus equivalent GDAL performance? | The frozen 72-trial comparison retains ties, wins and losses: several-large **4.83 vs 4.85 s**, overlap-b **4.71 vs 3.38 s** against tuned GDAL. Current candidate also passes 24 three-reader diagnostic replays. No general or end-to-end public-provider speed advantage is established. |
| GET-count problem understood/improved? | **Understood; production count unchanged.** Native 48 = 24 metadata-page GETs + 24 tile GETs. Connections are reused. Broader metadata reads and paired tile reads explain GDAL's smaller counts. Coalescing needs a redesigned, explicitly reserved encoded buffer; none was added speculatively. |
| What prevents native becoming default? | Narrow CRS/TIFF/geometry coverage, remaining request/concurrency cost, overlap-b loss, limited platform coverage, upstream pre-1.0 dependency exposure and need for independent scientist experience. A dual-feature executable still uses GDAL geometry and links GDAL. |
| Ready for its own private-alpha trial? | **Technically ready for a supervised trial of this narrow native stream workflow.** A self-contained trial is prepared. No human trial has completed; existing ownership/licensing approval remains pending before source distribution. This is not a new release/default decision. |

## Implementation and boundary convention

The validated `tiff-reader` / `geotiff-core` decoder and version-bound range
transport remain intact. New optional pinned dependencies are `geo 0.33.1` for
OGC polygon validity/planar clipping and `proj4rs 0.2.1` for WGS84 UTM transforms.
Their default features are disabled. STAC's catalog code, deterministic item and
requested-band ordering, source identities, timestamps, calibration provenance,
planner, bounded scheduler and final source validation are reused.

User geographic AOIs are limited to ±80° latitude, at most 5° extent per axis,
10,000 input positions and 100,000 transformed positions. The target UTM zone's
central meridian must lie within 6° of every query longitude, with matching
hemisphere. Geographic edges are straight and densified to ≤0.001° before
mapping. Other datum/axis/unit semantics, antimeridian wrapping, invalid topology,
3D positions, inverse/cross-zone query transforms and public MultiPolygon or
collection requests fail explicitly. Existing exact-native-CRS rectangular
selection remains available. There is no raster reprojection engine.

Native grids must be north-up PixelIsArea within the admitted tiled single-band
UInt16 lossless TIFF profile. AOI envelopes use clipped floor/ceil windows;
source nodata is retained and excluded centers become that same nodata value.
Masking requires representable source nodata; unsupported TIFF profiles fail.
Chunks containing no included centers are skipped. Partial coverage is clipped,
and a completely empty selection can finish with zero chunks.

Pixel inclusion uses GDAL's default scanline rules, including vertical and
directed horizontal center ties and holes (`ALL_TOUCHED=false`). Coordinates,
crossings and rounding are evaluated in **source-global pixel coordinates**;
integer output-window offsets are applied afterward. An extra audit found that
per-window floating origins could change exact center ties with chunk size,
including ordinary 60 m Sentinel grids. This was fixed and independently gated
at chunk edges 17, 32 and 64. The GDAL oracle burns the full source grid once,
then crops. Separately rasterizing a GDAL subwindow can round exact ties
differently; the declared full-grid convention is the native authority.

## Independent acceptance and regressions

The [correctness matrix](correctness.json) executes rasterio 1.4.4 / GDAL 3.10.3 /
PROJ 9.7.1 plus GEOS through Shapely 2.1.2 independently of Tile-Ripper geometry.
It covers north/south UTM, several zones, adjacent zones, winding, holes,
concavity, boundary centers, split clipping, nodata, partial/empty coverage,
dates, explicit time instants, ordering, native affine coordinates and rejection
cases. Maximum mapped/clipped geometry difference is **3.81 µm** (declared
reference tolerance 1 mm); delivered stored values and chunk metadata are exact.
Additional Rust tests execute system GDAL/PROJ comparisons, including 518
boundary/winding cases and source-global partition tests.

The [live receipt](live.json) independently discovers and checks
`S2A_T17SPV_20250728T161108_L2A`, B04/red then B08/NIR on 28 July 2025. Both source
ETags and sizes are pinned and rechecked. Eight native chunks contain 20,566
samples, of which 10,008 are valid; every stored value agrees with GDAL's
independent read/mask. Native acquisition measured 26.77 s in this macOS run and
3,035,058 encoded **data** bytes in two tile requests. These counters exclude
discovery, metadata GETs and validation HEADs; no total public HTTP byte count is
invented. Public timings vary substantially and are not a controlled benchmark.

The [eight STAC workflow checks](workflow-checks.json) additionally exercise
pagination, duplicates, off-date candidates, false-positive footprints, reversed
band order, empty dates, unsupported bands, consumer failure, cancellation and
source mutation after provisional delivery. No failure returns a completion
receipt. The stream's original provisional-delivery/`finish()` contract remains.

[Regression receipts](regressions.json) repeat all **61 native COG profile/fault
checks**, **32,391,168 bit-exact frozen real-source samples**, and seven existing
COG/Zarr public-consumer cases, including late mutations, UInt16 Zarr and
Float32/NaN/reversed/swapped-axis Zarr. The older convenience API's post-delivery
local mutation test also passes. [Rust verification](verification.json) records
53 default, 63 all-feature, 27 native-only and 20 Zarr-only passing tests, plus
native/all-feature Clippy with warnings denied and formatting checks. Tests
requiring external fixtures remain explicitly ignored in ordinary Cargo runs;
the native convenience fixture was run separately.

## Installation and resource evidence

[Deployment evidence](deployment.json) records official image digests, executable
hashes/sizes, dependency tree, `ldd`/Mach-O linkage and clean build logs. A clean
Rust 1.92 slim Linux ARM64 container installs from source in **54.26 s** and builds
the separate consumer in **53.41 s**, using separate empty container-local targets.
No GDAL/PROJ/libtiff packages are installed. An initial host-mounted-target build
failed with a missing proc-macro `quote` rlib; the successful reproducible recipe
keeps targets inside the disposable container. Failed and successful raw logs
are retained locally.

The separate Python-slim runtime imports no geospatial Python package. It asserts
GDAL/PROJ/libtiff absence, executes actual public STAC discovery/planning and the
native consumer, and checks all eight chunk records against the independent
GDAL receipt, including source versions. Peak consumer RSS is **20.4 MiB**; the
plan-plus-acquisition probe takes **7.59 s**. That timing includes a separate CLI
plan and consumer acquisition, so it is not directly comparable with the macOS
consumer-only run. Linux linkage is `libstdc++`, `libgcc_s`, `libm`, `libc` and the
loader. The native macOS consumer links only OS libraries/frameworks.

The live request admits two readers and peaks at **15,891,424 reserved working
bytes** under its explicit 16 MiB budget. This reservation is not RSS: plans,
geometry, metadata, decoder scratch, runtime allocations and caller retention
have distinct scopes. The candidate's eight frozen native replays peak at
**51.8 MiB RSS**, below the separate 192 MiB consumer gate. The new geometry
dependencies slightly increase binary/resident footprint; the original
under-49-MiB result is historical, not a universal new-candidate bound.
Compilation peaks near **1.62 GB**, unrelated to acquisition working memory.

C/C++ compilers, CMake and pkg-config remain needed for existing native-code
Zarr/TLS dependencies. No system geospatial library is required. macOS ARM64 and
Linux ARM64/glibc are tested; Windows, Intel macOS, Linux x86_64, musl and WASM
remain unverified. Cancellation is cooperative; existing bounded transport
timeouts still bound a pending request rather than providing instant interruption.

## Equivalent workloads and remaining request cost

The unchanged frozen three-repeat medians at 8 ms / shared 100 Mbit/s are:

| Workload | Native | Tuned GDAL | Native / GDAL GETs |
| --- | ---: | ---: | ---: |
| many-small | 2.92 s | 2.46 s | 80 / 32 |
| several-large | 4.83 s | 4.85 s | 48 / 30 |
| overlap-a | 4.61 s | 4.90 s | 48 / 30 |
| overlap-b | 4.71 s | 3.38 s | 48 / 18 |

These are equivalent frozen native-bound workloads, not the new polygon workflow's
public-provider performance. The new candidate repeats the full four-workload,
three-reader, two-network-profile diagnostic matrix: **24/24 pass** independent
science/coverage/identity and applicable memory gates, with exact native/baseline
chunk agreement. Current binary hashes and raw paths are in
[verification](verification.json); single-repeat timings remain diagnostic.

[The request investigation](request-profile.md) attributes all 48 native GETs,
checks exact physical byte ranges, observes connection reuse and separates
range wait from decode/assembly estimates. On the six-source fixtures, each
source is opened twice and reads two 4 KiB metadata pages per open. Horizontal
encoded tile wrappers are adjacent, but vertical row gaps are around 13 MB.
An 8 KiB metadata prefix would reduce these fixtures to 36 total GETs without
extra bytes, yet overfetches profiles with only one needed page. Pairing tiles
could save 12 more requests here, but retaining a merged encoded buffer alongside
the decoder/cache/output reservations would require roughly 17.43–17.86 MB
per reader, exceeding the current 16 MiB budget. Native also conservatively admits
one active reader for these four-tile selections while tuned GDAL overlaps more
payload requests. No coalescing or transport change was installed, and exact
accounting/source validation/buffering limits were preserved.

Use the [workflow](workflow.md), [bounded request](request.json) and
[private-trial guide](trial.md) for the next scientist test. Reproduction commands
are in the [protocol](protocol.md). The dgov file-claim plan compiled structurally;
this checkout has no configured dgov dispatch provider, so tasks were executed
through session collaborators without automatic dispatch or merge.
