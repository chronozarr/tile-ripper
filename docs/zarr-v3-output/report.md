# Experimental Zarr v3 output — 20 October 2026

All five explicit native-COG output profiles pass independent scientific
validation, the real-source matrix, publication faults and bounded-memory
checks with zarrs 0.23.14. Normal fetch continues to write uncompressed Zarr v2.
Experimental v3 requires the existing build feature and an explicit encoding.

The profiles are ordinary raw v3 and 2×2 sharded raw, gzip level 1, Blosc/Zstd
level 1 with byte shuffle, and Zstd level 1 with frame checksum. Inner spatial
chunks use the selected output edge; outer shards contain 2×2 chunks. Shard
indices now use standard little-endian bytes plus CRC32C at the end. This is an
explicit experimental metadata change; the preceding dependency gate retained
and tested the old bytes-only configuration for the original comparison.
There is no replacement encoder or scene/shard cache in the application.

The [90-cell matrix](benchmark.json) repeats 96/512/2048-square windows on four
original Sentinel B04/B08 observations, three repetitions on each of warm local
HTTP and 8 ms/100 Mbit/s HTTP. Every cell completes and is reopened with independent
Zarr-Python 3.1.3/xarray 2025.6.1 against GDAL source references. This includes
the previously failing medium/large cases for every new encoding.

[Controlled checks](candidates.json) cover exact values, both masks, native
coordinates/grids, fill, nanosecond timestamps, ordering and provenance, plus
late source mutation, failed source reads, empty selections and small-budget
admission. The existing application/publication suite runs 31 cases per
encoding, **155 total**, including EIO, SIGINT, SIGKILL and a destination race.
Each corresponding `faults-*/faults.json` receipt preserves the outcome. All
pre-commit failures with ordinary cleanup leave no published destination;
post-commit failures preserve a complete independently readable dataset and
report unconfirmed durability. These are not power-loss tests.

The [memory gate](memory.json) increases logical value+mask volume 64-fold.
Largest peaks are 68.7 MiB ordinary raw, 70.5 sharded raw, 70.3 gzip,
69.6 Blosc/Zstd and 71.3 Zstd. All pass the 192 MiB peak and under-64-MiB growth
screens. The declared per-call writer reservations at edge 256 are 2.375 MiB
ordinary raw, 8.375 MiB sharded raw and 9.375 MiB compressed shards, separate
from source scheduler reservations and process RSS.

Fourteen Rust acquisition tests pass with all features, including all new
encodings at unequal source/output edges. Strict all-feature clippy, Zarr-only
compilation, Python syntax/lint and formatting checks pass. The initial
native-only acquisition test command selected zero tests because that fixture
module requires GDAL; the all-feature run above supplies the actual coverage.
[Verification and measured large-window medians](verification.json).

The tested profiles are suitable for further experimental work. Shard rewriting
can cost more CPU and destination I/O than the optimized v2 default, and these
results do not justify changing that default. No Float32 COG source profile,
v3 input, release or deployment is introduced by this output milestone.

Reproduction after building the explicit candidate example:

```sh
prototype-output/durable-output/reader3/bin/python bench/zarr_v3_output.py checks
prototype-output/durable-output/reader3/bin/python bench/zarr_v3_output.py faults
prototype-output/durable-output/reader3/bin/python bench/zarr_v3_output.py memory
prototype-output/durable-output/reader3/bin/python bench/zarr_v3_output.py matrix
```

Use fresh work directories; run timing separately from builds/other gates.
Generated datasets and raw logs remain under ignored prototype-output.
