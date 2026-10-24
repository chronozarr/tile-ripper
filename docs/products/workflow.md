# Native scientific product workflows

Build the portable native path (the global Cargo default remains GDAL):

```sh
cargo build --release --locked --no-default-features --features native-cog
./target/release/tile-ripper products
./target/release/tile-ripper discover --product landsat-c2-l2-sr \
  --bbox -78.95,35.95,-78.94,35.96 \
  --start 2024-06-29T00:00:00Z --end 2024-06-29T23:59:59Z \
  --bands SR_B4,SR_B5,QA_PIXEL --limit 8
./target/release/tile-ripper plan examples/product-landsat.json --gdal-free --human
./target/release/tile-ripper fetch examples/product-landsat.json --gdal-free --output NEW.zarr
./target/release/tile-ripper stream examples/product-landsat.json --gdal-free --values
```

The same `plan`, `fetch`, and `stream` commands accept product requests; the ordinary low-level request and SWOT job schemas remain available. Product commands select the native COG backend explicitly. A dual-feature build still links GDAL; `--gdal-free` enforces its absence. Zarr v2 and native Rust chunks are the admitted product outputs. Unsupported physical types/codecs/layouts fail with a specific reader error, without a GDAL fallback.

## Request and observation selection

```json
{
  "product": "landsat-c2-l2-sr",
  "aoi": {"kind": "bounds", "crs": "EPSG:4326", "bbox": [-78.95,35.95,-78.94,35.96]},
  "start": "2024-06-29T00:00:00Z",
  "end": "2024-06-29T23:59:59Z",
  "bands": ["SR_B4", "SR_B5", "QA_PIXEL"],
  "quality_policies": ["landsat_pixel_clear"],
  "max_observations": 8,
  "buffer_bytes": 33554432,
  "chunk_edge": 256
}
```

`products` enumerates canonical names, mission-specific aliases, storage types, calibration, catalog/access requirements, and policy definitions. HLS requests use `hls-l30` or `hls-s30`, with `B04` and `Fmask`; these remain distinct profiles and observations. S30 `B08` and `B8A` retain distinct spectral meanings. Landsat surface temperature is outside this SR profile. Landsat 4–7 require their own QA/science admission.

Without `observations`, discovery uses Planetary Computer STAC for Landsat and freshly resolves the exact CMR collection short-name/version for NASA products. The full bounded search must fit a single page; exceeding a cap fails with instructions to narrow the query. A request permits 1–32 observations, 1–16 bands, a date interval of at most 31 days, and a WGS84 AOI no larger than 30 degrees per axis / 400 square degrees. It has no antimeridian wrapping. Missing requested assets, ambiguous catalog links, duplicate bands/observations, timestamps outside the interval, unknown policies, or a policy without its raw layer fail explicitly.

To use a selected catalog result, add its `observations` array to the request. An explicit empty array is an empty selection and never starts another search. Explicit observations contain `id`, `datetime`, `collection`, nullable `platform`/`processing`, and `assets` keyed by canonical native band name. An asset contains `href`, optional `etag`, and optional source `metadata`. Local mirrors are accepted and retain local size/mtime binding; remote objects require strong ETags and before/after identity validation. Profiles cannot make a physically incompatible object readable.

The public NASA example requests pin actual official reference products. Their acquisition dates can predate production archive coverage. See [qualification](qualification.md) for the precise sample and production-access distinction. CMR discovery alone does not authorize a protected asset download. Generic S3 inputs, requester-pays setup, and Earthdata credentials are never inferred from a family name.

## Scientific output

Each source asset/band has its own `observations/000000` Zarr group, with native `values`, `x`, `y`, `time`, `spatial_ref`, `valid`, and `aoi_mask`. Product, release, granule, acquisition timestamp, band, radiometry, calibration, original source metadata, object version, and applied policies remain in provenance. Open raw values with `xarray.open_zarr(..., group=..., decode_cf=False)`; xarray's default CF decoding intentionally applies scale/offset and missing-value decoding.

Stored UInt8, UInt16, Int16, and Float32 values are preserved. Float32 NaN payloads are retained in admitted lossless TIFF decodes and Zarr writes. No implicit reflectance conversion, decibel conversion, alignment, reprojection, resampling, compositing, band harmonization, or index calculation occurs. The current product gate requires north-up 30 m WGS84 UTM imagery; polar projections are described by some specifications but have not been admitted here.

`aoi_mask` records geographic pixel-center inclusion. `valid` combines AOI inclusion, physical source missingness, and documented product fill semantics; it does not mean cloud-free, water, or geometrically unaffected radar. For sources with a finite nodata sentinel, excluded envelope centers carry that sentinel; with no finite sentinel (including Float32 NaN), their stored values remain intact and the AOI mask excludes them. The output never invents a source fill code.

A requested closed policy produces `quality_POLICY` only in the observation containing its required raw QA/classification/radar layer. Those arrays combine the named policy with AOI inclusion, physical source validity, and product semantic validity. They do not silently mask another band or observation. This keeps raw classification labels distinct from water-membership masks and QA words distinct from cloud acceptance. To combine a spectral band with its QA group, explicitly verify matching observation, CRS, transform, coordinates, and window. The policy's exact definition is stored with the derived array. Unknown codes fail before publication; raw acquisition without a derived policy remains possible.

