# Stack and playback review fixes — 28 October 2026

The draft acquisition and stack PRs plus the independent Chronozarr bridge PR
package the review of `dffca42` and the original sibling checkout at `6fb85ac`.
These changes remain unreleased. [The acceptance record](acceptance.json) binds the source
hashes, tests and retained-capture results. That first review-fix phase reused
local captures and performed no network acquisition, publication or human trial.

The stack now retains real Sentinel-2 and product pins, rejects contradictory
metadata, reports chronological calibration changes, checks source decoding and
working-buffer limits, responds to cancellation, protects input paths and
publishes consistent completion metadata. The existing optimized transaction
and guarded decoders are reused.

Chronozarr's direct `convert_tile_ripper` function joins a selected QA observation
to its science scene and uses the existing encoder without a persistent stacked
copy. It preserves source descriptors, per-band calibration and raw sample bits,
including invalid NaNs. Source-valid infinities remain valid unless explicit QA
excludes them. The first bridge admits ordinary raw C-order, unfiltered v2 COG
outputs; unsupported input encodings fail explicitly.

The initial retained-capture validation passed 64 Rust library tests, 25 independent Rust regressions and
325 Python tests (including 38 bridge regressions), plus strict Clippy, Ruff,
formatting and module/new-test type checks. Initial independent tests reproduced
nine defects in the original implementation. One pre-existing Clippy assertion
was also corrected using the equivalent `contains` form.

Retained real data passed independent byte, mask, scale, unit, geometry,
provenance and input-immutability checks:

- HLS S30 B04 plus separate Fmask: 784 physically valid pixels became 3 after
  the clear policy; 781 were correctly excluded by QA.
- Sentinel-2 B04/B08: two dates on a 113 × 91 native grid passed the fixed Rust
  stack and generic conversion with explicit provenance. The legacy compressed
  capture has an absent null-fill CRS scalar chunk and is correctly refused by
  the direct importer; that placeholder carries no science measurement.
- Both stores opened through local static HTTP in Chromium. Temporal navigation,
  raw and physical click-to-query values passed with zero browser errors. Every
  nonlocal browser request was blocked.

`bench/stack_review_bridge.py` repeats the independent source/output checks;
its optional `--stack` argument checks an already written stack subgroup using
generic conversion with explicit provenance. `bench/stack_review_viewer.mjs`
checks those receipts in the viewer. Local stores, receipts and screenshots are
under `prototype-output/stack-review-20261028/` and are ignored by Git.

The first live [matched 30-day HLS river analysis](hls-river-30day.md) completed
live acquisition and three trials per path. Values, QA and playback agree;
ordinary Python took 12.61 s versus Tile-Ripper's 18.82 s. Tile-Ripper used less
sampled memory and retained an extra raw archive. That run also fixed signing
rate limits and the inspected southern HLS grid convention. Product expansion
remains held: these results establish correctness and a playback path, without
a speed or measured human usability advantage for Tile-Ripper.

The later [performance follow-up](hls-river-performance.md) reduced native runtime
25.4% in interleaved old/new controls while preserving complete source descriptors
and all science values. That historical Python comparison favored Python:
13.70 s versus optimized Tile-Ripper's 17.47 s. All 213 native tests and the
repeated independent readback/browser checks passed; three existing fixture
tests remain skipped. Original measurements and outputs remain separate.


## Current comparison and review package

The latest [sixteen-way cross-scene comparison](hls-river-parallel16.md) gives
both routes sixteen actual concurrent HTTP requests and sixteen metadata/asset
workers across scenes. Python took **4.66 s median**, versus **5.06 s** for native
acquisition plus the bridge (three fresh processes each). Native retained lower
sampled peak process-tree RSS, **209 versus 278 MiB**, and an additional raw
observation archive. Earlier speed leads used different Python scheduling and
are historical controls. The roughly 0.40-second gap on public infrastructure
is not a general speed or adoption result.

All nine timed outputs matched the original values, masks and analysis;
complete source identity and QA checks passed. HTTP response intervals and
payload bytes agree with the preceding experiment. The linked report and
receipt record exact denominators, commands and limitations. Product expansion
remains held pending a concrete need for native acquisition or the extra archive.

The [PR packaging validation](pr-packaging.md) records current-checkout tests,
the independent bridge PR, dependency order and a fresh offline HLS readback.
Historical receipts remain unchanged and describe the source hashes and local
publication status at the time of each experiment. The current work is proposed
for review; it has not been merged, released or deployed.
