# Output, values and masks

With the build from [Installation](../installation.md), every fetch writes
Zarr v2 that opens with ordinary Zarr and xarray tools. It holds the source's original stored values on the source's own grid, masks as
separate arrays, and a `manifest.json` completion record. This page explains
where things are, how to tell that a result is complete, and how to get from
stored codes to physical values.

Commands assume `tile-ripper` is on your PATH; from a source checkout use
`./target/release/tile-ripper`. Python examples need any environment with
xarray and zarr, for example
`uv run --no-project --with xarray --with zarr python`.

## What `--output` creates

Choose a new path; existing paths are refused.

| Request type | `--output` | What is created | `manifest.json` | Groups |
| --- | --- | --- | --- | --- |
| Zarr stores: ERA5, IMERG, GFS | a new directory, e.g. `./era5-subset` | `era5-subset/<asset id>.zarr/` (e.g. `era5.zarr`, `imerg.zarr`, `gfs.zarr`) | beside the store, in the directory | one Zarr group per source asset; one array per variable plus coordinate arrays |
| Sentinel-2 SpatioTemporal Asset Catalog (STAC) request | the store, e.g. `./s2.zarr` | the Zarr store | inside the store | `observations/000000`, `000001`, ...: one per band of each scene |
| Landsat, HLS, OPERA products | the store, e.g. `./dswx.zarr` | the Zarr store | inside the store | `observations/000000`, ...: one per band of each scene |
| GOES, SMAP, NISAR products | the store, e.g. `./goes.zarr` | the Zarr store | inside the store | `observations/<granule id>`: one per observation, holding all requested variables |
| SWOT job (per docs) | the store, e.g. `./swot.zarr` | the Zarr store | inside the store | `observations/<id>`: one per job observation |

