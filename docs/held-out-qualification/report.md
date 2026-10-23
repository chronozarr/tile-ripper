# Held-out SWOT Raster D qualification

Local decision gate, 2026-10-23, continuing `987e760` on
`codex/acquisition-planner`. **Pass for the documented held-out selection and
unchanged good/suspect policy.** The source-format and authentication contracts
are unchanged. Nothing was pushed, merged, tagged, published or deployed.

| Decision | Evidence and result |
| --- | --- |
| 1. NetCDF-C failure | **A: incomplete reference capture.** Opening eagerly reads metadata absent from the native subset capture. The original abort is preserved. |
| 2. Independent references | Separate h5py/HDF5 and NetCDF-C/HDF5 processes read version-pinned source bytes with automatic NetCDF masking/scaling disabled. Both pass under an enforced capture guard. |
| 3. Additional reference acquisition | **61,298 bytes, 127 range GETs**, under the predeclared 512 KiB / 128 range-GET cap. Only observed reference metadata misses were fetched. |
| 4. Meaningful held-out selection | **1,310 accepted cells** in a 128×128 window. Every stored bit, source mask, derived mask, coordinate, identity and summary matches both readers. The original empty window also passes. |
| 5. Existing workflow | Same normal `plan`/`stream` CLI, native reader, explicit Earthdata session and access policy. No product-specific production branch or science correction. |
| 6. Resources | The 32 MiB budget is too small; an inspected conservative **37,434,488-byte** budget succeeds. Plan inspection and the admission error now explain the components. |
| 7. Resume | Eleven local held-out scenarios pass. Live NASA interruption/resume and complete-checkpoint reconstruction pass; complete replay fetches **8 source-body bytes** for fresh authorization. |
| 8. Platforms | macOS ARM64: eight focused filesystem tests pass. Linux ARM64: the same eight pass as UID 1000, plus an explicit foreign-owner rejection test as root. |
| 9. Broader qualification | Limited to the documented PGD0 products, selections and policies. Other scenes/releases/quality policies and larger resource envelopes still need their own qualification. |

## Preserved failure and diagnosis

The held-out source is CMR `G3927559976-POCLOUD`, selected by the earlier
deterministic next-day lexicographic rule in
[the original candidate receipt](../transfer-resume/held-out-candidates.json):

`SWOT_L2_HR_Raster_100m_UTM01W_N_x_x_x_011_082_016F_20240217T183710_20240217T183712_PGD0_01.nc`

Its canonical URL, strong ETag `"8c583d1085e24a2c438260a206d38131-1"` and
6,527,265-byte object length remain pinned. The original native capture has
390,822 unique captured bytes. Before any extension, 13 files plus the original
checkpoint were copied into `prototype-output/held-out-qualification/original/`.
[Preservation hashes and environment](preservation.json),
[checkpoint record hashes](preserved-checkpoint.json), and
[the exact failing command and signal](failure.json) retain the original
request, candidate, source identity, capture extents/hashes, reference harnesses
and result receipts. The original capture and old failing receipt were not
overwritten.

The unchanged original reference command reproduces SIGABRT (return code -6),
with empty stderr. Separately executed probes show that the failure occurs
**inside `netCDF4.Dataset(path, "r")`**, before the first completed open and
before harness enumeration, attribute access, coordinates, science reads,
masking/scaling or explicit cleanup. Some repetitions exit with NetCDF error
-101 instead of aborting. h5py targeted reads pass on the old capture, but
h5py enumeration also fails or aborts. [All original isolated probes](diagnostic.json).

The diagnostic POSIX interposer identifies the first missing read as
`[4985152, 4985664)`. It returns EIO and writes `CAPTURE_MISS` rather than returning
filesystem-synthesized zeros. The fetched bytes begin with `FHDB`, the HDF5
fractal-heap direct-block signature, and contain root variable names. Further
misses occur while NetCDF-C opens/enumerates the other variables' metadata.
This is missing reference input, separate from an HDF5 chunk index declaring a
science chunk unallocated. All five selected science chunks are allocated;
the native plan and both readers agree on their values and CF masks.

