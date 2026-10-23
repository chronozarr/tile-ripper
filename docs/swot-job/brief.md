# Tile-Ripper — Bounded Multi-Granule SWOT Acquisition and Scientific Output

Continue from commit `7e7aa93` on `codex/acquisition-planner`.

The original and held-out SWOT Raster D PGD0 selections now pass independent
h5py and NetCDF-C verification through the same native reader, authentication
policy and explicit good/suspect quality policy.

Authenticated source-byte checkpointing works through the normal CLI and Rust
API. Scientific streams restart from the beginning. Checkpoint filesystem
behavior has focused macOS and Linux ARM64 validation.

The held-out reference failure was incomplete capture metadata, not a native
scientific-decoding defect. Preserve the corrected capture guards and original
failure evidence.

## Objective

Build a useful, bounded acquisition job:

A caller supplies several supported source granules and selections, Tile-Ripper
acquires them with existing authentication and transfer recovery, and the CLI
writes one independently readable scientific Zarr dataset.

This is integration of established capabilities—not a new format reader,
distributed scheduler, general catalog service or SWOT analysis package.

Do not begin another broad research or release-preparation campaign.

## 1. Establish the smallest batch contract

Inspect the existing Request, Prepared, streaming, authentication, checkpoint
and scientific-output interfaces.

Use a small explicit manifest whose entries identify:

- Canonical source product.
- Dataset/coordinate selections and strides.
- Explicit quality policy.
- A stable observation identity.
- Any caller-supplied expected source version.
- Relevant resource limits.

Reuse existing request structures where practical.

Begin with the already qualified product/profile and two to four entries.
A one-entry job must remain valid.

Define duplicate-entry behavior and deterministic ordering. Do not silently
merge repeated observations or assume equal timestamps imply equal data.

Do not add CMR discovery, geographic polygon selection for SWOT, reprojection,
resampling or mosaicking in this milestone.

## 2. Coordinate execution within an aggregate budget

Reuse the existing scheduler and admission machinery.

Account for simultaneously active:

- Decoders and encoded-range buffers.
- Prepared metadata and coordinate arrays.
- Authentication sessions.
- Checkpoint indexes and write buffers.
- Output-writer buffers.
- Queued or consumer-retained data.

Do not multiply a per-granule budget by the number of active granules without
reporting and enforcing an aggregate bound.

A conservative serial execution path is acceptable initially. Add bounded
parallelism only where the existing architecture supports it cleanly and
measurements justify it.

Retain per-source memory diagnostics. When a job cannot fit, identify the
limiting component rather than silently increasing the user's budget.

Keep working reservations distinct from measured process RSS.

## 3. Preserve per-object authentication and checkpoint boundaries

Create explicit caller-owned sessions for each canonical source under the
existing qualified access policy.

Do not broaden credential forwarding or redirect allowlists to make batching
convenient.

Use separate version-bound checkpoint namespaces per acquisition identity.
Enforce explicit aggregate disk and entry limits across the job.

Document whether limits are allocated per source or shared. No unbounded
persistent cache, eviction daemon or automatic unrelated-file cleanup.

On restart:

- Reauthenticate and revalidate each reused source.
- Reuse only verified source ranges.
- Restart scientific delivery and consumer calculations.
- Do not append replayed results to an old calculation.
- Do not claim exactly-once processing or partial-output recovery.

It is acceptable to regenerate output from checkpointed source bytes.

## 4. Reuse the scientific Zarr writer

Inspect the existing COG scientific-output adapter and generic writer facilities.

Extend them narrowly where needed for HDF5-derived observations containing:

- Float32 WSE.
- UInt8 summary quality.
- UInt32 bitwise quality.
- Float32 uncertainty.
- UInt32 contributing-pixel counts.
- Exact x/y coordinate values.
- Source masks and a separately identified derived quality mask.

Preserve stored values and bit patterns. Do not silently calibrate data,
cast integer flags to floats, or replace raw auxiliary values with masked ones.

Retain source units, fills, attributes, CRS, processing identifiers, times,
quality definitions and per-object provenance.

Use the existing reliable Zarr v2 output path unless a concrete blocker is
demonstrated. Do not change v3 defaults or create a new storage format.

