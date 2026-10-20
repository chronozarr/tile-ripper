# COG process memory and the constrained default

Use **two acquisition workers** for the tested 192-MiB native-consumer profile,
with a 32-MiB GDAL block cache, one GDAL decode thread, a 16-MiB working buffer,
rendezvous delivery, maximum delivered payload of 2 MiB, and no caller-retained
pixel arrays. These settings describe a tested profile, not an allocator-enforced
RSS ceiling. Explicit worker, working-buffer and GDAL-cache settings remain
available. The final matched matrix is reported separately; the diagnostics
below are not replacements for its failed or passing resource runs.

## Evidence and measurement

[The compact receipt](memory-components.json) retains every accepted memory
observation, both consumer binary hashes, mirror identity, inherited resource
failures and the relevant allocation-stack excerpts. All source bytes come from
the original frozen Sentinel-2 sparse mirrors. No provider requests or paid
services are involved.

`bench/cog_memory_profile.py` launches the independent public-API statistics
consumer in fresh processes. On macOS an optional small observer samples
`task_info` resident memory/physical footprint, `malloc_zone_statistics` live and
reserved heap, and GDAL's public `GDALGetCacheUsed64`/`GDALGetCacheMax64` counters
every 10 ms. Fresh-process `wait4` peak RSS remains the gate measurement; samples
can miss short peaks. These explicit-assets trials set cache size in the process
environment before launch. The observer initializes GDAL's cache-size getter
early, so these runs are diagnostic and excluded from timing rankings.

The baseline is the preserved production-streaming consumer at `6a817e2`.
The candidate contains asset-selection reader reuse, bounded parallel inspection
and the two-worker default. Explicit two- and four-worker overrides are profiled
on both binaries. Each table cell contains two repetitions of overlap-A and
several-large: four fresh acquisitions, unshaped loopback, identical frozen
sources and scientific work. All 32 accepted instrumented acquisitions pass
independent exact-value, metadata, coverage and completion verification.

The diagnostic candidate consumer has SHA-256
`dbba53555cb0ad72f5c7d0b71ef7b793329cfcf1f15779ffa5498b80ae9d2a9f`.
The final matrix's own binary receipt remains its authority, including subsequent
validation-guard edits. Do not silently substitute these diagnostic measurements
for final-matrix measurements on a differently hashed consumer.

| Consumer | Explicit workers | GDAL block-cache setting | Observed process peak RSS range, MiB | Largest sampled block-cache use, MiB |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 2 | 1 MiB | 115.86–146.00 | 4.00 |
| Baseline | 2 | 32 MiB | 127.20–147.12 | 16.00 |
| Baseline | 2 | 64 MiB | 133.08–148.33 | 16.00 |
| Baseline | 4 | 1 MiB | 131.09–159.45 | 8.00 |
| Baseline | 4 | 32 MiB | 154.86–166.75 | 30.00 |
| Baseline | 4 | 64 MiB | 152.69–178.77 | 32.00 |
| Candidate | 2 | 32 MiB | 120.36–127.89 | 8.00 |
| Candidate | 4 | 32 MiB | 123.53–132.09 | 12.00 |

The 1-MiB cache setting still permits approximately 4/8 MiB of active decoded
blocks with two/four baseline workers. A source block is 1024 × 1024 UInt16
samples, or 2 MiB before allocation overhead. GDAL's configured cache capacity
therefore cannot be treated as a strict peak allocation bound. Reducing the
block-cache setting to 1 MiB also does not eliminate the baseline's several-large
75,988,992 source-response bytes. Enlarging it to 64 MiB does not reproducibly
eliminate the excess traffic. Neither ablation justifies changing the 32-MiB
cache default.

## What occupies memory

The scheduler reports only 262,144/524,288 bytes of active COG output reservations
with two/four workers in these cases. One 256 × 256 UInt16 output payload is
131,072 bytes; consumer delivery can additionally retain a payload while the
producer proceeds. The 16-MiB working budget is neither an allocation of 16 MiB
nor a charge for every allocation below the GDAL API.

Independent paused baseline processes provide allocation-stack ownership via
macOS `malloc_history`, with `heap`/`vmmap` snapshots. Pausing after the eighth
chunk allows workers to reach delivery backpressure. Stack logging and inspector
pauses perturb memory, so snapshot RSS is excluded from all resource gates and
timing comparisons. The following are live allocation sizes rounded by the
platform tool, in its native `M`/`K` notation; they are not an additive RSS model.