Only supplying the observed missing metadata makes the same reader binaries
pass opening, enumeration, attributes, both coordinates, each of the five
science variables, and closing. The native scientific decoder was not changed
to obtain this result. The abort is retained as observed library error handling
on incomplete input; this work does not diagnose a separate HDF5 abort-handler
defect. [Regression: explicit EIO control and 22 corrected operations](reference-regression.json).

## Reference input and independence

[The cap](reference-budget.json) was saved before fetching. Each added extent
is the exact hole requested by the guarded reader, fetched through the existing
official-range mirror with conditional `If-Match`, exact ETag/length/range
framing and identity encoding. Every added extent and the final capture are
hashed in [the range receipt](reference-ranges.json). The completion procedure
also invokes conditional HEAD validation at initialization, after each extension
and at sealing: 129 HEAD operations for this run. The range-GET cap excludes
these structurally bounded checks and authentication/redirect hops. Reference
wire authentication traffic was not separately instrumented.

The extended reference capture contains 452,120 unique bytes, about 6.93% of
the object. It remains an incomplete input outside the tested procedures.
An optional broader h5py `visititems` traversal requested another uncaptured
metadata range; that diagnostic was stopped without fetching more bytes.
[Retained traversal miss](optional-traversal.json). This is not a full-file
reference capture or a claim that arbitrary HDF5 operations work on it.

Reference additions live only in
`prototype-output/held-out-qualification/reference-mirror/`. Native local tests
serve the **original native-only capture** through strict replay; live native
tests read directly from NASA. Reference-fetched bytes never enter native
acquisition or its checkpoint/accounting.

The scientific worker remains the unchanged
[`swot_quality_reference.py`](../../bench/swot_quality_reference.py), which
does not import native acquisition tooling. Its stored-value arrays, CF masks,
quality calculation and coordinates are compared exactly with normal CLI output.
The references run separately, including separate h5py and NetCDF imports:

| Reader | Actual scientific libraries | Linkage evidence |
| --- | --- | --- |
| h5py 3.12.1 | HDF5 1.12.2 | [Dylib paths, hashes and `otool` linkage](h5py-environment.json) |
| netCDF4 Python 1.7.4 | NetCDF-C 4.9.3; HDF5 1.14.6 | [Dylib paths, hashes and `otool` linkage](netcdf4-environment.json) |

The two readers use different HDF5 builds and remain independent of Tile-Ripper's
native Rust parser/decoder. The interposer only checks byte coverage; it performs
no format parsing, numerical conversion or scientific comparison. Both scientific
reference children and the parent metadata checks run under it, with zero misses.
NetCDF-C opens a filename through POSIX I/O; it does not use h5py's file-object
interface.

## Bounded selection and scientific result

The [predeclared selection rule and budget](selection-budget.json) samples
`/wse_qual` every 16 cells starting at `(0,0)`, in row-major order, with at most
16,384 returned quality values and zero additional provider bytes. The actual
112×112 lattice contains 12,544 values and 51 good/suspect samples. This variable
occupies one physical 1,779×1,780 chunk: HDF5 decodes that 3,166,620-byte chunk
even for the sparse selection, from its already captured 18,509 encoded bytes.
No whole product was scanned or downloaded.

The first accepted sample is `(16,720)`. Centering and clipping a 128×128 window
gives rows `[0,128)` and columns `[656,784)`. The original all-invalid
`[600:728,400:528]` window is retained as a separate edge case. The lattice,
category counts, coordinates, inspected windows and exact request are recorded
in [selection.json](selection.json). No later window was tried.

The [qualification](qualification.json) compares all 16,384 stored values in
each of five variables, every source mask, coordinate and variable definition,
the derived good/suspect mask and all summaries against both references.
There are 1,310 accepted cells, WSE mean 29.671594082490177 m, and contributing
pixel sum 13,702. This describes the selected raster region; it is **not a
river-specific result**. The quality policy is unchanged in [policy.json](policy.json).

## Resource admission

The [resource receipt](resources.json) reproduces the original selection at
32 MiB, at the inspected sufficient estimate, at 64 MiB and at 128 MiB, with
the same authenticated/checkpointed transport and 16 KiB metadata strategy.
The three admitted executions also pass both references.

