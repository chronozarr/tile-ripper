# Bounded multi-granule SWOT job decision gate

**Passed for the two named, independently qualified Raster D PGD0 selections.**
The normal CLI and typed Rust API now acquire one to four explicit observations,
recover verified source ranges and write an ordinary scientific Zarr v2 dataset.
The serial implementation retains each native grid and enforces an aggregate
working reservation, fixed aggregate checkpoint quotas and all-or-nothing
no-replace publication. Work continues from `7e7aa93` on
`codex/acquisition-planner`; implementation and evidence are local only.

Use [the runnable workflow](workflow.md), [manifest](../../examples/swot-job.json)
and [Rust example](../../examples/swot_job.rs).
[Final-binary live dataset](../../prototype-output/swot-job/live-final-binary-reconstruction-2bwb4n_c/observations.zarr) is a completed local
artifact, not a hosted or released dataset. Its manifest and per-observation
attributes retain the actual NASA canonical identities. Raw datasets/captures
are intentionally outside Git; compact receipts and harnesses are committed.

## Scientific result

The initial end-to-end job uses the original qualified UTM19T observation and
held-out UTM01W observation. The manifest supplies the later observation first;
execution and groups preserve that order. They are heterogeneous native grids,
not an aligned time series at one location.

| Observation/group | Native source shape | Native CRS | Explicit selection | Good/suspect cells |
| --- | --- | --- | --- | ---: |
| `held_out_20240217` | 1,779 × 1,780 | WGS 84 / UTM zone 1N | rows 0:128, columns 656:784 | 1,310 |
| `original_20240216` | 1,580 × 1,580 | WGS 84 / UTM zone 19N | rows 600:728, columns 400:528 | 3,076 |

Each group preserves all 16,384 stored values in **each** of five variables,
five independent source masks, the derived quality mask and both selected
coordinate vectors. Float32 WSE/uncertainty, UInt8 summary quality and UInt32
bitwise flags/counts retain their numeric bits. No calibration, masking of raw
auxiliary values, resampling, reprojection, mosaic or implicit alignment occurs.
No AOI mask is invented for index selection.

[Qualification](qualification.json) and [live receipts](live.json) compare every
value, mask, coordinate, source time field, quality definition and source
identity against separate h5py and NetCDF-C processes. Both actual HDF5 library
versions are recorded. Each real observation passes 142 interpreted scientific
attribute comparisons and 148 fixed-attribute shape/raw-byte checks through
h5py. Uninterpreted HDF5 storage attributes remain preserved as typed raw
provenance; they are not converted into invented CF values. Root processing
identifiers and time-coverage metadata are retained; the optional scalar `/t`
path is separately tested, including its exact signed-zero bits.

Zarr-Python 2.18.3 and xarray 2025.6.1 independently reopen all output groups and
compare their raw arrays. The typed Rust example also runs plan/fetch and passes
both scientific reference readers: [Rust receipt](rust-api.json).
The numeric fixture additionally verifies noncanonical NaN payloads, signed
zero, values above 2³¹, integer fill semantics and explicit packing metadata.
Source attributes named like Zarr dimension/provenance annotations are preserved
without overriding the destination annotations. Missing source CRS metadata
fails rather than causing a CRS inference.

Native held-out acquisition uses its original native-only capture. Independent
references use the corrected, separately extended reference capture under the
unchanged C capture guard. All successful scientific comparisons have zero
capture misses. [Deliberate omitted-signature tests](capture-guard.json) make
both independent readers fail with `CAPTURE_MISS`; missing bytes never become
apparent source zeros. The original held-out failure snapshot is reverified and
its prior failure/qualification evidence is unchanged.

## Resource and access boundaries

The [plan](plan.json) reports the source selections, grids, variable definitions,
output groups, decoder diagnostics and admission costs. There is one active
source and a synchronous writer; there is no queued next tile or completed
observation retained in memory. Each source's prepared metadata, coordinates,
authentication session and checkpoint index are dropped before the next source.

| Component | Working allowance in the example |
| --- | ---: |
| Largest source request cap, including its own decoder/cache/metadata/auth/checkpoint work | 67,108,864 bytes |
| Job metadata, bounded source receipts, coverage and serialization | 33,554,432 bytes |
| Writer scratch, edge 128 | 593,920 bytes |
| Aggregate admitted cap | 101,257,216 bytes |
| Caller aggregate limit | 134,217,728 bytes |

