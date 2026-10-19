# Research recovery and what to reuse

The repository initially contained only `study/`, with no Git repository or
Rust implementation. The study and its tracked raw results were preserved in
an initial Git snapshot; prototype work lives in a separate managed worktree.
The installed Python environments and original checkout were left in place.

The supplied research is internally consistent on the main decision: do not
rewrite TIFF, transfer libraries or reprojection to obtain a planner. The
most consequential measured findings are in study/07 and its raw JSON/table
outputs, not the older speculative roadmap. The older architecture's
`ChunkedFormat::span -> ByteSpan` cannot naturally represent a chunk object or
library-only dataset slice; ADR 0001 explicitly replaces that boundary.

Already known; do not repeat the ecosystem census:

- Tuned GDAL was close to the direct manifest executor on one noisy link.
  Rasteret's persisted-index reads made the same requests as the manifest.
  The custom-engine throughput case remains unproved.
- E5 generated manifests over 48,000 replicated assets in about 0.5 seconds.
  This was warm layout-table computation, not 48,000 live object inspections.
- E6's real-pixel parity covered 48 small-window and 24 corner-window assets,
  native grids, one provider, the tested deflate/predictor layout. It did not
  establish generic TIFF codec or reprojection correctness.
- GeoParquet won as a reusable local snapshot, not universally as a live
  discovery route. Large footers, asset columns, ordering, live tails and
  coarse/exact footprint predicates dominated cost.
- lazycogs supplied design ideas, not a Rust engine to fork. The tested
  duckdb-cog version did not expose useful low-level request manifests.

Reuse from the actual experiment code:

| Source | Reuse | Prototype decision |
|---|---|---|
| planwin.py | fixed workload definitions and source-grid bounds | independent offline oracle inputs |
| e4_layout.py | object identity + internal layout are distinct from catalog metadata | GDAL inspection, local identity cache; no copied TIFF scanner |
| e5_planner.py | floor/ceil clipping, half-open windows, tile-index selection | ported native math; SQL rerun retained as comparator |
| e5_planner.py | sorted per-object gap grouping | defer until a transport adapter controls physical reads |
| w_floor.py | minimal range accounting and pixel oracle idea | preserve historical results; do not ship handwritten decoder |
| w_gdal.py | tuned reader path and request instrumentation cautions | native Rust GDAL adapter plus independent rasterio worker |
| e6_pixels.py | frozen inputs, randomized interleaving, per-output equality | reusable local benchmark with server-side exact counters |
| e6b_gdal_accounting.py | threaded debug logs can undercount HTTP | server counts requests, never infer observed counts from tiles |

ChronoZarr was inspected read-only in its current `performance.py`, `mosaic.py`
and ingest entry points. General lessons are resource-aware limits, quality
reads before expensive data reads, bounded admission, strip writes, explicit
failure reporting, cached-URL failure cleanup, and retry cancellation. Its
Sentinel SCL classes, radiometric shifts and monthly compositing are not
Tile-Ripper defaults. Its measured preferred concurrency is environment-bound.
No ChronoZarr files were changed or copied into this package.

Only implementation-relevant upstream facts were refreshed: the Rust GDAL
version compatible with installed GDAL, the GTiff reader surface, Zarr v2
metadata/chunk semantics, HDF5 chunk-information APIs and CMR discovery. Sources
are linked in ADR 0001. Historical tool versions, release projections and
benchmark timings remain explicitly historical; no unverified claim that a
previous limitation persists upstream was needed to choose this prototype.