| Component | Bytes / interpretation |
| --- | --- |
| Physical science chunks | Four layers have 890×890×4 = 3,168,400 native bytes; summary quality has 1,779×1,780 = 3,166,620. The model reserves eight times native bytes plus three times encoded bytes for the largest active decoder, not the sum of five simultaneous decoders. |
| Limiting decoder | `/wse_uncert`: 25,460,429 bytes, including its 37,743-byte encoded chunk. |
| Coordinate preparation | Full physical axes contain 14,232 and 14,240 decoded bytes. The observed preparation reservation peaks at 4,626,755 bytes including its then-live metadata/transport state; this phase is not added again to the execution total. |
| Prepared metadata, coordinates and plan | 1,920,796 bytes, excluding transport reservations. |
| Authentication | 1,048,576 bytes. |
| Checkpoint | 2,162,688 bytes with the default 2,048-entry index/scratch bound. |
| Encoded-range retention | `buffer_bytes / 8`: 4,194,304 bytes at 32 MiB; 16,777,216 at 128 MiB. |
| Fixed stream overhead | 65,536 bytes. |
| Aligned assembly | Admission allows 128 bytes per tile pixel; 2,097,152 bytes conservatively accommodates this complete 128×128 logical tile. Reported array/coordinate peak above the base is 305,152 bytes. |

At 32 MiB the base alone is **34,852,329 bytes**, exceeding the requested
33,554,432 bytes before assembly. At 128 MiB the measured working reservation
is **47,740,393 bytes**, matching the earlier receipt. The working reservation
is not process RSS; RSS was separately sampled around 30 MB in these runs.

The new `aligned_execution.working_memory` inspection and execution error share
the existing admission calculation. They identify `/wse_uncert`, list component
sizes and report a conservative **37,434,488-byte (~35.70 MiB)** sufficient budget
for this prepared selection and metadata strategy. The estimate accounts for
the cache increasing with the requested budget, preparation floors, and room for
the largest logical tile. Requesting that exact budget succeeds; the regression
also checks an independently generated fixture at its inspected budget.
No requested budget is changed automatically. This is not a universal SWOT
minimum or an RSS promise.

## Held-out transfer recovery and costs

[Eleven local HTTPS scenarios](resume.json) use only the original pinned native
capture. They cover uninterrupted acquisition, interruption after complete
science ranges, fresh-session resume, complete reconstruction, changed source
generation before reuse, mutation during final replay validation, incompatible
selection and policy, provisional delivery interruption/redelivery, and a fresh
checkpoint write control. Every successful result passes both scientific
readers and a consumer that recalculates from zero. Failed streams are rejected
by that consumer, including those that already delivered a tile.

| Local attempt | New source-body bytes | Previously retained bytes read | Wall / CPU seconds | Sampled process RSS |
| --- | ---: | ---: | ---: | ---: |
| Uninterrupted, no checkpoint | 453,889 | 0 | 0.173 / 0.055 | 27,869,184 |
| Resume after science interruption | 69,187 | 384,710 | 0.081 / 0.052 | 30,277,632 |
| Complete checkpoint replay | 8 | 453,889 | 0.067 / 0.051 | 27,541,504 |
| Uninterrupted checkpoint write control | 453,897 | 0 | 0.487 / 0.074 | 30,588,928 |

The resumed 69,187 bytes comprise **69,179 new source bytes plus the eight-byte
fresh authorization proof**. This saves 384,702 source-body bytes versus a full
restart after the same interruption. The interrupted attempt sent 384,718 bytes;
combined interruption plus resume sends 453,905 versus 838,607 for interruption
plus restart. These are single local fixed-delay observations, not performance
benchmarks or general speedup claims. The fresh write control records 0.276 s
of checkpoint write time; resume records 0.00143 s writing new ranges and
0.00074 s reading/hashing retained ones. All per-attempt counters are retained.

