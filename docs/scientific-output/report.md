# GDAL-free scientific output decision gate — 20 October 2026

The scoped scientific-output gate passes. A scientist can use the ordinary CLI
in a build without GDAL, request Sentinel-2 by WGS84 polygon/date/band, save actual
native UInt16 values, and open the result with independent Zarr/xarray readers.
The implementation is local on `codex/acquisition-planner`, continuing `e9b5de5`.
No publishing, merging, deployment or ChronoZarr integration is involved.

## 1. Format and writer

Selected **Zarr v2 through the existing pinned `zarrs 0.22.10`** dependency, using
its standard array/group metadata, uncompressed chunk encoding, filesystem store,
and existing timing/performance adapters. No output format or decoder was invented.

The audit found reusable source-version checks, the public provisional chunk
stream and its final `finish()` receipt, typed native buffers, affine/window
metadata, and the zarrs storage/codec boundary. The old direct-Zarr writer's
source-metadata copying and time/y/x output buffer assume supported Zarr source
datasets; they cannot safely reinterpret arbitrary COG observations. The new
adapter uses the same writer and metrics but consumes COG deliveries directly.
Its destination grid is anchored to each selected native window, so ordinary
deliveries align with destination chunks and need no scene assembly cache.

| Candidate | Audit and decision |
| --- | --- |
| Existing zarrs writer, Zarr v2 | Already pinned and used by the project; independent Zarr-Python 2, Zarr-Python 3 and xarray interoperability demonstrated. Selected. |
| zarrs writer, Zarr v3 | The maintained library supports v3 as well as a compatible v2 subset. V3 is a credible future destination, especially for evaluating sharding, but it adds a separate metadata/fill/reader validation matrix without solving heterogeneous-grid alignment automatically. Deferred. |
| Other Rust implementations | `zarr3-rs` identifies itself as a prototype and has no published releases; it provides no established replacement advantage here. zarrs ecosystem storage adapters/wrappers do not require replacing the existing writer for this local-directory task. |