| Live allocation owner | Two-worker snapshot | Four-worker snapshot |
| --- | ---: | ---: |
| `GDALRasterBlock::Internalize`, decoded image blocks | 5 allocations, 11.8M | 8 allocations, 16.9M |
| `VSICurlFilesystemHandlerBase::AddRegion`, compressed HTTP byte regions | 471 allocations, 9.20M | 571 allocations, 11.2M |
| zlib `inflateInit2_` through TIFF `ZIPSetupDecode`, retained decoder state | 2 allocations, 16.0K | 4 allocations, 32.0K |
| Native `RasterBand::read_as` output allocations | 3 allocations, 432K | 5 allocations, 720K |
| Direct COG planner allocations in this call-tree branch | 228 allocations, 42.5K | 228 allocations, 42.5K |

The decoded image blocks are decoder output and already belong to GDAL's block
cache accounting. The retained zlib state is small in these snapshots; this does
not bound transient decoder scratch that was freed before inspection. The
planner branch is a partial attribution, not all metadata. Other live stacks
include GDAL/PROJ spatial-reference structures, SQLite, HTTP clients, runtime and
linked-library initialization.

GDAL additionally has a process-wide `/vsicurl/` compressed-byte cache, distinct
from its decoded block cache. Its documented default is 16 MB; directory/HTTP
metadata, active responses, libcurl and decoder buffers are additional.
[GDAL's virtual-filesystem documentation](https://gdal.org/en/stable/user/virtual_file_systems.html)
describes this cache and its independent `CPL_VSIL_CURL_CACHE_SIZE` control. The
allocation stacks above directly establish that this separate cache is occupied.

At the sampled RSS peak, RSS minus live libc allocations ranges from **76.64 to
106.92 MiB** in the baseline 32-MiB-cache trials and **82.30 to 87.94 MiB** in the
candidate trials. This is an **unassigned arithmetic residual**. It includes some
combination of mapped code/data, allocator slack, stacks and allocations outside
the sampled live heap; it must not be labelled measured decoder memory. Live
malloc allocations already include GDAL blocks and the compressed-byte cache,
so those figures must not be added again. The four-worker paused `vmmap` snapshot
also reports approximately 25.0M of dirty allocator fragmentation and empty
retained regions. Its all-mappings resident total includes shared mappings and
does not equal `wait4` process RSS; summing it into RSS would be incorrect.

The candidate's lower live block-cache peaks and live heap agree with its lower
observed RSS. Source-reader lifetime changes remove duplicated work and reduce
simultaneous retained decoded blocks. These measurements do not retroactively
identify the exact allocation responsible for each inherited resource failure.

## Preserved failures and default decision

The original matched four-worker matrix retains **three RSS-gate failures**.
Their exact wall time/RSS values were lost by its original assertion-only gate;
scientific outputs were rechecked, and the failed outcomes remain recorded.
The inherited fixed-two-worker follow-up passed **24/24** acquisitions across
all four workloads and both unshaped/shaped links, with maximum observed RSS
**154.17 MiB**. Those runs remain distinct from the original default-worker
matrix. The new instrumented baseline runs do not reproduce a miss on unshaped
loopback, which does not negate the earlier shaped-link failures.

Two workers are the defensible constrained default. The final matched matrix
must still measure fresh-process RSS with the uninstrumented candidate and
retain explicit four-worker resource diagnostics. No profile here covers
arbitrary source layouts, arbitrary manual concurrency/cache settings,
caller-retained arrays or unrelated application state. Enforcing an absolute
process RSS ceiling would require an allocation model or an operating-system
resource boundary covering GDAL, runtime and caller allocations; the working
reservation alone does not provide one.

Reproduce accepted diagnostics from the project root:

```sh
prototype-output/zarr-env/bin/python bench/cog_memory_profile.py \
  --label baseline-memory-repeat --probe --reps 2
prototype-output/zarr-env/bin/python bench/cog_memory_profile.py \
  --label candidate-memory-repeat --probe --reps 2 --cache-mib 32 \
  --consumer target/release/tile-ripper-native-stats
prototype-output/zarr-env/bin/python bench/cog_memory_profile.py \
  --label baseline-live-allocations --snapshot-after-chunk 8 \
  --cache-mib 32 --reps 1 --workloads overlap-a
```

The first observer harness attempt allowed an auxiliary subprocess to inherit
its log path. Its partial raw artifacts are retained and excluded; the corrected
harness injects the observer only into the measured consumer process.
