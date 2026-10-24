# Efficient source validation: decision and evidence

Keep the small HDF5 binding change and stop this transport milestone here.
One live ETag/size observation now also seals the just-created one-object
binding. No bytes or callbacks intervene between those operations. The former
second adjacent HEAD established no additional consumed-data boundary.
Anonymous shared-object and native COG bindings also remain invalid after a terminal source failure,
including when a server restores an earlier version. An in-flight successful
response cannot clear a concurrent failure of that binding.

Work continues from `157facc` on `codex/acquisition-planner`.
[The complete lifecycle, diagrams and check/reuse contract](lifecycle.md),
[per-request authenticated trace](lifecycle-authenticated-swot-candidate.json),
[per-request anonymous COG trace](lifecycle-anonymous-cog-candidate.json),
[all repeated metrics](summary.json) and [verification manifest](verification.json)
provide the decision gate. Full diagnostics remain locally available by hash in
[the receipt index](receipt-index.json).

## Wire requests and source traffic

These are fixture-observed HTTPS requests, including signing and login hops.
They are distinct from application-level HEAD calls. Source bytes count response
bodies and exclude HTTP/TLS framing; redirect bodies are separately accounted for
and empty in these fixtures. All source-body totals are unchanged.

| Operation | Canonical HEADs, before → after | All wire requests, before → after | Source body bytes, either version |
|---|---:|---:|---:|
| A: one authenticated SWOT stream | 5 → 4 | 40 → 38 | 453,889 |
| A: fresh checkpoint acquisition | 5 → 4 | 41 → 39 | 453,897 |
| A: entirely retained checkpoint replay | 5 → 4 | 15 → 13 | 8 |
| B: three-source geographic generation | 15 → 12 | 104 → 98 | 1,021,395 |
| B: independent normal plan | 9 → 6 | 92 → 86 | 1,021,395 |
| B: independent durable fetch, including final sweep | 24 → 18 | 156 → 144 | 7,426,555 |
| B: sum of generation + independent plan + fetch | 48 → 36 | 352 → 328 | 9,469,345 |
| Anonymous HDF5 control | 5 → 4 | 31 → 30 | 71,210 |
| C: anonymous native COG | 5 → 5 | 12 → 12 | 6,615 |
| D: Zarr v2 | 52 → 52 | 78 → 78 | 15,744 |
| D: Zarr v3 ordinary chunks | 177 → 177 | 223 → 223 | 7,414 |
| D: Zarr v3 sharded | 57 → 57 | 81 → 81 | 9,736 |

For A, the removed canonical HEAD also removes one resource HEAD: two actual
wire requests. The 26 conditional resource GETs and one login request remain.
Signed-route resolutions fall from six to five. In B's durable fetch, six fresh
object bindings occur: three acquisition bindings and three final-sweep bindings.
The saving is therefore six canonical HEADs plus six signed-resource HEADs.
Its 84 conditional GETs and six login requests remain. The final sweep still
uses fresh sessions, original identity comparisons, online authorization GETs
and final conditional HEADs.

| Source/phase | Metadata-request body bytes | Scientific payload-request body bytes | Authorization proof bytes |
|---|---:|---:|---:|
| A, ordinary stream | 329,813 | 124,076 | 0 |
| A, checkpoint warm | 329,813 | 124,076 | 8 |
| A, retained replay | 0 | 0 | 8 |
| B, generation or independent plan | 1,021,395 | 0 | 0 |
| B, durable fetch and final sweep | 1,021,395 | 6,405,136 | 24 |
| Anonymous HDF5 | 69,650 | 1,560 | 0 |
| Native COG | 3,840 | 2,775 | 0 |
| Zarr v2 | 4,584 | 11,160 | 0 |
| Zarr v3 | 5,441 | 1,973 | 0 |
| Sharded Zarr v3 | 7,763 | 1,973 | 0 |