Streaming emits provisional chunks followed by an explicit completion record. `--values` includes base64 little-endian stored bits, AOI masks, and selected derived policy masks. Rust callers use `products::prepare` (or explicit `prepare_earthdata_authenticated`), the existing `Prepared::execute` / `into_stream` / `into_stream_cancellable` lifecycle, and `products::source_validity` / `products::derive_quality_mask`; combine these with explicit AOI/physical-source validity and only the policy's required raw layer. Check successful stream completion before accepting any result. Dropping/cancelling a stream invalidates provisional results. Product fetch uses the existing bounded incremental writer, cancellation flag, final source validation, staging directory, and atomic publication; existing destination paths fail.

## Access boundaries

Landsat uses the no-cost Planetary Computer route. Delegation signing is performed only for canonical Landsat C2 Azure objects, stays in runtime memory, and is omitted from inspection/output provenance. Signed URLs can expire; a failed acquisition must be prepared again. No long-lived token is saved in a request or plan.

NASA archive discovery is public. Current production HLS, PO.DAAC DSWx-HLS, and ASF RTC-S1 HTTP/S3 access have distinct Earthdata policies. HLS L30/S30 v2, current DSWx-HLS v1.1 and RTC-S1 v1.0 each have a separately qualified exact-object COG policy. The product helper selects the matching archive constructor; these policies do not inherit the SWOT HDF5 route. Build `--no-default-features --features native-cog,authenticated-http` and use:

```sh
cargo build --release --locked --no-default-features --features native-cog,authenticated-http
./target/release/tile-ripper fetch docs/products/hls-l30-authenticated-request.json \
  --auth earthdata --netrc PRIVATE_FILE --gdal-free --output NEW-HLS.zarr
```

The [current production receipt](hls-auth-validation.json) pins both L30/S30 sources. `bench/wave1_hls_auth.py` independently verifies fresh or existing outputs, with an explicit private netrc argument and no signed URLs persisted. The public GSFC HLS reference tiles and JPL OPERA sample bucket also permit credential-free qualification. Current PO.DAAC DSWx v1.1 and ASF Cumulus RTC v1.0 are also qualified with distinct observed redirect/resource hosts. Older ASF `datapool.asf.alaska.edu` routes remain outside the qualified authentication policy; use a current CMR Cumulus record or the frozen request below. Source reads remain conditional and bounded. No whole-file fallback or requester-pays setup is implicit.

```sh
./target/release/tile-ripper fetch docs/products/dswx-authenticated-request.json \
  --auth earthdata --netrc PRIVATE_FILE --gdal-free --output NEW-DSWX.zarr
./target/release/tile-ripper fetch docs/products/rtc-authenticated-request.json \
  --auth earthdata --netrc PRIVATE_FILE --gdal-free --output NEW-RTC.zarr
```

The [DSWx production receipt](dswx-auth-validation.json) verifies WTR, DIAG, SHAD and CLOUD; the [RTC production receipt](rtc-auth-validation.json) verifies a current Sentinel-1C VV/VH/mask burst crossing valid and source-fill pixels. `bench/wave1_dswx_auth.py` and `bench/wave1_rtc_auth.py` provide independent, explicit-netrc rechecks. DSWx’s catalog version and exact per-observation processing release are distinct; see [scientific definitions](specifications.md).

## Reproduce acceptance

The existing `prototype-output/zarr-env` has rasterio, NumPy, Zarr-Python, xarray, pyproj, shapely, and requests; alternatively create an environment using `bench/wave1-requirements.txt` (the extra TIFF parser is for independent inspection only).

```sh
cargo build --locked --no-default-features --features native-cog
prototype-output/zarr-env/bin/python bench/wave1_nasa_probe.py capture
prototype-output/zarr-env/bin/python bench/wave1_products.py landsat-live
prototype-output/zarr-env/bin/python bench/wave1_products.py landsat9-live
prototype-output/zarr-env/bin/python bench/wave1_products.py nasa-live
prototype-output/zarr-env/bin/python bench/wave1_nasa_probe.py verify
prototype-output/zarr-env/bin/python bench/wave1_products.py offline
```

Live capture is explicitly bounded and freezes original byte offsets/ETags; subsequent tests replay verified pages locally and fail on uncaptured ranges. Raw mirrors are ignored artifacts under `prototype-output/wave1`; compact results are versioned under `docs/products`. Benchmarks report the tested small AOIs, reference work, network/loopback distinction, encoded bytes, requests, time, CPU/RSS, scheduler/writer reservations and output size. They establish correctness and costs, not a general performance advantage.

The authenticated harnesses take an explicit private netrc path. Use fresh output/report paths. For example:

```sh
python bench/wave1_hls_auth.py --netrc PRIVATE_FILE --binary target/release/tile-ripper --work NEW-HLS-CHECK --report NEW-HLS-REPORT.json
python bench/wave1_dswx_auth.py --netrc PRIVATE_FILE --binary target/release/tile-ripper --work NEW-DSWX-CHECK --report NEW-DSWX-REPORT.json
python bench/wave1_rtc_auth.py --netrc PRIVATE_FILE --binary target/release/tile-ripper --work NEW-RTC-CHECK --report NEW-RTC-REPORT.json
```

For optimized replay, build with `--release --no-default-features --features native-cog,authenticated-http` and set `TILE_RIPPER_PRODUCTS_BINARY=target/release/tile-ripper` when running `bench/wave1_products.py`. The benchmark defaults to the debug binary, which must also be built without GDAL when `--gdal-free` is exercised.
