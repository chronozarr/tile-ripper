# ADR 0004 — Same-grid time stacking and the Chronozarr bridge

Status: local implementation and review fixes, 2026-10-28. Unreleased.
The original stacking commit is `dffca42`; its parent is `e98c448` in the
PR #3–#6 dependency chain. This document describes the current local behavior,
not the merge or release status of those dependencies.

## Purpose

The scientific COG writer keeps one `[time, y, x]` group per source asset/band.
A same-grid time series therefore needs explicit assembly before analysis or
playback. This is a COG-output convenience gap: other Tile-Ripper writers already
produce time-series Zarr, and the materialized GFS subset has an existing
[Chronozarr conversion receipt](icechunk/tile-to-chronozarr.json).

Two consumers now have different paths:

- `tile-ripper stack INPUT OUTPUT` writes a reusable `[time, band, y, x]` cube
  when the scientist needs a persistent scientific stack.
- Chronozarr's checkout-only `convert_tile_ripper` Python function reads the
  original COG observation output and feeds its existing encoder directly. It
  joins a selected QA mask to the matching science scene without first writing
  that extra cube. See the sibling Chronozarr checkout's `docs/tile-ripper.md`.

The qualified acquisition writer is unchanged. Neither path resamples,
reprojects, mosaics, composites, deduplicates acquisitions or changes stored
calibration. No new Rust encoder, product profile or general plugin system is
introduced. Product expansion remains held pending a separate acquisition use case.
The accompanying evidence PR records the matched HLS comparison.

## Persistent scientific stack

The stack key is the exact collection, CRS text, transform bits, window shape,
dtype, nodata, scale/offset/units and sorted set of derived quality-mask names.
Bands within a key follow input request order; times increase at millisecond
resolution. Equal timestamps from distinct observations, missing bands or a
varying AOI mask refuse that stack with the offending identities reported.
Unrelated valid stacks may still be written. Unsupported undated or annual
summary observations are reported separately. Corrupt or contradictory metadata
is an integrity error, not an unsupported observation to quietly omit.

Literal CRS and bitwise geometry matching deliberately admit only exact windows.
Equivalent CRS spellings, adjacent tiles, partial-scene windows and different
native grids can form separate stacks. A shared grid does not imply comparable
SAR orbit/burst geometry or processing semantics; use source identities and an
explicit observation selection. No automatic tolerance or cross-orbit merge is
implied by this command.

Scale and offset come from the written `values` attributes, including the
writer's STAC fallback. The persistent stack uses one CF calibration per values
variable and therefore splits bands with different calibration. This is an
intermediate-layout restriction, not a Chronozarr format requirement: the direct
Python bridge supplies separate band descriptions with per-band calibration.

Calibration reports describe actual adjacent changes in chronological order.
If calibration A returns after B, the report includes A-to-B and B-to-A
transitions; it does not pretend that the calibration-key stacks are disjoint
chronological intervals. Concurrent calibration variants are ambiguous and must
not be presented as a chronological boundary.

```
stack.zarr/
  zarr.json                    complete root; stack_version and summary
  manifest.json                complete stack_result and input summary
  stacks/{k}/
    zarr.json                  consolidated metadata for this stack
    values       [time, band, y, x]  native dtype, lossless zstd
    valid        [time, band, y, x]  copied per-band validity
    valid_all    [time, y, x]        AND of valid over bands
    aoi_mask     [y, x]              time-invariant AOI inclusion
    quality_*    [time, band, y, x]  masks belonging to these raw observations
    time, band, y, x, spatial_ref
```

Every observation retains its source descriptor. Earth Search's
`object_identity.etag` and product `object_version` are both understood; the
original identity fields and processing metadata remain available. The input
path and manifest length in the stack summary are location/size information,
not a cryptographic identity or a payload checksum. The direct Python bridge
also records the input manifest SHA256; that hashes metadata, not all chunks.

The input is read only. Output must be a new directory outside the input tree,
including through path aliases. Staging and publication reuse the existing
transaction: checked writes, directory synchronization, device barriers and an
atomic no-replace rename. The filesystem store uses the same deferred macOS
device-flush option as the qualified scientific writer.

Copying uses checked working-buffer admission and bounded spatial tiles.
`--buffer-mib` controls the reservation; it is not a process RSS promise.
Each encoded source object is capped at one eighth of that reservation (8 MiB
with the default 64 MiB buffer), and encoded buffers are charged alongside
decoded chunks. Source reads reuse fixed-output gzip/Zstd decoding and checked
Blosc headers. Unsharded raw/gzip/Zstd/Blosc are admitted; unsafe filters and
sharded source layouts fail explicitly.
Cancellation is checked during AOI validation and copying. The preflight plan
reads metadata and coordinate arrays from a completed fetch output. AOI-mask
equality is checked when writing, after those masks are read.

```sh
tile-ripper stack s2-fetch.zarr --plan
tile-ripper stack s2-fetch.zarr s2-stack.zarr --buffer-mib 64
```

## Chronozarr output and quality policy

The direct Python bridge is the preferred playback path for these observation
outputs:

```python
from chronozarr import convert_tile_ripper

convert_tile_ripper(
    "hls-fetch.zarr",
    "hls-playback",
    bands=["B04", "B8A"],
    quality_policy="hls_fmask_clear",
)
```

The HLS fetch must include Fmask and the selected quality policy. The derived
mask belongs to the UInt8 Fmask observation, while reflectance is Int16. They
must be joined by scene identity, processing metadata, exact grid and time;
matching dtype or merely repeating `--mask-var` cannot establish that join.
The bridge refuses missing or ambiguous QA and combines its accepted plane
with validity/AOI inclusion across the selected science bands. With no quality
policy, clouds remain data. The persistent stack's four-dimensional quality
arrays are not directly interchangeable with the generic converter's shared
`[time, y, x]` mask argument.

The direct bridge initially admits the ordinary uncompressed, C-order,
unfiltered Zarr v2 COG output. Other encodings or writer schemas fail explicitly;
the converter does not invoke an unbounded compressed-source decoder under a
declared working-buffer reservation.

A quality-free scientific stack can still use the generic converter:

```sh
chronozarr convert s2-stack.zarr/stacks/0 s2-playback \
  --variable values --mask-var valid_all
chronozarr validate s2-playback
```

The generic converter does not automatically carry the stack's source
descriptors into final provenance. Use the direct bridge on the original fetch
output, or pass explicit `provenance=` through the Python conversion API when
using this stack route and retaining source pins in the playback artifact.

The stack is an intermediate ordinary Zarr store, not a conforming Chronozarr
store by itself. Its CF scale attributes are intentional. Conversion writes
Chronozarr metadata, geometry, band descriptions, a shared mask and overview
levels. Matching nominal chunk edges does not eliminate decoding, re-encoding
or converter staging/spill I/O. Measure those costs together with acquisition.
For categorical class codes, explicitly use one level rather than arithmetic
mean overviews. The direct bridge preserves invalid Float32 NaN sample bits;
the generic converter instead replaces NaNs with fill under its mask.

## Qualification boundary

Named nonannual product fetches remain limited to 31 days and 32 observations,
and `stack` consumes one completed output. Longer archives and other writer
schemas are not implied by this bridge. The accompanying evidence PR records
retained real-data readback, browser checks and the matched HLS comparison.