The completed local checkpoint contains 27 records and occupies 459,259 logical
bytes: 453,897 range-body bytes, 2,376 bytes of fixed record headers, and 2,986
identity bytes. Filesystem allocation/inode/directory overhead is outside these
logical bytes. Authentication proofs, login/redirect counts and HEAD validation
are separated from retained/new source payload in the receipts.

[Bounded live acceptance](live.json) uses the same request, policy and ordinary
Earthdata CLI options, with a 1 MiB checkpoint cap and three invocations.
The first invocation stops after metadata preparation with 329,813 bytes of
complete ranges retained. Resume reuses those bytes and fetches 124,076 new
source bytes plus the eight-byte authorization proof. Complete replay then
reuses 453,889 bytes and fetches only the eight-byte proof. Each successful
invocation still makes five final-resource HEAD validations, one login, and
nine redirects; complete replay has ten wire HEADs and five wire GETs including
hops. Its 374 redirect-body bytes are separate from the eight source bytes.

All three live invocations together receive 453,913 source-body bytes, including
24 proof bytes, and 1,122 redirect-body bytes, below the recorded 2 MiB expected
source-body bound. The live checkpoint occupies 460,835 logical bytes, including
the larger actual Earthdata identity. Both live successful streams match both
references. The final rebuilt native binary has the same SHA-256 as these runs:
`39401be62333b5ae5918b1a71e8b15bfb0400ba8863c8a9de2df3ae260f9dd82`.

Only complete source ranges persist. The scientific stream and consumer
calculation restart at zero; delivered tiles may repeat. Only successful
`finish()` **and** successful process completion permit committing results.
No decoded tiles, consumer cursor, exactly-once behavior or offline success is
provided. The [existing source-transfer contract](../transfer-resume/decision.md)
and [authenticated-access contract](../earthdata-auth/access-contract.json) apply.

## Platform and verification limits

[Platform receipt](platforms.json) records the exact Docker image, architecture,
UIDs, commands and log hashes. Linux testing uses the existing ARM64
`rust:1.92-slim` image, Rust 1.92.0, and its `/tmp` Linux filesystem; source and
Cargo registry mounts are read-only. Eight focused tests run as UID 1000 so real
permission-denial checks work. A separate root invocation opens a 0700 directory
owned by UID 1000 and proves the explicit ownership check rejects it before
creating managed files. The added no-replace test attempts publication over an
existing completed record and confirms that reopening still reads the old bytes.

The focused tests also exercise locks in-process and across processes, incomplete
temporary recovery, corruption/truncation, symlinks/hardlinks, unrelated-file
preservation, injected ENOSPC and eight abrupt persistence-exit boundaries.
This establishes process-interruption behavior on macOS and Linux ARM64; it does
not establish power-loss durability, other Linux filesystems/architectures or
Windows support.

[Verification commands, counts, source hashes and log hashes](verification.json)
cover native-HDF5/authenticated Rust tests, all-feature strict Clippy, no-default
compilation, Python/C checks, the corrected-reference regression and the
qualification/resource/resume gates. The existing 32-case adversarial CLI resume
suite also passes against the final binary, with its original receipt retained
and [this rerun saved separately](fault-regression.json).
Raw captures, subprocess stdout/stderr and
range traces remain under `prototype-output/held-out-qualification/`; compact
receipts are versioned here. The optional broad traversal miss remains explicit.

Reproduce offline gates from this checkout with its preserved captures:

```sh
python bench/swot_held_out.py preserve
python bench/swot_held_out.py diagnose
python bench/swot_held_out.py reference-regression
python bench/swot_held_out.py select
python bench/swot_held_out.py qualify
python bench/swot_held_out.py resources
python bench/swot_held_out.py resume
python bench/swot_held_out.py faults
```

`complete-reference` is an explicit, bounded provider operation that adds only
guard-observed metadata holes to the separate reference capture. `live` is a
separate explicit NASA acceptance operation; repeat local gates first.
The [committed native request](request.json) can be passed directly to the
existing `plan`/`stream` CLI with `--quality-policy policy.json --auth earthdata
--netrc "$HOME/.netrc"`; add a private `--checkpoint` directory for transfer
resume. The existing Rust APIs need no new adapter.