Metadata-request bytes include bounded parser-page overfetch; the COG fixture
is small enough that its initial parser page includes the object. Shard-index
requests are metadata. Anonymous wire-body classifications reconcile exactly
with reader payload counters; authenticated phases reconcile with total source
wire bytes. Reduced HEAD counts do not conceal additional GETs or deferred work.

## Controlled performance

Three interleaved repetitions per binary and selection run with 0, 25 and
100 ms **added delay per wire request**. Zero means no injected delay; TLS,
process startup and fixture transport costs remain. Baseline and candidate
binaries are preserved and hashed. Each format uses the same fixtures, selection,
stream/writer and reference checks on both sides. Authenticated runs use macOS
ARM64; anonymous runs use Linux ARM64 GDAL-free binaries and the platform's
private fixture CA trust. Absolute timings across these different selections and
hosts are not comparisons between formats. Compilation and regression work
finish before the matched timing runs.
Geographic generation uses the existing native fixture adapter, which omits
catalog assertions only for local mirrored objects. Production CMR locator and
catalog validation remain strict and are covered by the discovery regressions.
This milestone makes no new live NASA timing or access qualification claim.

Median wall seconds, before → after:

| Operation | 0 ms added | 25 ms added | 100 ms added |
|---|---:|---:|---:|
| A: authenticated stream | 0.069 → 0.070 | 1.245 → 1.190 | 4.352 → 4.153 |
| A: retained checkpoint replay | 0.061 → 0.061 | 0.508 → 0.444 | 1.686 → 1.447 |
| B: geographic generation | 1.470 → 1.457 | 4.538 → 4.356 | 12.528 → 11.742 |
| B: complete independent workflow | 1.838 → 1.844 | 12.585 → 11.391 | 39.311 → 36.330 |
| Anonymous HDF5 | 0.961 → 0.956 | 1.629 → 1.621 | 3.499 → 3.395 |
| C: native COG | 0.519 → 0.520 | 0.748 → 0.750 | 1.606 → 1.605 |
| D: Zarr v2 | 0.845 → 0.832 | 2.251 → 2.239 | 5.961 → 5.968 |
| D: Zarr v3 | 1.376 → 1.385 | 4.738 → 4.751 | 13.831 → 13.837 |
| D: sharded v3 | 0.908 → 0.911 | 2.334 → 2.350 | 6.117 → 6.115 |

There is **no zero-delay throughput improvement** for the authenticated source
or geographic workflow. COG and Zarr have an explicit no-improvement outcome:
their request/byte counts are identical, and small timing differences are noise.
The delayed HDF5 gains are consistent with removing one HEAD per binding; the
three-source full-workflow saving at 100 ms is 2.982 seconds for 24 fewer wire
requests. Three repetitions support a controlled comparison, not a general
provider speedup or a statistical significance claim. Every sample and min/max
range is retained in the metrics.
The 24 removed hops account for 2.4 seconds of configured delay at 100 ms,
and 0.6 seconds at 25 ms. Remaining differences in whole-workflow wall time
include other timing variation, including durable-output synchronization; they
are not attributed entirely to transport validation.

Median preparation and first provisional chunk times at 25 ms added delay:

| Stream | Preparation, before → after | First provisional chunk, before → after |
|---|---:|---:|
| Authenticated SWOT | 0.965 → 0.913 | 1.175 → 1.119 |
| Anonymous HDF5 | 0.995 → 0.977 | 1.096 → 1.075 |
| Native COG | 0.191 → 0.193 | 0.339 → 0.341 |
| Zarr v2 | 1.622 → 1.630 | 1.926 → 1.932 |
| Zarr v3 | 2.060 → 2.080 | 2.768 → 2.774 |
| Sharded v3 | 1.428 → 1.439 | 1.693 → 1.703 |

These in-process stream clocks start after input parsing; driver wall time also
includes startup and receipt serialization. B separately retains generation,
independent planning, per-source preparation/producer-first-output clocks,
first complete observation, final source sweep and whole-publication timings.
A first chunk is provisional; its arrival does not authorize completion or
publication. Checkpoint replay reconstructs and compares all scientific chunks,
uses eight fresh source bytes and still performs its final checks.

