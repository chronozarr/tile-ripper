# OPERA product expansion qualification

This local candidate adds three separate native COG profiles. Values, native grids, source fill, calibration metadata and requested classification masks remain separate. No product algorithm, confidence threshold, reprojection or cross-observation alignment is introduced.

| Profile | Delivered scope | Qualification |
| --- | --- | --- |
| `opera-dswx-ni-prototype` | Four UInt8 layers from one ETag-pinned public DSWx-NI v0.1 engineering specimen | [Three AOIs, independent raw bits/masks/coordinates, stream and reopened Zarr](dswx-ni-qualification.json) |
| `opera-dist-alert-s1` | Ten production v1.0 layers, including UInt8 classes, Int16 dates/duration and Float32 metrics with NaN fill | [Authenticated source and output evidence](dist-s1-cmr-receipt.json); [details](dist-s1.md) |
| `opera-dist-ann-hls` | All 21 annual v1 layers, including previous-year status classes | [All-layer independent arrays/masks/coordinates/time bounds and completed stream](dist-ann-qualification-receipt.json); [source evidence](dist-ann-source-receipt.json) and [scope](dist-ann.md) |

DSWx-NI remains an explicitly pinned prototype. It has no production discovery route and does not qualify final NI v1.0. Its class-8 policy is separate from DSWx-S1. Context classes remain raw values; no unobserved class is claimed present in the checked windows.

Annual output retains the actual provider coverage interval and summary year. Its time coordinate labels coverage start; CF `time_bounds` stores the unchanged provider endpoints, including fractional seconds. Selection uses interval intersection. Supplied annual intervals and asset references are checked against the exact live CMR record before acquisition. Date layers use `days since 2020-12-31`; duration layers use `days`.

The annual example uses an explicit 128 MiB buffer for 21 authenticated source transports. The default 32 MiB request correctly fails its transport reservation guard; Tile-Ripper never silently increases a caller's budget. The original annual receipt covers northern T17SNU. Subsequent [southern qualification](../dist-ann-southern/README.md) covers T20LRP’s provider-declared EPSG:32620 with negative northings through a scoped annual-only admission path; original receipts below are not rewritten.

## Reproduce

Build the authenticated native binary:

```sh
cargo +1.92.0 build --locked --release --no-default-features \
  --features native-cog,native-hdf5,authenticated-http \
  --bin tile-ripper --example array_faults --example array_product
```

The [examples](../../../examples/opera-dist-ann-hls.json) use the ordinary `plan`, `stream --values` and `fetch --output NEW.zarr` commands. DIST archive access requires explicit `--auth earthdata --netrc /private/path/.netrc`; credentials are sent only to the configured login origin and are absent from tracked evidence. [Pinned Python dependencies](reference.txt) support the independent qualification harnesses under `bench/`.

Production DIST-S1 uses Zstandard COG compression. A [scoped TIFF dependency patch](../../../vendor/tiff-reader/TILE_RIPPER_PATCH.md) adds one-shot decoding with fixed output/context reservations and rejects corrupt, oversized, truncated or concatenated frames. Integer and floating predictors, NaN payloads and raw bits have controlled independent fixtures. [Dependency audit](dependency-audit/vendor-audit.json).

[Failure checks](failure-qualification.json) cover wrong ETag pins, a false annual interval and non-overlapping annual selection, with no published or staged output. An ETag conflict detected during identity HEAD is distinguished from a conditional data-read failure. Existing transaction and changed-source tests remain separate shared gates.

[Acceptance](acceptance.json) binds the engine, tests and examples to new check receipts. Earlier qualification receipts remain historical. This work has no release, merge, deployment or ChronoZarr change.
