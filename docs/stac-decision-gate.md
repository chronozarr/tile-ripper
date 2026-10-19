# STAC milestone decision gate

**Outcome: a scientist can use this narrow tool today without custom Python.**
The checked workflow is public Earth Search Sentinel-2 C1, AOI/date/band search
through native masked UInt16 GeoTIFFs and a provenance manifest. Stop here;
no second format executor, mosaicking engine, publication or merge.

## The five decisions

1. **Can a scientist retrieve a useful subset today?** Yes. The copyable
   command in `stac-workflow.md` acquires B04/B08 for a small real polygon on
   2025-07-28. It returns two native GeoTIFFs and a manifest, using the same
   Rust discovery/planning/execution code as the library example. Python is
   needed only for independent verification/benchmark development.
2. **Advantages over tuned STAC + GDAL?** A packaged request→inspectable native
   plan→per-asset output workflow, explicit AOI conventions, bounded jobs,
   complete-within-cap pagination, conditional version checks, and provenance.
   Existing readers remain responsible for pixels. These are integration and
   correctness conveniences, not a demonstrated new throughput advantage.
   The current safe prepared-plan implementation actually performs **more
   metadata I/O** than the one-pass tuned comparator.
3. **Remaining limits?** One provider/collection; public HTTPS COG spectral
   assets; UInt16; explicit representable nodata; bounded non-wrapping polygon
   AOIs within ±80° and 5° per axis. No cloud masking, calibration, mosaics,
   composites, target-grid resampling, other authentication adapters,
   persistent remote layout cache, asynchronous execution, resume or hard RSS/
   wire-byte guarantee. Geometry edges use the documented densification and
   GDAL pixel convention. Hosting applications must coordinate GDAL global
   caches and same-URL activity. Source/version or read failures stop the job.
4. **Does the planner architecture hold up?** Yes. STAC discovery prepares
   runtime asset bindings and source provenance independently of the COG
   reader. Native windows remain `LibraryWindow` reads in the same DAG.
   Polygon masks add source-grid geometry to a selected asset; the sink maps
   chunk writes to one file per asset. No byte-range-only abstraction or
   mandatory uniform time/grid was introduced. One concrete deficiency was
   GDAL's URL-only cache across prepared versions; cache rebinding and a
   same-process snapshot test now protect that boundary. Zarr planning and
   the original tile-selection oracle remain intact.
5. **Second fully supported format: Zarr or HDF5/NetCDF?** **Zarr**, starting
   with one documented regular-grid, unsharded array convention, explicit
   coordinates/fill values, and existing-reader decoding. The planning-only
   chunk-object adapter already tests the contrasting read primitive. HDF5/
   NetCDF remains valuable, but SWOT/NISAR require dataset-specific variables,
   coordinate selections, filters and sometimes irregular geolocation; that
   is a larger scientific adapter task. This recommendation is not approval
   to implement it now. First harden this STAC workflow with an outside user.

## Evidence

The final source/lock hashes and environment are recorded in
`stac-benchmark-results.json`. It contains 20 interleaved fresh-process runs,
80 independently checked asset outputs, 10 separate transfer/decode probes,
and exact HTTP observations from a local server. Both 10/20 m resolutions and
EPSG:32617/32618 grids are preserved. Fixture data include nodata/partial
coverage and polygon holes across adjoining scene footprints.

Integration gates passed: three-page discovery; exact footprint rejection of
a bbox-only false positive; discovery-only with zero raster I/O; empty query
plan/fetch; footprint overlap without any selected pixel centers; no silent
item truncation; output-budget preflight; changed-object rejection; failed-
read cleanup; credentials stripped from saved references; and an older
prepared plan refusing another snapshot's GDAL cache. The last case runs an
otherwise ignored Rust fixture test through the harness.

`cargo test --locked` passes 11 ordinary tests (including the original Zarr
case); the HTTP harness additionally runs the 12th snapshot test. Strict
all-target Clippy, Rust formatting and Python Ruff checks pass. The original
SQL parity check still matches all 96 asset/window cases. These plan checks
are separate from the new pixel comparisons.

The final live acquisition selected item
`S2A_T17SPV_20250728T161108_L2A` and B04/B08. Two 91 × 113 native extracts
contain 20,566 stored pixels; 5,004 centers per band are inside the polygon.
All stored samples and spatial/nodata/scale/offset metadata match independent
remote rasterio/GDAL reference operations exactly. Details and hashes:
`stac-live-correctness.json`; manifest: `stac-live-manifest.json`; stage/wall/RSS
measurements: `stac-live-run.json`. The Rust library example was also executed
against the real catalog; its diagnostic result is `stac-library-plan.json`.

## How to interpret performance

Use `stac-benchmark-protocol.md` for stage definitions. Repetitions are local
loopback HTTP on one workstation with warm OS cache, not a cloud-near-data or
multi-day public throughput experiment. The comparator uses tuned GDAL and
the fixture's independently frozen exact item selection set. Reader versions
differ (Rust host GDAL 3.12.4, reference rasterio GDAL 3.12.2), and Python
startup/imports differ from the Rust executable.

Reported production read/decode time is fused RasterIO time. Separate probes
measure transfer-only tile ranges and in-memory GDAL decode/window assembly;
these are **not additive pipeline components**. Live HTTP bytes/request counts
are unavailable and remain null. The live reference-check duration is not an
end-to-end comparator. No speedup claim follows from any wall-time ratio.

GDAL URL-cache clearing ensures version binding even when another prepared
job has read a different object version. This costs an execution metadata
reopen. A future optimization could retain identity-bound reader handles;
it should preserve this safety test and be measured before adoption. The
current implementation accepts the metadata overhead explicitly.

## Controlled observations (five repeats per cell)

| AOI | Reader | Wall median [min–max], s | SD, s | Peak RSS, MiB | Raster HEAD / GET | Raster body bytes |
|---|---|---|---|---|---|---|
| large | gdal-stac-tuned | 0.2851 [0.2825–0.2855] | 0.0012 | 75.8 | 8 / 13 | 556,108 |
| large | tile-ripper | 0.2047 [0.2039–0.2062] | 0.0009 | 53.4 | 12 / 17 | 687,180 |
| small | gdal-stac-tuned | 0.2758 [0.2747–0.2847] | 0.0042 | 74.9 | 8 / 12 | 327,142 |
| small | tile-ripper | 0.2003 [0.1983–0.2016] | 0.0013 | 52.4 | 12 / 16 | 458,214 |

Every successful run made three STAC requests; all raster GETs were ranged
and conditional. The extra Tile-Ripper metadata requests/bytes are a real
tradeoff. The lower Rust process wall/RSS includes runtime/import differences
and is not evidence of a faster GDAL data path.

| AOI | Transfer-only probe median, s | In-memory decode/assembly median, s | Probe range requests | Probe payload bytes |
|---|---|---|---|---|
| small | 0.00493 | 0.00187 | 8 | 195,978 |
| large | 0.00631 | 0.00222 | 10 | 261,271 |

The live example took 5.38 s wall with 68.2 MiB peak worker RSS. Discovery was 0.547 s; planning/identity/metadata 1.509 s; execution open 0.490 s; fused read/decode 2.661 s; masking 0.000186 s; sink callbacks 0.0043 s. Startup, flush and manifest work are separately present in wall time. This is a single live observation, not a performance claim.

## Next action

Have an outside scientist run the documented request on their AOI, inspect
the manifest and independently verify one native output. Resolve packaging,
provider errors and any boundary ambiguity before widening formats or
investing in scheduling. Do not build a second acquisition engine.