## Scientific, mutation, authentication and completion gates

The matched runs comprise 108 authenticated operation records, 90 anonymous
records and 18 derived workflow sums. Scientific values, coordinates, masks and
all three durable-output fingerprints match between implementations. The three
qualified geographic outputs also pass separate guarded h5py and NetCDF-C
references. No live provider requests or new scientific captures were needed.

The [explicit authenticated fault matrix](authenticated-faults.json) compares
both binaries on 20 cases, including mutation just after binding, between grid
inspection and preparation, after preparation and after provisional delivery;
authorization failure after earlier success; missing/weak/changed validators;
size/range/length conflicts; expired and wrong-method signed routes; cancellation
and bounded failure. The [anonymous matrix](anonymous-faults.json) contains 55
fault/restore records across HDF5, COG, v2, v3 and sharded v3. There are no new
successful invalid-source completion paths in either matrix.

Two existing distinctions are explicit rather than counted as new rejections:

- V2 can finish after an already-read positive payload object changes: it checks
  metadata and missing objects again, while positive payload identity is checked
  at its conditional read. V3 checks positive objects/shards again as well.
  Neither provides an atomic collection snapshot.
- The COG fixture can accept a same-ETag size change before its native decoder
  binds the new size. Its initial resolver has not supplied cached parser bytes;
  subsequent opens compare the decoder snapshot and ranges check total size.
  This baseline acceptance is unchanged. It does not establish a general
  guarantee against providers lying about strong ETags.

The tightened anonymous restore cases first detect a conflict, restore the
server and retry the **same** preparation. Baseline accepts that retry; candidate
rejects it for HDF5/COG/v2/v3/sharded v3. COG also exercises a payload conflict
followed by restoration. A fresh preparation remains the recovery
boundary. Four concurrent preparations, changed actual coordinate bytes at a
reused canonical locator and sticky old-version conflicts also pass.

Repeated existing gates include:

- 64 HDF5 stored-value/coordinate oracle cells and 33 HDF5 faults.
- 38 qualified five-layer science cells, 31 quality/profile faults and 108
  independent geometry cases covering 181,548 centers.
- 38 authentication/secret-confinement cases and 32 checkpoint/replay cases.
- 19 multi-observation recovery/admission cases, including mutation of an earlier
  source during later acquisition, cross-source login refusal, checkpoint restart,
  cancellation and resource limits. Shared-writer Rust gates additionally cover
  destination races, cancellation and no-replace publication.
- 61 native COG profile/fault checks, exact COG samples and seven public-consumer
  gates; CF-Zarr/v2 regressions; 40 v3 science and 27 v3 fault cases, including
  physical shard identities and bounded shard-index/payload reads.
- All-feature/all-target Rust tests: 151 passed, nine explicitly ignored;
  all-target Clippy with warnings denied; formatting and Python checks.
- All 16 native feature combinations and a clean Linux ARM64 GDAL-free build:
  80 tests passed, six explicitly ignored. The binary links no GDAL, HDF5,
  NetCDF or PROJ shared library. Ignored runtime/manual helpers are individually
  listed in verification; ignored tests are not counted as passes.

An initial Linux comparison was discarded after detecting shared Cargo root
artifact reuse. Final baseline/candidate targets are separate. A parallel debug
test link exceeded container memory; final Linux tests run serially in release
mode. Filesystem permission tests were rerun as an ordinary user after a root
run bypassed their intended write denial. No production change or test skip
resolved these setup failures. Their retained logs and the successful final log
are hashed in verification.

## Memory, API and remaining latency