Keep one explicit native-grid observation group per granule. Different
projections, resolutions, extents and dimensions must remain distinguishable.

Do not invent an AOI mask for an index-only selection or label a derived
quality mask as source validity.

## 5. Define job completion and publication

Default behavior should be all-or-nothing at the requested destination:

- Work in owned staging.
- Require complete delivery and successful final validation for every
  required observation.
- Record per-source receipts and the job's completion status.
- Publish through the existing checked no-replace mechanism.
- Never silently omit a failed granule.

Keep completed source checkpoints available for a retry when a job fails,
subject to the documented ownership and cleanup policy.

A restart may rebuild the output from source checkpoints. Do not implement
persistent completed-observation reuse unless it is genuinely necessary.

Specify the timing and scope of final source revalidation. Test a previously
read source changing before job completion.

Do not imply that individually version-validated objects constitute an
atomic snapshot of a changing NASA collection.

Retain existing distinctions between process-interruption safety,
synchronization requests and unproven power-loss durability.

## 6. Provide a normal CLI and Rust workflow

The user should not need to write Rust or process base64 NDJSON to obtain
the dataset.

Integrate with existing plan/fetch conventions. A target experience is:

    tile-ripper plan job.json ...
    tile-ripper fetch job.json ... --checkpoint ./checkpoint \
        --output ./swot-subsets.zarr

This is illustrative syntax. Verify the actual implemented interface and
document runnable commands.

Authentication must remain explicitly opted in. Request/manifest files must
not contain credentials, cookies or signed URLs.

The plan should describe:

- Selected sources and logical windows.
- Native grids and variables.
- Intended output groups.
- Resource requirements and admission limits.
- Checkpoint behavior.
- Unsupported requests and known costs.

Provide a typed Rust example using the same implementation.

## 7. Independent scientific validation

Use the original and held-out qualified selections for the first end-to-end job.

They are useful heterogeneous observations; do not present them as an aligned
time series at one location.

Open the completed Zarr independently with Zarr-Python and xarray.

Compare every stored value, mask, coordinate, time field, quality definition
and source identity against the guarded h5py and NetCDF-C references.

Preserve strict capture-coverage checking. Missing reference bytes must never
silently become apparent source zeros.

Test:

- One and multiple observations.
- Different native dimensions and CRS.
- Input entries supplied out of time order.
- Empty and all-invalid selections.
- Incompatible source schemas or policies.
- One source failing after another has delivered data.
- Source mutation before final job publication.
- Cancellation during acquisition and writing.
- Restart from checkpoints.
- Existing output destinations and publication races.
- Small aggregate memory/disk budgets.
- Secret redaction and credential isolation across sources.

Do not claim support for additional products solely because they share a
mission or filename pattern.

## 8. Measure whole-job behavior

Compare the batch workflow with running the same supported acquisitions
individually.

Measure acquisition and durable output separately where possible:

- Metadata and payload requests.
- Authentication and redirect traffic.
- New and reused source bytes.
- Decoder and writer work.
- Time to first observation and total completion.
- Checkpoint disk usage.
- Output files and bytes.
- Working reservations and process RSS.

Use pinned mirrors/local HTTPS fixtures for repeated tests. Keep live NASA
acceptance bounded.

Do not compare durable output with a no-write stream as though both perform
the same work.

Do not make a speedup a prerequisite if the principal result is a correct,
usable, recoverable multi-observation workflow.

## Decision gate

Deliver:

1. A runnable multi-granule plan/fetch CLI workflow.
2. A corresponding Rust API example.
3. An independently readable scientific Zarr dataset.
4. Exact scientific and provenance verification.
5. Aggregate memory, disk and authentication boundaries.
6. Job failure, restart and publication evidence.
7. Matched whole-job measurements.
8. Remaining limitations.
9. A recommendation on whether geographic/date discovery is the next useful
   extension.

Commit validated implementation and evidence locally.

Do not merge, push, tag, publish, deploy, modify ChronoZarr or submit upstream
patches. Do not reopen ownership/licensing work in this engineering task.

Success means an outside scientist can acquire several supported observations,
recover interrupted transfers, and open the completed scientific dataset using
ordinary analysis tools—without writing a custom acquisition program.