# Output-write decision gate

**The regression was repeated destination-chunk replacement and synchronization.
A bounded buffer in the Zarr output adapter removes it on the measured workloads.
The shared planner and minimum source selection are unchanged. Stop here.**

1. **What caused 11.29 seconds?** Source-aligned callbacks fragmented destination
   chunks. zarrs' partial writer repeatedly decoded/recompressed/replaced them;
   its filesystem store synchronizes each replacement. The reproduced medium
   warm median is 11.15 s, with 2,147 native-object writes/synchronizations and
   1,493 reads of previously written destination files. Synchronization alone
   takes about 9.8 s in a representative baseline run. GDAL is not in this path.
2. **What did the fix eliminate?** The adapter assembles decoded destination
   chunks in an LRU limited to one quarter of the existing buffer allowance,
   then uses zarrs full-chunk writes. Medium native-object writes fall
   **2,147→654**, encoded bytes written **2,940,145→999,345**, and destination
   file reads **1,493→0**. Including manifests, writes fall **2,149→656** and
   bytes passed to writes roughly **3.37→1.43 MB**. Final output remains 655
   files and approximately 1.299 MB; every non-manifest object is byte-identical
   to `b3ceb70`. Warm median saves **7.38 s (66%)**, with no weaker synchronization.
3. **Does it approach optimized xarray?** Yes under matched scientific output,
   chunk shape, gzip level and per-object synchronization: 3.77 versus 3.83 s
   warm. This is parity within workstation variability, not a general speedup.
   The old 0.337 s xarray result used larger output chunks and no per-object
   synchronization. It remains valid historical evidence, not an equivalent
   materialization comparison. Different gzip implementations produce different
   encoded sizes even at the same level; all bytes are measured.
4. **Is minimum source selection preserved?** Yes. Current planned, independently
   derived and observed key sets agree: 42 small / 112 medium, each looked up once; existing chunks decoded once and absent chunks filled. Medium encoded source bytes remain
   879,821. The prior `46dd503` needs 1,188 reads / 13,503,563 bytes. Metadata and
   coordinate activity is explicitly outside this data-key guarantee.
5. **Does a bounded stream make sense?** Yes: the existing synchronous
   `Prepared::execute` callback already provides bounded delivery, backpressure,
   cancellation, native values, georeferencing, timestamps and asset identity.
   The example consumes buffers immediately with a provenance header. Small
   allocation-free coordinate/validity helpers work for both COG and Zarr.
   A formal stream header should expose exact stored coordinate axes: affine
   centers alone do not guarantee bit-identical Zarr coordinates. No async
   framework, general stream protocol or ChronoZarr adapter was added.
6. **Ready for remote Zarr?** The planner abstractions remain suitable; the
   product is **not ready**. Remote version/snapshot consistency, conditional
   reads, credentials, transport limits/retries and measurement still need an
   explicit adapter. Bounded LRU eviction can still rewrite chunks when a large
   output frontier exceeds the allowance. No universal minimum-destination-write
   or hard process-RSS guarantee is claimed. No remote work started here.

Three shuffled repetitions per cell, fresh processes/destinations. “Cold source”
uses Darwin `F_NOCACHE`, not a reboot or hardware-cache purge. Times are medians
[min–max], seconds; the active workstation has substantial synchronization stalls.

| Medium reader | Warm | Cold source | Maximum RSS, MiB |
|---|---:|---:|---:|
| `b3ceb70` | 11.15 [10.63–11.36] | 10.70 [10.65–13.36] | 40.2 |
| Buffered writer | 3.77 [3.31–6.71] | 3.49 [3.25–4.15] | 40.4 |
| `46dd503` | 3.49 [3.33–4.15] | 3.67 [3.64–4.34] | 40.2 |
| Matched xarray/Zarr | 3.83 [3.73–4.20] | 4.68 [4.57–4.92] | 84.2 |
| Minimal Zarr writer | 3.69 [3.57–3.94] | 4.47 [4.39–5.20] | 84.6 |

Small warm medians are 1.154→0.431 s (matched xarray 0.639 s); cold-source
medians are 0.938→0.724 s (xarray 0.581 s). Small cold ranges overlap strongly;
there is no consistent cold-small advantage. Full results include every sample,
actual files/bytes, counters and stage timings.

The median-wall medium warm run separates planning (0.0067 s), execution metadata
(0.0016), source retrieval (0.0082), fused read/decode/materialization (0.0157),
assembly (0.0018), output initialization (0.0717), callback sink (2.2566), final
buffer drain (1.2632), and manifest stages (0.0042). Nested subtimers show callback
allocation/copying at 0.00015/0.00039 s, output fill/retrieval at 0.0152 s, output
copying at 0.00041 s, encoding/serialization/library overhead at 0.0346 s, and
filesystem synchronization at 3.3305 s. These nested measurements are not added
to their enclosing stages. Remaining wall time includes startup, snapshot checks
and uninstrumented bookkeeping. Decompression is distinguished through store
subtraction plus independent gzip probes, without replacing the library decoder.

All 26 ordinary Rust tests pass, plus the HTTP-harness snapshot test. Independent
xarray checks validate **20,419,920 values across 60 runs**, raw/CF-decoded nodata,
coordinates, dates, radiometry and metadata. Seven additional boundary/temporal/
empty cases pass; compressed outputs are deterministic across repeated runs and
match the old writer byte-for-byte. A 1 KiB-budget test exercises eviction and
compares objects against a larger buffer. Failure/cleanup tests remain intact.
The CLI plan/fetch and Rust callback demonstrations run successfully.

COG's 18-run comparison independently validates all 72 outputs. Request counts,
source bytes and selected pixels are unchanged. Small medians are 0.174→0.166 s;
medium 0.173→0.170 s, with overlapping ranges and no demonstrated regression.
The additional STAC failure, geometry, pagination and version-cache gates pass.

[Commands, stage definitions and limitations](output-write-workflow.md) ·
[Zarr measurements](output-write-after.json) ·
[Independent edge/determinism checks](output-write-validation.json) ·
[COG comparison](output-write-cog.json) ·
[Final regression checks](output-write-regression.json).

No dependencies, codecs, output formats or planner operations were added.
No paid infrastructure, ChronoZarr edits, merge or publication occurred.

Preservation review: the pinned filesystem store retains a lock per object.
Output-object count (including group/coordinate metadata) is now preflighted
against `budget.max_nodes`, bounding that bookkeeping as well as cached payload.
The guard rejects excessive tiny-chunk outputs before creating the directory;
normal benchmark requests and their measured I/O are unchanged.

The preservation check reran 27 ordinary tests, clippy and all five benchmark
readers in both cache modes: [record](output-write-preservation.json). This
single-repetition check is not evidence of significance against xarray.
