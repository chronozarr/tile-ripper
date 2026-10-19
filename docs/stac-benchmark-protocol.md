# STAC milestone measurement protocol

This is a correctness/integration benchmark, not evidence for a new transfer
engine. All repeated tests run on loopback HTTP, serially. No paid resources,
public concurrency tests, new decoder or second-format executor.

The deterministic fixture has two adjoining scene footprints, UInt16 COG bands
at 10 and 20 m, separate EPSG:32617/EPSG:32618 grids, nodata/partial coverage,
and a bbox-only false positive. STAC serves one item per page, forcing three
pages. Small and larger AOIs cross the scene boundary and include a polygon
hole. The exact item selection set for the independent baseline is frozen
from the fixture contract; pagination and HTTP parsing are still executed.

For each AOI, interleave Tile-Ripper and a tuned Python STAC + rasterio/GDAL
worker five times with a fixed shuffle seed. Every run is a fresh process.
Both readers use serial GDAL, a requested 32 MiB block cache, 32 KiB initial
metadata ingestion, bounded retries/timeouts, conditional object reads,
chunked native masking/writing, and tiled deflate/predictor-2 outputs. Record
process wall and wait4 maximum worker RSS. GDAL versions are recorded and may
differ. OS/source page caches are warm; no metadata-warm speed claim.

Check every output cell against a separate local GDAL source-window read and
independently transformed/rasterized AOI. Require exact raw samples, affine,
CRS, nodata, scale/offset, and both native resolutions/CRSs. The controlled
matrix has 20 runs × four source assets = 80 independently checked outputs.
All successful normal GETs must be conditional; mutation and failed-read
cases run separately from timing. Empty queries, no selected pixel centers,
cap rejection, signed-reference stripping and incomplete-output cleanup are
also mandatory integration gates.

The server counts STAC requests/response bodies, asset HEADs, ranged/whole
GETs, raster response bodies, conditional GETs and injected failures. These
counts are observations. Tile counts and payload estimates are not substituted
for observations. The server's response-write time is diagnostic, not a
measurement of complete client transfer latency. HTTP headers/TLS are outside
body-byte counters. Loopback measurements do not transfer to cloud bandwidth.

Stages reported where observable: discovery, planning (including metadata and
mask checks), execution dataset opens, fused RasterIO read/decode, masking,
sink writes and final flush. RasterIO does not expose isolated production
network/decode timers. Instead, **separate, non-additive probes** measure
transfer-only selected compressed tile ranges, and GDAL decode/window assembly
from an in-memory COG with source-byte load and metadata-open excluded. Five
probes per AOI are recorded. They are not presented as an exact decomposition
of end-to-end production time.

The live demonstration is one small public acquisition, B04/B08. Record wall,
RSS and internal stage timers; independently compare output pixels/georeferencing
using remote rasterio/GDAL reference reads. Live request/body-byte counters
remain unavailable rather than guessed. Do not compare the reference-check
wall time against end-to-end acquisition wall as a speed ratio.

Report median, range and sample standard deviation for controlled repetitions.
No throughput speedup claim: Python startup/imports, reader versions, index
inspection, version revalidation and manifest work differ. Source/lock/fixture
hashes identify the tested implementation and inputs. Negative tradeoffs,
including repeated metadata opens for safe prepared-plan version binding,
belong in the report.