This choice reflects reuse and demonstrated interoperability, not an assertion
that writing v3 is intrinsically too complex. Upstream references checked during
the audit: [zarrs](https://github.com/zarrs/zarrs),
[zarr3-rs](https://github.com/clbarnes/zarr3-rs),
[Zarr v3 core specification](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html),
[CF grid mappings](https://github.com/cf-convention/cf-conventions/blob/main/appf.adoc),
[pyproj CF conversion](https://pyproj4.github.io/pyproj/stable/api/crs/crs.html).

## 2. Ordinary fetch

`fetch ... --gdal-free --output ./sentinel.zarr` now selects scientific Zarr for
an ordinary COG/STAC directory request. A JSON `zarr_directory` sink or
`--output-format zarr_v2` selects it explicitly, including in GDAL builds.
Existing direct-Zarr output and default GDAL GeoTIFF workflows retain their
previous layouts. Unsupported native raster/geometry profiles still fail.

The [workflow](workflow.md) includes installation, a complete GeoJSON/direct-flag
example, reader code, layout, masks and completion rules. Python and Rust consumer
code are unnecessary to acquire and save the dataset.

## 3. Scientific metadata and independent values

The real public request produced **six observations on 2 and 12 January 2025,
for B04, B08 and B11**, including native 10 m and 20 m grids. Every one of the
**46,376 stored samples** and both masks was independently compared against
rasterio/GDAL source windows and source-grid AOI rasterization. CRS, affine,
pixel centers, exact acquisition times, band/asset identities, TIFF/STAC
calibration and pinned source versions passed. [Real receipt](live.json).

Separate controlled observations exercise two UTM zones, two resolutions,
different extents/dimensions, partial source coverage, polygon holes, source
nodata, and microsecond timestamps arriving out of chronological order.
All **60,128 output samples** and masks agree with an independent reference.
Groups are ordered chronologically while retaining independent native grids.
Named WGS84 datum fields let independent `pyproj.CRS.from_cf` reconstruct the
source EPSG CRS. [Controlled receipt](validation.json).

The observation-group layout avoids an incorrectly aligned cube. The file dtype
stays UInt16; calibration is metadata. `aoi_mask` preserves AOI inclusion separately
from `valid`, which also excludes source nodata. STAC calibration supplies absent
TIFF calibration fields, with both original records retained in provenance.

## 4. Bounded incremental execution

The writer consumes the existing rendezvous `ChunkStream`, writes values and two
byte masks per delivery, and streams 1D coordinates in bounded slices. There are
no intermediate rasters or scene-sized pixel arrays. Full scene dimensions may
describe a source grid, but no full source pixel array is allocated by the writer.

Output payload admission reserves at most one quarter of `buffer_bytes` for a
UInt16 destination chunk; explicit writer buffers and output-object bookkeeping
are independently bounded. The final empty-store admission check also requires
room for five root/observations/manifest metadata objects, before staging begins.
Scheduler reservations, explicit writer buffers and measured process RSS are
different quantities; this is not an allocator-enforced RSS sandbox.

Across the captured performance matrix, native acquisition reservations were at
most **11,932,307 bytes**, explicit writer buffers at most **524,288 bytes**, and
native-Zarr process RSS at most **55.9 MiB**, with a 16-MiB acquisition budget and
two workers. Source plans/metadata, runtime and library scratch are additional.
The final small-budget preflight tightening is separately tested; performance
capture hashes remain in the benchmark receipt.

## 5. Failure and publication

Each attempt owns a unique hidden sibling staging directory. Only successful
stream `finish()`, complete delivery coverage, finalized group metadata, a complete
manifest, synchronized files and staged directory entries precede publication.
macOS `renamex_np(RENAME_EXCL)` and Linux `renameat2(RENAME_NOREPLACE)` refuse an
existing destination atomically. There is no implicit replacement mode.

Controlled checks pass for source retrieval failure, a single source's final
ETag mutation **after pixel writes**, SIGINT after pixel writes, SIGKILL with
incomplete staging, a successful retry beside that interrupted sibling, and empty
selection. Normal failures/cancellation remove owned staging; SIGKILL leaves a
hidden uncommitted sibling. Rust checks also exercise a destination appearing
just before publication and preservation of unrelated interrupted staging.

Source validation is the existing per-object ETag/local size-and-mtime contract,
not an atomic multi-object snapshot. Parent-directory synchronization after rename
is best effort. Power-loss name durability, network filesystems and interrupted
write resumption are not certified. [Completion contract](workflow.md#incremental-execution-and-completion).

## 6. Performance and deployment

The **48 independently checked matrix runs** use four original Sentinel COG
assets on two dates, frozen captured source bytes, fresh child processes, three
shuffled repetitions per cell, and either warm loopback or a simulated
8-ms/100-Mbit/s link. Timings cover planning, acquisition, output finalization
and process completion; source capture and reference verification are excluded.
This matrix uses explicit assets, so STAC discovery is outside these timings.
The separate live CLI receipt includes real catalog discovery.

Median wall seconds:

| Workload / link | Native stream | Native → Zarr | GDAL → same Zarr sink | Existing GDAL → GeoTIFF |
| --- | ---: | ---: | ---: | ---: |
| Four 96×96 windows / loopback | 0.055 | 0.485 | 0.692 | 0.208 |
| Four 96×96 windows / shaped link | 0.715 | 1.095 | 1.261 | 0.841 |
| Four 2048×2048 windows / loopback | 0.237 | 4.652 | 6.947 | 0.446 |
| Four 2048×2048 windows / shaped link | 3.065 | 7.393 | 8.761 | 4.352 |

Native Zarr has lower medians than the same Zarr sink fed by GDAL in these cells.
Durable output is substantially slower than a stream that writes no scientific
dataset. The existing GeoTIFF path also does less work: it does not emit the
separate coordinate/validity/AOI arrays and has a different flush/durability
policy. These are scenario results, not general performance leadership or a
public-provider throughput claim. The earlier overlapping build/benchmark pilot
was stopped and excluded from the final matrix.

| Backend / selection | HTTP requests | Response-body bytes | Dataset files | Output bytes (approx.) | Observed output+manifest write calls |
| --- | ---: | ---: | ---: | ---: | ---: |
| Native → Zarr / small | 32 | 6,378,559 | 92 | 334,970 | 94 |
| GDAL → Zarr / small | 24 | 6,553,600 | 92 | 331,525 | 94 |
| Native → Zarr / large | 44 | 25,134,367 | 904 | 67,729,630 | 906 |
| GDAL → Zarr / large | 37 | 37,961,728 | 904 | 67,730,450 | 906 |

Counts are stable across link conditions. Output byte sizes vary slightly with
serialized timings. GeoTIFF produces 5 files/small run and 257 files/large run;
the native stream produces no dataset files. GDAL makes one raster API write per
delivered chunk (4/small and 256/large). Buffered GDAL raster writes bypass the
existing syscall interposer, so their total kernel-write count is **unavailable**;
the two observed manifest writes are not misreported as all raster I/O. Zarr
counts are library/kernel-facing operations, not physical device IOPS.

Every run's wall time, CPU, peak RSS, requests/bytes, file sizes/counts, output
store operations, observed syscall coverage and working reservations are in
[benchmark.json](benchmark.json). Raw output and timings stay in ignored
`prototype-output/scientific-output/`. File synchronization dominates much of the
new durable-output cost; compression, consolidation or v3 sharding have not been
benchmarked here.

A clean Linux ARM64 source installation used Rust 1.92, an empty build target,
the registry source cache and C/C++/CMake/pkg-config tools. No GDAL, PROJ or libtiff
was installed or linked. A separate slim Python runtime with none of those
libraries created the real six-observation dataset through the CLI and reopened
it with **Zarr-Python 3.1.3 and xarray 2025.6.1**. All values/masks and scientific
metadata matched the independent GDAL reference. [Deployment receipt](deployment.json).

An initial Python 3.13 ARM64 reader install with the old Numcodecs 0.13.1 pin
failed because a source build needed a compiler. The successful clean reader
uses Numcodecs 0.16.3 wheels and Zarr-Python 3; the independent macOS reference
uses Python 3.10, Zarr-Python 2.18.3 and Numcodecs 0.13.1. This packaging difference
does not change the Zarr v2 destination format or add acquisition dependencies.

## 7. Remaining usability and format limits

Output is an unconsolidated, uncompressed local Zarr v2 hierarchy. Users open an
observation group explicitly, then choose any cross-observation alignment in
analysis software. Raw masks add storage and raw chunks create many synchronized
files; tiny selections can have more metadata than sample bytes. No aligned cube,
automatic calibration, output compression control, object-store publication or
resume mode is provided. Full GeoZarr/CF conformance is not claimed.

COG scientific output currently accepts UInt16 unrotated native grids. Automatic
CF projection fields cover WGS84 UTM and EPSG:4326; other admitted direct CRS
strings remain explicit without claiming complete generic CRS interoperability.
Timestamp encoding uses Int64 nanoseconds and rejects out-of-range dated assets.
Native source/profile restrictions remain unchanged. Direct-Zarr source output
retains its existing separate adapter and compatibility behavior.

macOS ARM64 and Linux ARM64/glibc are exercised; atomic publication currently
targets those OS families. There is no independent human usability trial or broad
platform certification in this work. Licensing/release preparation is unchanged.

## 8. Before broader format support

Extend source/output adapters only with explicit dtype, nodata, calibration,
coordinate and version/completion contracts, plus independent value/metadata and
failure oracles. General grids need a deliberate interoperability policy, not
silent alignment. A future v3 destination should test fill and dimension-name
semantics, independent readers, sharding/write amplification and the same atomic
completion policy. General reprojection, HDF5/NetCDF readers and ChronoZarr
integration remain outside this change.

Verification and reproduction: [protocol](protocol.md),
[Rust/build checks](verification.json), [native COG/Zarr/streaming regressions](regressions.json).

Regression replay passes 61 native COG profile/fault cases, bit-for-bit comparison
of 32,391,168 real-source COG samples, and seven public-consumer checks including
UInt16/Float32 Zarr, reversed/swapped axes, missingness, cancellation and late
mutation. The final Rust/build receipt records active-feature test counts,
explicitly ignored external-fixture tests, strict lint and Zarr-only compilation.
