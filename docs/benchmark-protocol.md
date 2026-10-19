# Prototype benchmark protocol (fixed before final measurement)

Purpose: verify selection, raw pixels, georeferencing and truthful execution
accounting. No claim of superiority over tuned GDAL is tested at this gate.

Fixtures: deterministic seed 20261018, one 2048² UInt16 COG, deflate/predictor 2,
256² tiles, EPSG:32617, nodata 0, scale 0.0001 and offset -0.1. Small window
160² at (100,100), larger 1400² at (240,240). Chunks up to 512², same shape for
both tools. Raw values are not calibrated or resampled.

Five seeded interleaved repeats per configuration. Local: Tile-Ripper cold
layout, warm persisted layout, tuned GDAL. HTTP loopback: Tile-Ripper index-free
and tuned GDAL. Total 50 fresh-process executions. Serial readers with
GDAL_NUM_THREADS=1 and 32 MiB block cache avoid nested parallelism. This is a
controlled memory/concurrency baseline, not a global optimal GDAL tuning sweep.

Compare every output pixel using independent rasterio window reads, require
exact CRS/affine/nodata/scale/offset equality, and require every selected pixel
covered exactly once. Any mismatch fails the harness immediately. Store raw
per-run timing and counts, median, min/max and sample standard deviation.
Measure peak worker RSS with wait4; server/harness memory is outside worker
RSS. HTTP bytes count response bodies, excluding TLS/headers; loopback is not
a public-network benchmark. OS page cache and source bytes are warm.

Measure planning, fused read/decode and sink separately where exposed. End-to-
end process wall includes startup and manifest writes. Direct discovery has
zero catalog I/O. Do not invent a separate decode timer for GDAL RasterIO or
remote counters inside the Rust executor. The server is the observer.

Layout index cache hit removes inspection only; GDAL may still open metadata
during execution. Remote persisted layouts are not used. The historical SQL
parity experiment is offline and separate from pixel timing. Historical
Rasteret/direct-executor speed comparisons are not newly reproduced.

No paid services or public imagery requests. Scope stops after this gate.
