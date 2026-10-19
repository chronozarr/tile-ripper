# ADR 0003 — One prepared acquisition lifecycle

Continue from 46dd503. Preserve the native logical planner and mature readers.
STAC is discovery, not another format. A common Request resolves catalog or
direct sources into runtime bindings and an existing native plan. The common
`acquisition::Prepared` owns those bindings, version checks, explanation,
callback execution and native writing. Existing reader/facade entry points
remain compatible. CLI aliases route through the same lifecycle.

Inspection JSON and result provenance use explicit version/type/completion
markers. Inspection is never deserialized into an executable job. A prepared
runtime executes the exact in-memory plan; source checks remain mandatory.
Local metadata/coordinates and selected keys are guarded with size/mtime;
remote COGs retain conditional ETag/GDAL cache rebinding. Local checks are not
an atomic transaction, and no persistent execution-plan replay is introduced.

Source discovery time and Zarr's internal time axis remain different concerns.
Named STAC bands resolve to native asset/band identities, while Zarr dataset
paths/time coordinates remain adapter-owned. Budgets own pixel/DAG limits;
provider discovery/I/O caps are separate source configuration. Unsupported
semantics fail, rather than being dropped during STAC request conversion.

The Zarr repeated-read deficiency was physical planning against output-sized
windows. Compile each intersecting native chunk once into the existing
ChunkObject read variant, let zarrs decode the chunk, then fan out bounded
native planes. This uses no cache or scheduler and keeps missing-key fill
semantics in zarrs. Exact planned/observed key sets are measured independently
against coordinates/chunk shapes and xarray reads. Output buffer boundaries
may still cause repeated output-object updates; they are not source reads.

One provenance envelope is shared; physical files, coordinate representation,
format-specific capabilities and grid metadata stay native. Direct COG's
legacy output granularity is retained. Neither a common raster grid nor a new
mosaic/codec/storage engine is necessary for this milestone.


Native GDAL errors are suppressed with a scoped, thread-local CPL handler and
restored on unwind rather than imposing a global handler on library hosts.
Only sanitized errors/references enter the shared inspection/result interface.
The benchmark gate explicitly records the Zarr output-write regression; source
read minimization is not treated as an end-to-end speedup.
