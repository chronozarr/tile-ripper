# Review package — 28 October 2026

These changes are proposed in draft PRs. No merge, release or deployment has
occurred. Original dirty checkouts and experiment artifacts remain intact;
packaging uses separate review worktrees.

## Review order

| Change | PR and target | Boundary |
| --- | --- | --- |
| Acquisition hardening | [Tile-Ripper #7](https://github.com/chronozarr/tile-ripper/pull/7), targets `codex/icechunk-acquisition` | Depends on existing #3–#6; source identity, transport, bounded selection, signing and inspected HLS native-grid handling |
| Scientific stack | [Tile-Ripper #8](https://github.com/chronozarr/tile-ripper/pull/8), targets #7's branch | Exact-grid assembly, checked decoding, cancellation, provenance and atomic completion |
| Evidence and docs | `codex/hls-benchmark-evidence`, targets #8's branch | Reproducible tools, unchanged historical receipts, current comparison and qualification boundaries |
| Playback bridge | [Chronozarr #145](https://github.com/chronozarr/chronozarr/pull/145), targets `main` | Independent of Icechunk #143; checkout-only Python API and explicit scene/QA joins |

Review native changes in dependency order; these are not independent patches
against main. The Chronozarr API is not in the published 0.5.1 package. No
version or release metadata changes are included.

## Local checks

The final native source passes 221 native-COG all-target tests (3 existing
ignored), 321 all-feature all-target tests (9 existing ignored), and 164
Zarr-only all-target tests (1 existing ignored). Strict all-feature/all-target
Clippy and rustfmt pass. Rust 1.91 native-full passes 283 tests (8 existing ignored), checking the
older baseline. Feature runs are serial and use an isolated target directory.
Independent read-only reviews found no blocking issue in either code scope.

The main-based Chronozarr branch passes all 2,256 existing unit tests. A final
targeted run passes 116 tests, including all 38 new bridge regressions under
the CI unit marker and documentation/version checks. A combined coverage run
records the final total. Ruff, formatting, ty with the project environment,
and architecture checks pass. Wheel/source distributions build, tests collect
from the isolated archive, and the wheel exports `convert_tile_ripper`.
Local Python is 3.13; Python 3.11 remains a separate CI job.

Both documentation sites build and check internal links. CI exposed existing
documentation dependency audit findings. Native docs pin patched
`@hono/node-server` 2.1.3; Chronozarr also pins `@vanilla-extract/css` 1.21.3,
removing its vulnerable deepmerge dependency. The seven-day package quarantine
remains enabled and the audit gate remains required. Scientific acquisition
and encoding algorithms are unaffected by these documentation fixes.

Benchmark Python lint/format and JavaScript syntax checks pass. The scene-pool
fixture verifies sixteen-way cross-scene acquisition, bounded retention,
chronological output and terminal failures. The C interposer compiles with
strict warnings. A fresh loopback fixture verifies 16 reads, 96 matched request
starts/ends, exact body endpoints, independently observed concurrency of 16,
credential redaction, and zero issues or pending transfers.

## Packaged bridge readback

The main-based PR code converts the retained 30-day HLS observation output:
eight dates, 32 pinned assets, B03/B04/B8A plus separate Fmask, explicit
`hls_fmask_clear`, native 333 by 332 geometry. This is offline readback.

All **3,318,672 samples** and **1,106,224 masks** across both levels match the
original timed result exactly. All 32 full source descriptors and manifest
SHA256 survive; an input-tree fingerprint is unchanged. The resulting store
validates. Static HTTP/Chromium rendering and raw/physical queries pass all
eight dates, including invalid flags on the two fully masked dates. Browser
errors: zero; external traffic blocked. The screenshot was inspected.

Logs, readback, distribution artifacts and browser proof are retained under
`prototype-output/pr-packaging-20261028/`. Historical receipts remain unchanged
and bind the original measured source/binary and their then-local publication
status. Packaging does not retime or retroactively rewrite those experiments.
The [machine-readable packaging receipt](pr-packaging-receipt.json) binds these
source hashes and readback results. The latest conclusion remains the [matched sixteen-way result](hls-river-parallel16.md):
Python 4.66 s versus native 5.06 s, lower native sampled RSS and an extra archive.

## CI and adoption

Each PR's live checks are the authority for its head. Local success does not
imply pending Linux, browser or Python-version jobs have passed. The docs audit
gate remains required. Exact grids, QA, pinning, bounded decoding and transaction
guarantees remain explicit. Product expansion stays held pending a concrete
acquisition use case. No merge, release or deployment is authorized by this record.