The source cap is reused serially, not multiplied by observation count. Per-source
metadata and execution diagnostics retain the limiting decoder and preparation
requirements. The uncheckpointed two-source execution reservation is
71,337,361 bytes including job/writer allowances; the live checkpointed execution
reservation is 72,975,761 bytes. Coordinate-preparation diagnostics remain
separate, while the larger admitted cap covers the full lifecycle. These are
working reservations, not measured allocated heap or process RSS. The matched
local batch sampled about 38.6 MB RSS; final live reconstruction sampled
40,402,944 bytes. Runtime/allocator/TLS and OS
cache are outside the reservation model. The exact counters and their scope
are recorded, rather than equating reservations with RSS.

Checkpoint bytes and range-entry limits are aggregate, divided into fixed equal
source quotas after charging the root identity. An exclusive private job lock
spans the operation; source namespaces have independent locks and version-bound
identities. All retained namespaces are inventoried before any new writes,
including after limits are lowered. No source can borrow another quota. There
is no eviction, completed-observation cache, unrelated-file cleanup or persistent
consumer cursor. Owned source checkpoints survive success/failure for explicit
caller cleanup. Directory/inode/block allocation is measured separately from
logical checkpoint bytes. Output key counts have their own aggregate limit;
output disk-space errors refuse the transaction rather than silently dropping
observations.

The existing exact-object authentication policy is unchanged. A caller-owned
factory creates a fresh session for each source and again for its final check.
The HTTPS fixtures use distinct login hosts and credentials, assert login-only
Authorization and origin-specific cookies, and reject a redirect toward the
other source's login origin. All stdout/stderr and compact evidence are checked
for fixture secret markers. Signed locators, cookies and credentials do not
enter manifests, checkpoints or scientific output. See [boundary receipt](boundaries.json).

## Completion, failures and restart

All source deliveries remain provisional until every observation has complete
coverage and successful source validation. Once all output is written, a serial
fresh-session sweep rechecks **every** source identity, an online authorization
GET and a conditional final HEAD/local stat. It runs before final root metadata,
manifest, synchronization and no-replace publication. Validation intervals are
recorded per source. This is not an atomic snapshot of a changing NASA collection;
a source can change after its own last validation.

[Fault/restart receipts](faults-restart.json) cover 19 cases, including a
later source failing after an earlier one is written, earlier-source mutation
during the second acquisition, cross-source redirect refusal, acquisition
cancellation/restart, checkpoint reconstruction, small aggregate memory/disk/
entry budgets, lowered-limit inventory, policy mismatch, different dimensions
and strides, all-invalid output, exact unusual numeric bits and unsupported
processing/quality/CRS schemas. Nonempty all-invalid output completes; empty
index counts fail explicitly. Seven Rust integration tests additionally cover
one/two/four observations, equal timestamps without merging, duplicate refusal,
writing cancellation, existing destinations and publication races.

| Local HTTPS attempt | New source-body bytes | Selected range bytes reused | Result |
| --- | ---: | ---: | --- |
| Retry after a later source failed | 2,478,134 | 453,889 | Rebuild both groups; exact references pass |
| Complete checkpoint reconstruction | 32 | 2,931,991 | Rebuild both groups; exact references pass |
| Restart after acquisition cancellation | 1,129,193 | 1,802,830 | Rebuild both groups; exact references pass |

Complete local checkpoints contain 53 range entries in 59 files and occupy
2,947,224 logical bytes. The 32 new bytes are two preparation and two final
validation proofs. Nothing appends replayed tiles to an earlier calculation.
There is no exactly-once claim, partial-output recovery or offline success.

Output uses the existing owned sibling staging and atomic no-replace rename.
A failed granule refuses the job; no earlier group is silently published alone.
Existing/racing destinations are preserved. Ordinary errors and cancellation
remove owned staging and keep checkpoints; abrupt death can leave a hidden
owned stage that is not reused or automatically swept. Existing file/directory
synchronization and device requests are retained. A failure after the rename
commit point leaves a complete destination and explicitly reports unconfirmed
synchronization. Cancellation/race/namespace checks pass on macOS and Linux
ARM64; power-loss durability remains unproven.

