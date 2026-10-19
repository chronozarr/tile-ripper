# Decision gate — keep the planner narrow

The proof supports a shared logical acquisition plan and a native COG
extraction utility. It does not justify a new transfer/decoder engine or a
claim of superiority over tuned GDAL. Stop at this gate.

Delivered: ADR and capability matrix; typed Rust request/DAG/capability model;
GDAL-backed COG planning and native UInt16 execution; contrasting Zarr v2
chunk-object planning; CLI plan/fetch; bounded pixel callbacks and GeoTIFF
chunks; a repeatable correctness and benchmark harness. One package is enough.

## Explicit answers

1. **Is a shared abstraction feasible?** Yes at the logical dependency/read
   boundary. COG `LibraryWindow` and Zarr `ChunkObject` both fit the actual
   compiled DAG. This is evidence for two regular-raster cases, not proof for
   irregular SWOT observations, all HDF5 layouts or feature sources.
2. **What stays format-specific?** Internal layout, coordinate lookup,
   overview/chunk selection, sparse/fill semantics, codecs and filters,
   physical reads and safe fallbacks. HDF5 may remain a library dataset slice;
   GeoParquet may remain feature predicates/row groups.
3. **What does DuckDB contribute?** Large indexed catalog/layout queries,
   pruning and batched SQL selection. The original E5 SQL is rerun as an
   independent tile-manifest oracle. It adds no necessary capability to a
   small direct-assets request, so the Rust package does not embed it.
4. **Which libraries execute reads?** GDAL GTiff now, preserving an established
   native-window fallback. Future measured adapters can use async-tiff plus
   object_store for controlled COG reads, zarrs for supported Zarr codecs and
   shards, HDF5/netCDF library slices, and DuckDB/Arrow for feature tables.
   Those candidates have not been integrated or certified by this prototype.
5. **Useful independently of ChronoZarr?** Yes: Rust callers receive native
   georeferenced buffers with asset/time/band identity, and CLI users obtain
   separate GeoTIFF chunks plus a provenance plan. No ChronoZarr dependency.
6. **An opportunity existing tools miss?** A small Rust request→explainable
   cross-format plan→fallible sink API is a plausible integration opportunity.
   No unique speed opportunity is demonstrated. Existing readers supply the
   data path; Rasteret already covers much of indexed COG acquisition.
7. **Smallest useful next milestone?** Support one public STAC collection end
   to end: polygon/date/bands → native extracts, with explicit polygon masks,
   runtime authentication, remote version/ETag cache validation, cancellation,
   and a two-session comparison against tuned readers. Keep native per-asset
   outputs; defer mosaics, composites and additional format executors.

## Verification and scope

- Eight Rust integration tests cover native raw values, georeferencing,
  nodata/scale/offset, clipping, time selection, cache reuse/invalidation,
  unsupported selections, budget rejection, sink failure, output overwrite
  protection and Zarr planning/execution preflight.
- `cargo test --locked`, `cargo fmt --check`, strict all-target Clippy, and
  Python Ruff checks/formatting pass.
- The original E5 SQL is rerun without network access; its ranges reproduce
  the saved manifests. Rust matches all **96 asset/window cases**, including
  tile indices, offsets, byte counts and pixel windows. These are plan checks,
  not 96 newly decoded real scenes. Raw result: study-parity.json.
- The final local benchmark has **50/50 exact pixel/metadata/coverage checks**
  across small and larger subsets, five interleaved fresh-process repeats per
  configuration, local cold/warm layout cache and index-free HTTP. Raw data:
  benchmark-results.json and benchmark-runs.jsonl; protocol: benchmark-protocol.md.
- The CLI demonstration generated both plan variants and a real GeoTIFF output.
  Zarr fetch correctly failed before creating output. Saved demo plans and
  output manifests contain opaque IDs, not runtime source locators.

Only a single machine and loopback HTTP were benchmarked. OS caches stayed
warm. GDAL versions differ between Rust and rasterio; process startup/imports
and manifest overhead differ. There was no public imagery performance test,
concurrency sweep, real SWOT/NISAR run or strict process-RSS budget proof.
GDAL fuses retrieval and decode, so those timers are combined. Direct input
assets have no catalog discovery phase. Rasteret and the historical direct
range executor were not rerun on this synthetic fixture. A Zarr decoding
comparator is premature because its adapter only plans keys. These limitations
prevent any throughput superiority claim.

`buffer_bytes` is a per-chunk pixel-buffer limit, not process RSS. Layout
metadata has a separate 65,536-record cap; persisted caches larger than 32 MiB
are rejected. The callback can retain buffers, outside executor control.
Remote metadata is re-inspected rather than unsafely persisted. Conditional
remote reads and hard RSS admission remain next-milestone work.

## Observed results

Both readers made identical HTTP requests and transferred identical response
bodies: small window **1 HEAD + 3 GETs / 554,293 bytes**; large window
**1 HEAD + 22 GETs / 6,429,366 bytes**. No I/O reduction was observed.

| Subset | Transport | Configuration | Wall median [min–max], s | SD, s | Peak RSS, MiB |
|---|---|---|---|---|---|
| large | http | gdal-tuned | 0.2814 [0.2754–0.2824] | 0.0035 | 70.4 |
| large | http | tile-ripper-cold | 0.2178 [0.2175–0.2188] | 0.0005 | 54.0 |
| large | local | gdal-tuned | 0.2300 [0.2234–0.2562] | 0.0128 | 64.7 |
| large | local | tile-ripper-cold | 0.2019 [0.1987–0.2084] | 0.0038 | 52.5 |
| large | local | tile-ripper-warm | 0.1956 [0.1929–0.1971] | 0.0016 | 50.2 |
| small | http | gdal-tuned | 0.2588 [0.2280–0.2780] | 0.0180 | 63.6 |
| small | http | tile-ripper-cold | 0.1941 [0.1792–0.2052] | 0.0093 | 48.8 |
| small | local | gdal-tuned | 0.1888 [0.1863–0.2062] | 0.0081 | 56.7 |
| small | local | tile-ripper-cold | 0.1757 [0.1683–0.1796] | 0.0041 | 46.2 |
| small | local | tile-ripper-warm | 0.1666 [0.1645–0.1694] | 0.0018 | 44.1 |

Process timings include unequal startup/import and manifest overhead. These
are observed measurements, not evidence that the Rust reader is faster than
GDAL. Both paths execute through GDAL. Warm metadata saves inspection but
does not remove GDAL execution opens. Current performance claims remain
unchanged from the earlier study: engine headroom is unproved.
