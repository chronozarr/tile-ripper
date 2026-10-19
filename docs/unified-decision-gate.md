# Unified acquisition decision gate

**The interface is coherent and scientifically validated within its documented
contracts. Zarr source-read amplification is fixed; end-to-end Zarr performance
regressed because of output writing. Stop here, with that regression explicit.**

1. **Can an external scientist use COG and Zarr through one understandable
   workflow?** Yes: `plan request.json --human`, inspect JSON if needed, then
   `fetch request.json --output new-directory`. The same prepared Rust API
   serves both. Complete [requests and commands](unified-workflow.md) cover a
   real remote Sentinel-2 acquisition and a deterministic local Zarr cube.
   This is a narrow tool requiring Rust/system GDAL, not arbitrary archive
   compatibility. Local Zarr's writing cost makes larger extractions inefficient.
2. **Does the shared planner remain clean?** At the logical boundary, yes.
   Catalog discovery resolves runtime assets before the existing native planner;
   Zarr resolves its actual coordinate axes internally. The same Request,
   source/variable identity, native windows, dependency DAG and fallible owned
   buffers apply to both. Physical `LibraryWindow` versus `ChunkObject` reads,
   coordinate interpretation, metadata and output encodings remain adapter-owned.
   The supported common data model is still UInt16 regular spatial planes,
   with optional nonspatial selections, not arbitrary scientific arrays.
3. **Does Zarr retrieve the minimum necessary chunks?** Yes, once per selected
   data key/asset in all measured cases, including absent chunks. The independent
   coordinate/chunk-grid minimum equals the planned and observed key sets.
   This does not claim minimum metadata/coordinate filesystem activity or
   output-object updates. xarray reads the same existing data objects, handling
   the missing key through a separate existence probe.
4. **Any demonstrated performance improvement?** Source I/O improves:
   110→42 calls and 297,646→109,903 encoded bytes on the small cube;
   1,188→112 calls and 13,503,563→879,821 bytes on the medium cube. **No overall
   speedup is demonstrated.** The new Zarr source order generates partial
   updates to fixed output chunks; writing dominates and total runtime worsens.
   COG request counts/bytes are unchanged from 46dd503. Process startup and
   differing reader/output implementations prevent broad language/speed claims.
5. **Ready for remote Zarr?** The logical lifecycle is a useful foundation;
   the implementation is not ready to ship remote Zarr. Filesystem size/mtime
   checks are not remote object identity or an atomic multi-object snapshot.
   Credential resolution, consistent metadata/coordinate/chunk versions,
   conditional reads, network limits/retries and transport observation remain
   unimplemented. The local writing regression should be resolved first.
6. **Smallest valuable next milestone?** A bounded Zarr output-writer pass:
   instrument output-object updates and compare mature zarrs write/encoding
   options while retaining one source read per selected key, exact outputs and
   buffer limits. Remove the measured writing regression before adding remote
   Zarr or a third format. No new cache, scheduler, mosaic or decoder is justified
   by this gate, and none was implemented here.

## Why the source reads repeated

The previous planner's distinct chunk set was already correct. It planned
output-sized dataset slices, however, so slices crossing native chunk boundaries
revisited the same source keys. The independent reconstruction of those slice
visits exactly matches the old observed call counts. This was not wrong chunk
math, metadata counted as data, prefetching or an implicit source cache.

The new planner enumerates each native chunk once; zarrs retrieves/decodes it,
then the executor emits bounded selected spatial/time planes. Source traces
record key, count and encoded bytes. No handwritten codec, downloader, cache or
scheduler was added. The old root-array planning-only probe remains tested.

The tradeoff is real: source-aligned planes need not align with output chunks.
The pinned zarrs partial-write path reads/updates/re-encodes an output chunk
when the write does not cover it. Measured sink time, not source decoding, now
dominates. Output requests have not been independently counted in this milestone;
that is the next useful instrument, rather than a speculative caching design.

## Correctness and provenance

The shared lifecycle tests cover native values/nodata/radiometry, partial and
empty selections, inclusive dates, unsupported/missing selections, source
mutation (including a newly created missing chunk), plan immutability,
cancellation and cleanup. The common prepared snapshot test also checks two
remote ETag versions against GDAL's URL cache. Scoped native diagnostics restore
the host handler, including on unwinding. Literal local path punctuation stays
intact while URL credentials/queries are removed from inspection/provenance.

The independent xarray/Zarr oracle checks 3,403,320 values in current-reader
benchmark runs, raw and CF-decoded metadata/coordinates/dates, plus seven edge
requests and their physical key sets. Hard termination leaves no schema-3
completion marker. All 120 outputs across the three-reader COG benchmark are
checked through independent rasterio/GDAL operations, preserving both native
CRSs/resolutions, nodata, masks and radiometry.

The real common CLI command acquired B04/B08 from
`S2A_T17SPV_20250728T161108_L2A`. All 20,566 stored samples and spatial metadata
match independent remote GDAL references; each band has 5,004 AOI pixel centers.
See [live checks](unified-live-correctness.json) and
[the common manifest](unified-live-manifest.json). This is one bounded live
correctness check, not a public-provider performance comparison.

Schema-3 manifests share source references, file/variable identity, timestamps,
named output dimensions, native CRS/affine, canonical UInt16 dtype, nodata,
scale/offset, processing, limitations and measured counters. Files remain
format-native: GeoTIFF observations/chunks or Zarr groups with real coordinates.
There is no forced common grid or temporal composite. Inspection JSON is not
replayable; prepared runtime bindings validate local versions or remote ETags.

The measurement tables and variance are in
[the benchmark protocol/results summary](unified-benchmark-protocol.md), with
individual [Zarr runs/key traces](unified-zarr-benchmark.json),
[STAC/GDAL runs](unified-stac-benchmark.json) and
[regression gates](unified-regression.json). Historical evidence is retained.
No dependencies, third format, remote Zarr, paid infrastructure, merge or
publication were added.