No identity record, cache entry, retention lifetime or public API parameter is
added. The existing shared-object failure flag now applies to anonymous bindings
too, and is checked on concurrent success. Native COG adds one preparation-local
atomic failure latch shared with its readers and pre/post identity checks,
including readers without retained parser pages. Ordinary consumer cancellation
remains retryable. The latch is fixed bookkeeping: a 24-byte Arc allocation on
these 64-bit targets, an Arc field in the source context and a reference per
active native reader, before allocator overhead. Reservation formulas retain
their existing fixed stream/codec allowances; no variable cache component is added.
Prepared reservations, measured scheduler peaks, Rust allocation counters for
generation and per-process RSS are retained separately. Existing timing JSON
sizes and scheduling can vary reservations slightly. RSS is not the buffer
budget; v2's legacy prepared metadata accounting remains separately bounded.
There is no demonstrated memory improvement.

Median process peak RSS at zero added delay (MiB, before → after):

| Consumer | Peak RSS |
|---|---:|
| Authenticated SWOT | 28.92 → 28.86 |
| Three-source generation | 26.72 → 26.34 |
| Three-source durable fetch | 43.41 → 42.50 |
| Anonymous HDF5 | 45.17 → 44.94 |
| COG | 45.42 → 45.42 |
| Zarr v2 | 45.55 → 45.55 |
| Zarr v3 | 47.17 → 47.05 |
| Sharded v3 | 47.67 → 47.67 |

The authenticated stream's base working reservation is 37,381,045 → 37,381,045 B.
COG and v3 ordinary/sharded scheduler peaks remain 150,781 B, 42,393 B, 64,035 B
for these selections. Small reservation differences can reflect existing
timing-JSON sizes and scheduling; the native failure latch does not retain data.
Process RSS includes runtime/TLS and the benchmark consumer's fingerprint
retention; it is not a new library allocation estimate.

The anonymous behavioral change is intentional: a terminal binding failure
requires fresh preparation. Authentication configuration, method-specific route
expiry, credentials/cookies, checkpoint digests/quotas, exact conditional ranges,
full-index defaults, geometric inclusion and quality policies retain their
existing contracts. Cargo manifests, lockfiles and dependencies are unchanged.

Remaining provider costs include fresh canonical validation at meaningful
lifecycle boundaries, separate HEAD/GET signing destinations, login/redirect
chains, required conditional metadata and science reads, checkpoint authorization
proofs and the final multi-source sweep. A matching old ETag cannot replace those
observations. COG would need a separate reviewed size-and-version handoff before
its parser-open HEAD could be reconsidered. Zarr's many HEADs describe individual
metadata, absence and payload/shard identities, not one root version.

Further format-specific transport optimization is not the next priority. Evaluate
Tile-Ripper as a complete scientific acquisition product: representative complete
workflows, usability, reproducibility, recovery, portability and user-visible
latency. Revisit transport only if that evaluation identifies a material remaining
cost with a concrete consistency contract. No time-based identity cache is
justified by this milestone.

## Reproduction and scope

The [brief](brief.md), pinned [anonymous fixtures](fixtures.json),
[authenticated fixture receipts](authenticated-fixtures.json), binary/source
hashes in verification and full receipt index define the inputs. Generate the
anonymous fixtures using `bench/source_validation.py fixtures` in the existing
legacy reference environment and `fixtures-v3` in the Zarr v3 environment.
Preserve a `157facc` source snapshot with the same benchmark example, build both
versions into separate targets, and retain the binaries under
`prototype-output/source-validation/{baseline,candidate}`. The Linux script
builds clean native binaries and runs native tests as an ordinary user.

After builds and regressions finish, run `bench/source_validation.py authenticated`
on macOS and `bench/source_validation_wire.py` in the Linux fixture environment
with the preserved Linux binaries. The Linux platform verifier receives only
the private fixture CA through `SSL_CERT_FILE`; production TLS verification is
not bypassed or changed. `bench/source_validation_regressions.py` routes existing
gates to this milestone's evidence directory. Finally run
`bench/source_validation_receipts.py` to summarize/compact diagnostics while
retaining full originals by hash. No persisted report is an execution authority.

All work is local. No merge, push, tag, publication, deployment, ChronoZarr change
or new scientific format is part of this milestone.
