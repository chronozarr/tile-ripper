# Version-bound transfer resume: local decision gate

The gate is open for the previously qualified SWOT Raster D PGD0 product and
five-layer policy. The normal CLI and Rust API now resume verified source
transfers and reconstruct a fresh provisional scientific stream. Work continues
from `60008d3` on `codex/acquisition-planner` and remains local, unmerged and
unreleased. [Implemented command sequence and Rust API](workflow.md),
[precise resume/replay decision](decision.md).

## Contract and source binding

Checkpoint schema 1 binds the unsigned canonical source URL, strong ETag and
object length, versioned exact-origin/method/path redirect policy, dataset paths,
index/coordinate selection and strides, time/grid/transform settings, stored
numeric interpretation version, and the aligned quality policy plus interpreted
flag definitions. Worker count, page size, traversal strategy, buffer budget and
tile edge are execution settings. Changed boundaries miss exact-range reuse;
current object, transport and decoder bounds still apply to hits.

The store contains complete HTTP response ranges, never decoded tiles or a
consumer delivery cursor. Every replay starts the scientific calculation over.
Tiles can repeat across processes; final source validation and `finish()` are
required before consumer publication. The no-checkpoint transport path retains
its original requests and checks. There is no HDF5 output writer, shared cache,
offline reader or consumer-state recovery.

Each checkpoint preparation requires a fresh unused caller-owned session. It
rechecks canonical HEAD identity and performs a conditional eight-byte GET over
the network before touching reusable entries. This additional authorization
proof matters because the archive's HEAD signer can be public. Failed login
cannot become offline success, even when every scientific byte is retained.
Preparation, pre-execution and final source validation remain in place. Fresh
signed routes do not participate in persistent identity. Source ETags remain
trusted version validators; local SHA-256 only detects local corruption.

## Filesystem, secrets and bounds

Each finalized range file contains an 88-byte fixed header (schema magic,
offset, length, acquisition digest, content digest) followed by bytes. A single
temporary record is written, fsynced and atomically renamed without replacing
an existing destination; the immutable acquisition identity is installed first.
There is no separate per-entry manifest update. Startup checks headers and file
lengths; each reused body is hashed before use. Incomplete temporary files are
ignored/discarded only in the managed namespace. Unknown files are preserved.

Directory descriptors pin every path component. No-follow opens, owned private
POSIX modes, ordinary single-link files, generated basenames and exclusive
process-held locking cover the declared macOS/Linux scope. Tests exercise
same-process and cross-process competing opens, symlinks, hardlinks, tampered
offsets/lengths/identity/digests, truncation, real permission errors, injected
ENOSPC, and abrupt process exits at eight identity/entry persistence boundaries.
Those process tests do not establish power-loss durability. A process with the
same user's privileges is outside the local integrity boundary.

Defaults are 64 MiB logical managed-file bytes, 8 MiB per entry and 2,048 entries.
The exact-range index and bounded identity/scratch memory reserve 2,162,688
bytes, plus the existing 1 MiB transport reservation. There is one active write,
at most five checkpoint file/directory handles, and no additional body copy for
persistence. Filesystem block/inode/directory allocation overhead is outside
logical-byte accounting. Limits/errors fail the acquisition; no eviction,
unbounded fallback or unrelated-file cleanup occurs. The largest measured
32-MiB-profile working reservation is 31,922,033 bytes. RSS is measured separately
and includes runtime/TLS/allocator costs; callers retaining tiles are outside
the working reservation.

Request and access-policy metadata serialize through bounded buffers before
JSON values are constructed, and the final identity serializes borrowed fields
through a separate 64 KiB bound. Oversized caller strings cannot first create
unbounded checkpoint copies; the tests include a one-MiB identity input.

Synthetic password/cookie/signature/negotiation sentinels and actual caller
credential values/Basic encoding are absent from audited managed checkpoint
records, native stderr, NDJSON and receipts. Neither signed redirect URLs nor
their query keys appear there. The native CLI links neither libgdal nor
libhdf5. [Redaction/linkage audit](redaction-and-linkage.json).

## Scientific and adversarial evidence

[32 local HTTPS resume scenarios](fixtures.json) include interruption before
useful reads, after metadata, after complete science ranges, during a body, and
after provisional delivery; final mutation after cached replay; fresh signed
routes; denied/expired authentication; changed source length/ETag, selection,
stride, axes, quality and access policy; retries/exhaustion; small memory and
entry budgets; and credential-environment isolation for anonymous CLI requests.
Filesystem/persistence boundaries are covered by the Rust tests above.

Successful uninterrupted/resumed results match independent h5py and NetCDF-C
for every stored bit in all five variables, every source mask, coordinates,
derived quality mask, variable identity and summary. Strided summary/bitwise
cases and execution-tuning changes also pass. The separate restartable NDJSON
consumer begins at zero, recomputes the same summary, rejects interrupted
attempts and sees the first delivered tile again on resume. These are replay
semantics, never exactly-once delivery.

Retained regressions include [38 authenticated transport cases](authenticated-regression.json),
[38 independent quality-science cases](quality-science-regression.json) and
[31 rejected quality faults](quality-fault-regression.json). Rust suites cover
anonymous COG, Zarr v2/v3, HDF5, output transactions and stream lifecycle:
114 all-feature tests, 59 native-HDF5/auth tests and 39 no-default-feature tests
pass; strict Clippy passes for all three configurations. Ignored tests retain
their explicit live/fixture/helper contracts; the persistence helper and
authenticated concurrency case are invoked by their parent gates. Feature
configurations run sequentially because Cargo shares the CLI executable path.
[Exact commands, log hashes, source hashes and counts](verification.json).