## Whole-job measurements

[Matched controls](measurements.json) run the same acquisitions as one two-entry
job versus two serial one-entry jobs through the **same durable scientific Zarr
writer**. Every stored array matches between batch and individual outputs, and
source-body traffic is identical. There are three local HTTPS repetitions with
1 ms added per source GET. Host load was not isolated; these are bounded local
observations, not a general speedup benchmark. No durable output is compared to
a no-write stream.

| Median / representative counter | Two-entry job | Two serial one-entry jobs |
| --- | ---: | ---: |
| Wall time | 0.253 s | 0.297 s combined |
| Process CPU | 0.118 s | 0.147 s combined |
| Source-response bodies | 2,932,007 bytes | 2,932,007 bytes combined |
| Physical HEAD + GET requests, including redirects | 101 | 101 combined |
| Logins | 4 | 4 combined |
| Native source-chunk decodes | 10 | 10 combined |
| Output files | 93 | 98 combined |
| Output logical bytes | about 1.872 MB | about 1.875 MB combined |

Median batch time to first completed observation is 0.105 s; median final
manifest/synchronization/publication time is 0.016 s. Reports retain
per-phase metadata/payload requests, auth/redirect counters, range bytes, source
read/decode time, writer calls/bytes, encode time, synchronization stages,
checkpoint usage, source reservations and sampled process RSS. Source
`read_decode_seconds` includes range retrieval/checkpoint access; it is not an
isolated decoder CPU benchmark. Writer library counts are not physical-device
traffic; header/TLS bytes are not measured. The source work is effectively the
same; this milestone's principal result is a usable bounded/recoverable workflow.

Bounded NASA acceptance used the two qualified selections only. The initial
write and first replay were followed by a final-binary reconstruction after
metadata hardening, without fetching new selected payload. All three runs
combined received **2,932,087 source-body bytes** and
**4,488 redirect-body bytes**, below the predeclared
16 MiB expected source-body bound. Complete live checkpoints occupy 2,950,794
logical bytes in 53 records. The final binary fetched only 32 proof bytes,
finished in 24.66 s, reached its first observation in
7.54 s and spent
10.23 s in fresh final
validation. Live auth/redirect latency is material; these changing-network runs
are not the matched performance comparison. Their actual binary hashes are
retained, and final reconstruction/independent equality use the final binary.
No additional NASA acceptance is needed for this gate.

## Verification and decision

[Verification receipt](verification.json) records exact commands, source/log
hashes, toolchains, native linkage and final binary identity. There are 69
passing native/authenticated Rust tests with five explicitly ignored cases,
strict all-feature and native-HDF5-without-auth Clippy checks, no-default-feature
compilation, Rust/Python formatting/lint checks, the original 32-case transfer
CLI regression, seven job integration tests and the new checkpoint-root test.
The latter eight focused tests also pass as UID 1000 on Linux ARM64 in the
existing pinned `rust:1.92-slim` image, with source/registry mounted read-only.
The final binary is linked without GDAL or libhdf5. Independent references use
their separate HDF5/NetCDF-C implementations.

The gate is complete: runnable plan/fetch, typed Rust workflow, readable Zarr,
exact science/provenance checks, aggregate admission, authentication/checkpoint
boundaries, failure/restart/publication evidence, matched durable-write
measurements and documented limits are present. No merge, push, tag, release,
external publication, deployment, ChronoZarr change, upstream patch or ownership/
licensing work is part of this task.

Remaining scope is deliberately narrow: at most four explicit sources and
positive index slices, the qualified PGD0 profile, one group per native grid,
serial execution, static disk quotas, complete output regeneration, and the
current exact-object archive access policy. Other processing releases,
resolutions/products, geographic selection and arbitrary HDF5 schemas are not
qualified by mission or filename resemblance. There is no output-byte/free-space
reservation, generalized persistent cache, aligned time-series model, distributed
scheduler or collection snapshot guarantee.

**Recommendation:** a bounded geographic/date discovery helper is the next
useful extension, as a separate producer of explicit candidate manifests.
Manual granule identification is now the main usability gap. Keep this job
contract and admission/publication path as the handoff, cap discovery results,
and keep native pixel-window derivation explicit until geographic-to-native
selection is separately qualified. More batch concurrency is not a prerequisite.
