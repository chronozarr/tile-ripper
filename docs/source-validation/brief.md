# Tile-Ripper — Efficient Source Validation Without Weakening Consistency

Continue on `codex/acquisition-planner` from commit `157facc`.

The geographic SWOT workflow now uses one immutable grid inspection per granule. On the controlled three-source benchmark, generation fell from 8.791 to 4.664 seconds and HTTPS requests from 244 to 104.

Scientific correctness, authentication confinement, source-version checks, checkpointing and atomic output publication remain intact.

The next measured opportunity is redundant canonical source validation, particularly HEAD requests performed during repeated object binding and preparation.

**Objective: Reduce unnecessary remote source-validation round trips while preserving every scientifically and security-relevant consistency guarantee.**

This is a narrowly scoped investigation and implementation milestone. Do not build a new cache framework or broadly redesign the acquisition planner.

## 1. Map the current validation lifecycle

Trace the complete lifecycle for:

- Native HDF5/SWOT objects.
- Native COG objects.
- Zarr v2 and v3 object collections.
- Authenticated and anonymous HTTP.
- Checkpointed and uncheckpointed acquisitions.

For each request, identify which operation performs it, what guarantee it establishes, and whether an earlier observation could safely satisfy that guarantee.

Distinguish:

- Authentication and permission verification.
- Canonical object identity.
- Object size and version binding.
- Method-specific signed-route resolution.
- Conditional payload integrity.
- Pre-execution version checks.
- Post-execution checks.
- Checkpoint authorization proofs.
- Final multi-source publication validation.

Produce a request-level lifecycle trace for one authenticated SWOT acquisition and one representative anonymous source.

Do not classify a request as redundant merely because its ETag matches an earlier request.

## 2. Identify safe opportunities for validation reuse

Investigate whether immutable, version-bound inspection or preparation state can retain sufficient verified information to avoid repeated canonical size/HEAD requests within a single logical operation.

Define precisely which observations may be reused and for how long.

Preserve the following invariants:

- Fresh checkpoint reuse requires live authorization.
- An expired signed route cannot be treated as valid.
- HEAD and GET may require different signed destinations.
- Conditional payload reads must validate exact ETag, range and length.
- A detected source mutation permanently invalidates that preparation.
- A changed source cannot be accepted through cached metadata.
- Final validation must still occur before successful stream completion or durable publication.
- Multi-object Zarr sources must not be treated as one atomic snapshot merely because one object validates.

Do not introduce a time-based identity cache without a demonstrated and explicitly reviewed consistency contract.

If a check cannot safely be removed, retain it and explain why.

## 3. Implement the smallest justified change

Prefer scoped reuse of already validated immutable metadata or identity records.

Avoid introducing:

- A process-global mutable object cache.
- Persisted signed routes.
- A general credential manager.
- Background validation.
- A distributed cache.
- Broad changes to the public streaming API.

Keep existing anonymous and authenticated behavior isolated.

The improvement should not require weakening ETag, authorization, checkpoint or final-completion checks.

## 4. Adversarial validation

Use local HTTPS fixtures to exercise:

- Mutation immediately after binding.
- Mutation between inspection and preparation.
- Mutation after provisional delivery.
- Mutation before final publication.
- Source size changes without expected ETag behavior.
- Incorrect or weak ETags.
- Expired method-specific signed routes.
- Authentication failure after earlier success.
- Changed source identities with reused locators.
- Changed multi-object Zarr metadata or payloads.
- Checkpoint replay with retained ranges.
- Concurrent preparations using the same canonical source.
- Cancellation and retry exhaustion.

A removed request must not create a new successful-completion path for an invalid or changed source.

Compare accepted and rejected operations against the prior implementation, using explicit fault expectations.

## 5. Benchmark the validated change

Use preserved binaries, pinned fixtures and identical acquisition selections.

Benchmark:

A. One authenticated SWOT source.

B. The existing three-source geographic workflow.

C. Anonymous COG acquisition.

D. Zarr v2/v3 sources with their actual per-object identity semantics.

Measure separately:

- Canonical HEAD requests.
- Conditional GET requests.
- Signed redirect resolution.
- Authentication requests.
- Metadata and payload bytes.
- Preparation and first-chunk latency.
- End-to-end completion.
- Checkpoint reuse.
- Working reservations and RSS.

Use repeated interleaved runs under zero, moderate and high latency.

Do not conflate fewer application-level HEAD calls with fewer network round trips unless the actual wire traces confirm it.

Include an explicit no-improvement outcome if the remaining checks are necessary or the gains are below measurement noise.

## 6. Preserve general scientific regressions

Repeat the qualified:

- SWOT geographic AOI and five-layer science gates.
- HDF5 stored-value and coordinate oracles.
- Native COG acquisition.
- Zarr v2/v3 and sharded acquisition.
- Authentication and secret-confinement tests.
- Transfer checkpoint/replay tests.
- Multi-granule scientific output and publication faults.
- Feature-matrix and clean GDAL-free builds.

Keep the existing full-index HDF5 validation default.

Do not modify the validated SWOT geometric inclusion or quality policies.

## 7. Final decision gate

Deliver:

1. A precise validation-lifecycle diagram and request accounting.
2. A list of necessary versus safely reusable checks.
3. The implemented improvement, if justified.
4. Before/after wire-request counts and source traffic.
5. Controlled performance results.
6. Authentication, source-mutation and final-completion evidence.
7. Memory and API implications.
8. Remaining unavoidable provider latency.
9. A recommendation on whether further transport optimization is worthwhile.

Commit validated work locally.

Do not merge, push, tag, publish, deploy, modify ChronoZarr, add scientific formats or submit upstream changes.

**Stop if the remaining validation round trips are necessary. Do not trade source consistency or credential security for a benchmark speedup.**

After this milestone, prioritize evaluating Tile-Ripper as a complete scientific acquisition product rather than continuing format-specific micro-optimization indefinitely.