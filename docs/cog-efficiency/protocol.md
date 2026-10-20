# COG efficiency reproduction and scope

Continue from production-streaming baseline `6a817e2` on
`codex/acquisition-planner`. Use the already verified sparse mirrors under
`prototype-output/production-streaming/real/mirror/` and the original
[16-source manifest](../production-streaming/sources.json). These are original
Sentinel-2 C1 L2A bytes at their original offsets, with pinned ETags and logical
sizes. Every replay verifies captured hashes and rejects uncaptured ranges.
This phase makes no public-provider requests or paid-resource allocations.

The workload windows, native grids, oracle semantics, independent GDAL controls,
link shaping and tuning sweep remain those of the
[production-streaming protocol](../production-streaming/protocol.md).
Each final cell has three fresh-process repetitions, interleaved by a fixed
seed. Both output pairs are retained: native statistics and native chunk TIFFs.
GDAL controls are independently implemented and retuned over 1/2/4/8 asset
workers per workload/output/link. Their pinned conditional reads and
initial/pre-read/final source checks remain enabled. No reader-only control is
substituted into the matched timing table.

The candidate defaults to two data/metadata workers; manual worker/per-origin
settings remain available. Both readers use a 32-MiB decoded GDAL block-cache
setting, one native decode thread, serial HTTP multiranges, disabled directory
probing, 32-KiB opening reads and bounded retries/timeouts. Explicit CLI
GDAL environment settings now take precedence over its defaults. Embedded
callers own process-global cache initialization. The Rust native consumer uses
queue capacity zero, a 16-MiB working budget and a 2-MiB maximum delivered-payload
check, and retains no pixel arrays. The native-process RSS gate is **192 MiB**;
it is measured with `wait4`, not inferred from scheduler reservations. GDAL
control and file-path RSS are reported separately.

Preserve baseline binaries **before rebuilding**, with their commit and SHA-256
receipt, at `prototype-output/cog-efficiency/baseline-binaries/`. The isolated
layout prototype is preserved at `layout-binaries/`: it adds bounded inspection
and caller-local GDAL-option propagation while retaining baseline data
scheduling, execution cache invalidation and worker defaults. Its binary hashes
are in `layout-comparison.json`. The isolated experiment forces the same two-
or four-worker setting on both binaries. It verifies complete native plans and
every provisional chunk descriptor for equality and retains all HEAD/GET/byte
counts. A preliminary trial that overlapped a build remains local and is excluded
from accepted timings. Subsequent safety guards in the final planner serialize
repeated URLs and multiband layouts; the final matrix binds its own binaries.

```sh
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
cargo build --release --locked --bins --examples
CARGO_TARGET_DIR=target cargo build --release --locked --manifest-path consumers/native-stats/Cargo.toml

prototype-output/zarr-env/bin/python bench/cog_layout_comparison.py --label layout-quiet --reps 3
prototype-output/zarr-env/bin/python bench/snapshot_comparison.py --label cog-final-fast --reps 3
prototype-output/zarr-env/bin/python bench/snapshot_comparison.py --label cog-final-latency --reps 3 --latency-ms 8 --bandwidth-mbps 100

prototype-output/zarr-env/bin/python bench/cog_efficiency_summary.py \
  --runs prototype-output/production-streaming/real/cog-final-fast-0ms-0mbit/runs.jsonl \
         prototype-output/production-streaming/real/cog-final-latency-8ms-100mbit/runs.jsonl \
  --output docs/cog-efficiency/summary.json
prototype-output/zarr-env/bin/python bench/cog_io_trace.py \
  --runs prototype-output/production-streaming/real/cog-final-fast-0ms-0mbit/runs.jsonl \
         prototype-output/production-streaming/real/cog-final-latency-8ms-100mbit/runs.jsonl \
  --output docs/cog-efficiency/final-trace-accounting.json \
  --raw-output prototype-output/cog-efficiency/final-trace-full.json
prototype-output/zarr-env/bin/python bench/consumer_validation.py
prototype-output/zarr-env/bin/python bench/gdal_snapshot_checks.py
```

Labels must be fresh; the harness refuses to reuse a run directory. The commands
above record the accepted labels, so change labels for a later reproduction.
Keep builds, allocation observers and other benchmark agents quiet during timing.
Raw requests, range/block intersections, output records, stage traces, failures
and binary/harness/source receipts stay under ignored `prototype-output/`.
Measured pre-formatting harness snapshots are preserved under
`prototype-output/cog-efficiency/harness-snapshots/`; later script formatting,
relative-path handling and final-accounting checks do not change acquisition work.
Compact receipts and scripts are versioned; imagery is not committed.

`cog_io_trace.py` classifies response bytes against TIFF metadata and compressed
block intervals at every image level, including tiny overviews within the opening
range. Repeated interval coverage determines duplicate block bytes: two disjoint
ranges touching one block do not count as a duplicate. The parser is
benchmark-only, independently checked against GDAL tags, and decodes no pixels.
`cog_io_ablation.py` isolates concurrent/serial handles, decoded-cache capacity,
window order and reopening, with exact native oracle comparisons. Its diagnostic
timings are separate from the matched complete acquisition. Memory-counter and
allocation-stack reproduction is in [memory-components.md](memory-components.md).

Native first-chunk time starts inside the external Rust consumer and includes
preparation. GDAL first-chunk time starts after Python imports; process wall time
includes both startups. The first native chunk is provisional. File-path
`first_materialized_seconds` for Rust is relative to scheduler execution and
excludes preparation; GDAL's starts inside its worker before validation. These
are not directly comparable native first-chunk measures.
Layout inspection measures aggregate wall time of bounded inspection batches;
planning-compute time subtracts that wall time from native planning. GDAL's
metadata-worker time sums concurrent worker durations and cannot be interpreted
as preparation wall time. Complete raw stage timings remain available.

The independent rasterio/NumPy oracle checks exact native values/checksums,
counts/sums, nonoverlapping complete asset coverage, dtype, affine/CRS, scale,
offset, nodata, timestamps and variables outside timed acquisition. Native output
provenance ETags are checked against the frozen manifest. Separate external
consumer checks cover cancellation, already-read source mutation, catalog
mutation, final completion, missing UInt16 Zarr data and Float32 bit patterns with
swapped/reversed axes. This work changes no scientific format or public native
streaming API.

Report medians and observed ranges, every failed resource row, GET/HEAD counts,
response-body bytes, duplicate block payload, preparation, first chunk and RSS.
Preserve the original four-worker RSS failures alongside the new default's rows;
missing exact inherited failure metrics remain unavailable. A difference under
10% is a timing tie. Comparisons against the old Tile-Ripper matrix and against
newly tuned GDAL are separate. One workstation, warm local replay, different
linked GDAL versions and different checksum implementations limit attribution;
these results do not establish public-provider throughput or a general speed lead.
Stop after a validated local commit and decision gate. No release, deployment,
paid infrastructure, ChronoZarr integration, new format, persistent data cache or
adaptive controller is authorized or implemented.