## Controlled recovery measurements

The main matrix uses the pinned 89,242,093-byte product mirror behind local
archive/login/signed-resource HTTPS fixtures, with 3 ms injected per source GET
and three repeats per metadata strategy. Every successful stream is checked by
both independent readers and the fresh consumer. A science response is
interrupted after a partial prefix; complete preceding reads are retained.
The restart control repeats the transfer without using those retained bytes.
An additional uninterrupted checkpointed control measures write overhead.
[Full matrix](performance.json), [compact medians](performance-summary.json).

| Metadata | Uninterrupted s | With checkpoint s | Interrupted s | Resume / first tile s | Restart s | Combined resume / restart s | Checkpoint write s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| exact | 0.696 | 0.805 | 0.775 | 0.111 / 0.110 | 0.711 | 0.883 / 1.489 | 0.067 |
| page4k | 0.265 | 0.276 | 0.259 | 0.110 / 0.109 | 0.270 | 0.370 / 0.527 | 0.009 |
| page16k | 0.201 | 0.238 | 0.190 | 0.117 / 0.096 | 0.200 | 0.315 / 0.383 | 0.008 |

Resume saves 705,480 / 822,211 / 1,014,723 source-body bytes versus complete
restart, respectively, after paying the eight-byte fresh authorization proof.
The interrupted server-sent totals include 167,105 bytes of an incomplete
response, which never becomes an entry. Combined interrupted-plus-resumed
source-body traffic is 2,335,972 / 2,452,711 / 2,645,223 bytes. Header/TLS traffic
is outside body accounting; both native received-body and fixture-sent counters
are retained. A server-sent failed prefix need not have been consumed by the
cancelled client.

Each resumed trial makes five source GETs (one authorization proof and four
remaining science reads), no metadata/coordinate GETs, five final-object HEADs,
one login and nine redirects: nine wire GETs and ten wire HEADs including hops.
Full checkpoint replay still performs authorization/validation; it does not
shortcut them. The three failed-initial-login controls retain zero source ranges
and reuse zero bytes from a prior invocation; their subsequent transfer gains
nothing from resume. Current-invocation exact hits are counted separately from
previously retained hits.

| Metadata | Uninterrupted s | With checkpoint s | Interrupted s | Resume / first tile s | Restart s | Combined resume / restart s | Checkpoint write s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| exact | 0.696 | 0.805 | 0.775 | 0.111 / 0.110 | 0.711 | 0.883 / 1.489 | 0.067 |
| page4k | 0.265 | 0.276 | 0.259 | 0.110 / 0.109 | 0.270 | 0.370 / 0.527 | 0.009 |
| page16k | 0.201 | 0.238 | 0.190 | 0.117 / 0.096 | 0.200 | 0.315 / 0.383 | 0.008 |

First tile latency is measured from invocation through preparation/authentication.
The interrupted matrix's first fresh decoded tile mixes retained WSE bytes and
new reads of the other layers; the full-checkpoint live receipt separately
measures the first tile reconstructed entirely from retained source bytes.
No decoded scientific tile is itself persisted.

Timings, first replay/new chunk latency, combined work, checkpoint write time,
CPU, sampled RSS and working reservations are reported per attempt and strategy
in the compact medians. These local fixed-delay measurements demonstrate
recovery, not a general acquisition speedup. The prior live 45.70/19.07/17.04 s
observations are not benchmark baselines for this matrix.

## Bounded live acceptance and held-out outcome

The original qualified product passed normal-CLI interruption/resume directly
against NASA. Independent references use the previously sealed verified sparse
mirror, with no new reference download. A final-current-binary replay reused
2,478,102 retained read bytes and fetched only the eight-byte authorization
proof, plus live authentication/validation operations. Both reference readers
and the fresh consumer matched all five layers, masks and coordinates. It
retains 3,076 cells, WSE mean 275.451613110 m and contributing-pixel sum 21,757.
The complete live checkpoint occupies 2,485,011 logical bytes. Individual live
times are observations, not controlled performance comparisons. [Live receipts](live.json).

A deterministic independent CMR query selected the next-day 100 m UTM01W
PGD0 granule [G3927559976-POCLOUD](held-out-candidates.json), using the same
archive policy and 128×128 indices. Its 1779×1780 arrays and larger physical
chunks cause the 32 MiB execution admission to fail. Raising only the caller's
budget to 128 MiB completes native acquisition with a 47,740,393-byte working
reservation; no production format or routing code was changed for it.

Its 390,822-byte bounded capture supports complete h5py stored-bit/mask/axis/
summary equality, including a remote reference that refuses uncaptured holes.
However, NetCDF-C fails on this incomplete local sparse capture (including
SIGABRT and an error exit across attempts); the cause is not established. The selected window also has
zero quality-accepted cells. **This second product is not fully qualified.**
There is no full-download or profile-expansion workaround. [Held-out result](held-out.json).

The next useful engineering step is to resolve that independent NetCDF-C
reference/capture failure and inspect a bounded nonempty held-out selection
before widening product qualification. Exact-range misses under changed access
patterns, caller-owned consumer transactions, and macOS-only validation of the
filesystem implementation remain explicit limits. No merge, push, tag, release,
deployment, ChronoZarr change or upstream submission occurred.