All rows except SWOT were run for this page (ERA5, IMERG, GFS, the bundled
Sentinel-2 request, the public OPERA DSWx-HLS sample and GOES-19); the SWOT row
follows the [SWOT job contract](../swot-job/workflow.md). GeoTIFF output exists
only in the default GDAL build (see [Installation](../installation.md#other-builds)).

Zarr stores are written without consolidated metadata, so open them with
`consolidated=False`. To list the observation groups of a store:

```python
import zarr
root = zarr.open_group("s2.zarr", mode="r")
print(sorted(root["observations"].group_keys()))
```

## Is the result complete?

Use a result only if the command exits successfully and `manifest.json` says
`"status": "complete"`. Partial output is never a result.

- Sentinel-2, Landsat/HLS/OPERA, GOES/SMAP/NISAR and SWOT outputs are built in
  a hidden sibling such as `.s2.zarr.partial-PID-N` and then renamed into place
  in one step that never replaces an existing path. The requested path
  therefore only ever holds completed data. If the process is killed or the
  machine loses power, the hidden sibling can remain. Never use it, even if
  its own metadata says `complete`; inspect it and delete it yourself.
- Zarr-store requests (ERA5, IMERG, GFS) write into the requested directory
  directly. A caught failure removes it, but an abrupt interruption can leave
  an incomplete directory, possibly without `manifest.json`.

A failed run is never resumed: fetch again into a new path. The one exception
is the opt-in SWOT source-range checkpoint, which reuses verified source bytes
but still rebuilds and revalidates the whole output; see
[SWOT Raster](swot.md).

## What `manifest.json` records

The manifest is the completion record and the provenance for the run. Its
top-level keys depend on the writer:

| Output | Top-level keys |
| --- | --- |
| Zarr stores | `status`, `document_type` (`acquisition_result`), `schema_version`, `sources`, `source_metadata`, `outputs`, `inspection`, `plan`, `observed`, `processing`, `decisions`, `limitations` |
| Sentinel-2 and Landsat/HLS/OPERA | `status`, `document_type`, `schema_version`, `format`, `writer`, `sources`, `outputs`, `inspection`, `observed`, `processing`, `completion`, `output_encoding`, `output_chunk_edge` |
| GOES, SMAP, NISAR | `status`, `schema` (`tile-ripper-array-output-v1`), `format`, `profile`, `plan`, `observations`, `output_io`, `completion`, `validity`, `seconds_before_publication` |

In plain terms: `sources` (or `observations`) names every source object with
its version identity (ETag or size) as validated before and after the read;
`outputs` maps each written array to its source, window, data type, fill and
scale/offset; `inspection` or `plan` is the plan that was executed; `observed`
records what was actually read (requests, bytes, timings). Source versions are
checked per object; they are not an atomic snapshot of a whole catalog or
collection.

## One observation per group

Each scene, band or granule keeps its own native grid, coordinates and
`spatial_ref`. Nothing is aligned, mosaicked or reprojected. In a Sentinel-2
fetch, a 10 m band and a 20 m band are separate groups on different grids;
two scenes from neighbouring tiles stay separate even if they overlap. To
combine groups (for example a band with its quality layer), first check that
the CRS, transform, coordinates and window match, and choose any alignment
yourself in your analysis software.

Zarr-store outputs keep the source's variable and coordinate names (ERA5 uses
`latitude`/`longitude`, IMERG `lat`/`lon`), with data stored in
(time, latitude, longitude) order; the source's own axis order is recorded in
the plan.

## Stored codes and physical values

Tile-Ripper never applies calibration. Scale and offset stay as attributes
(`scale_factor`, `add_offset`, `units`), and the stored codes are written
unchanged. xarray decides what you see:

```python
import xarray as xr
store = "goes.zarr"
group = "observations/OR_ABI-L2-CMIPC-M6C13_G19_..."
codes = xr.open_zarr(store, group=group,
                     consolidated=False, chunks=None,
                     mask_and_scale=False)
values = xr.open_zarr(store, group=group,
                      consolidated=False, chunks=None)
```

Use the full group name from the listing above. `mask_and_scale=False` keeps
the stored types and codes (times still decode). The default open applies
`scale_factor` and `add_offset` and turns fill values into NaN. Examples from
real fetches:

- GOES-19 band 13: stored `CMI` is Int16 code 3310; the default open gives
  3310 × 0.0614533 + 89.62 = 293.03 K. The GOES data quality flag (`DQF`)
  stays Int8 only with `mask_and_scale=False`; the default open turns it
  into floats.
- Sentinel-2: stored `values` are UInt16 with `scale_factor` 0.0001 and
  `add_offset` -0.1; the default open gives reflectance as floats and turns
  the source nodata value 0 into NaN.
- ERA5: Float32 kelvin with no scale or offset, so both opens give the same
  numbers.

**Always read masks and classification layers with `mask_and_scale=False`.**
In Sentinel-2 and Landsat/HLS/OPERA outputs the masks have fill value 0, so
the default open turns every 0 into NaN and every 1 into 1.0, which hides the
excluded pixels. Class layers such as OPERA `WTR` likewise become floats with
their fill code (255) as NaN.

## Masks

Masks are separate UInt8 arrays: 1 means the condition holds, 0 means it does
not. Each mask answers one question; combine them explicitly.

| Array | Written for | 1 means | Notes |
| --- | --- | --- | --- |
| `aoi_mask` | Sentinel-2, Landsat/HLS/OPERA, GOES/SMAP/NISAR; SWOT with a geographic selection | the pixel center is inside the requested area | Source nodata may still be inside. A native-index selection marks every selected pixel as included, including GOES space. |
| `valid` | Sentinel-2, Landsat/HLS/OPERA | inside the area and not source nodata or a documented product fill | `flag_meanings` `excluded included`. Not cloud-free, not water. |
| `source_valid_<variable>` | GOES/SMAP/NISAR, SWOT | the source value is not missing (fill marker or NaN) | One per variable, e.g. `source_valid_CMI`. Independent of area and quality. |
| `earth_visible` | GOES/SMAP/NISAR | the native pixel center's line of sight meets the Earth | 0 is space (GOES); latitude and longitude are NaN there. |
| `quality_<policy>` (Landsat, HLS, OPERA) | when requested, only in the group holding the raw QA or class layer the policy reads | the named policy accepts the pixel **and** it is inside the area and valid | Already combined with area inclusion, source validity and product fill. `flag_meanings` `rejected accepted`. |
| `quality_<policy>` (GOES, SMAP) | each observation group, when the request names the policy | the raw flag passes the policy: `DQF` exactly 0 (`goes_dqf_good`) or retrieval word exactly 0 or 8 (`smap_retrieval_recommended`) | Raw flag only: independent of `aoi_mask`, `source_valid_*` and `earth_visible`, so combine them yourself. Described by `policy` and `definition` attributes (no `flag_meanings`). NISAR has no quality policy. |
| `quality_mask` | SWOT | the job's quality policy accepts the pixel | `flag_meanings` `excluded accepted`; not validity or area. |

Real names seen in fetches for this page: `aoi_mask`, `valid`,
`quality_dswx_wtr_water` (OPERA DSWx-HLS), `source_valid_CMI`,
`source_valid_DQF`, `earth_visible` and `quality_goes_dqf_good` (GOES-19). The
policy's exact definition is stored in the mask's attributes (`long_name`,
`definition` or `derivation`). A GOES analysis mask, for example, is
`aoi_mask & source_valid_CMI & earth_visible & quality_goes_dqf_good`; see
[GOES, SMAP and NISAR](goes-smap-nisar.md) for a worked example.

Zarr-store outputs (ERA5, IMERG, GFS) have no mask arrays; missing values are
the source's own fill (NaN in these Float32 stores).

## Streaming instead of saving

`stream` writes newline-delimited JSON (NDJSON) to standard output instead of a
store. Every chunk record is provisional. The last line is a completion record
(`"event": "complete"` for named products, `"type": "complete"` for other
requests). Use streamed data only after that record arrives and the command
exits successfully; a failure or cancellation invalidates earlier chunks.
`--values` (stored values as base64 little-endian bits, plus masks) works only
for named-product streams and SWOT `--quality-policy` streams.

## Details and evidence

- [Sentinel-2 Zarr output details](../scientific-output/workflow.md): layout,
  calibration attributes and the publication protocol.
- [Named products](../products/workflow.md): `valid`, `aoi_mask` and
  `quality_<policy>` semantics for Landsat, HLS and OPERA.
- [GOES, SMAP and NISAR report](../products/wave3/qualification.md) and
  [geographic selection report](../products/wave4/qualification.md).
- [SWOT job contract](../swot-job/workflow.md): output arrays and reading
  examples.
- [Support contract](../alpha/contract.md): completion and failure rules.
