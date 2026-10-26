# Sentinel-2

Tile-Ripper reads Sentinel-2 Collection 1 Level-2A scenes from the Earth Search catalog (a STAC, SpatioTemporal Asset Catalog, service) and writes the pixels inside your polygon to a new local Zarr store. It reads only the parts of each cloud-optimized GeoTIFF (COG) that your area of interest (AOI) touches. Every band of every scene stays on its own native grid, as stored UInt16 codes.

## Before you start

Uses the build from [Installation](../installation.md). Commands assume `tile-ripper` is on your PATH; from a source checkout use `./target/release/tile-ripper`. Run them from the repository root, because paths such as `docs/native-sentinel/request.json` are relative to it.

No account is needed. Earth Search and the Sentinel-2 files it points to are public. Python is optional and only used to read the result.

## Fetch

Two ways to ask for the same kind of result. Run `plan` first: it reads metadata and source versions but no pixels.

### Bundled request

[`docs/native-sentinel/request.json`](../native-sentinel/request.json) selects bands B04 and B08 over a small triangle near Chapel Hill, North Carolina, on 28 July 2025.

```sh
tile-ripper plan docs/native-sentinel/request.json \
  --gdal-free --human
tile-ripper fetch docs/native-sentinel/request.json \
  --gdal-free --output ./sentinel-bundled.zarr --human
```

In a test run the plan listed one scene (`S2A_T17SPV_20250728T161108_L2A`) with B04 and B08 each 91 x 113 native pixels in EPSG:32617. It estimated 41,132 bytes of values but 3,035,042 encoded bytes read, because whole COG tiles are fetched. The fetch ended with `Acquisition complete: 2 output records.` Catalog contents change, so a later run may list other scenes.

### Your own area, with flags

Save a GeoJSON Polygon geometry (not a Feature) as `polygon.geojson`, with longitude first:

```json
{"type": "Polygon", "coordinates": [[
  [-79.06, 35.91], [-79.05, 35.91],
  [-79.052, 35.92], [-79.06, 35.91]]]}
```

```sh
tile-ripper fetch \
  --source https://earth-search.aws.element84.com/v1 \
  --format stac --collection sentinel-2-c1-l2a \
  --aoi polygon.geojson \
  --start 2025-01-02T00:00:00Z --end 2025-01-13T23:59:59Z \
  --variables B04,B08,B11 \
  --gdal-free --output ./sentinel.zarr --human
```

Both interval endpoints are inclusive UTC. `--bbox WEST,SOUTH,EAST,NORTH` can replace `--aoi`. Flags cannot be mixed with a request file. Choose a new `--output` path; existing paths are refused. The same flags with `plan` preview the request.

In a test run this produced six observation groups: two scenes (2 and 12 January 2025) times three bands. The plan listed B04 and B08 at 91 x 113 native pixels (10 m grid) and B11 at 46 x 57 (20 m grid).

Keep `--gdal-free`: without it, the recommended build stops with `GeoTIFF output requires GDAL`, because GeoTIFF is the default output.

## What you get

`sentinel.zarr` is an ordinary Zarr v2 group, with `manifest.json` inside it:

```text
sentinel.zarr/
  manifest.json
  observations/
    000000/         one scene, one band
      values/       UInt16 [time=1, y, x]
      valid/        UInt8: 1 usable, 0 not
      aoi_mask/     UInt8: 1 inside polygon
      x/, y/        native pixel centers (Float64)
      time/         Int64, ns since 1970-01-01
      spatial_ref/  CRS and grid mapping
    000001/ ...
```

- `values` holds the source codes. Cells outside the polygon are set to the source nodata code. `valid` is polygon inclusion and non-nodata combined; `aoi_mask` tells "outside the polygon" from "nodata inside it". Neither is a cloud mask.
- Groups are ordered by acquisition time, asset ID and band. Find a band in the group's `source` attribute (`requested_band`) or in `manifest.json`.
- Native grids (10, 20 or 60 m by band) stay separate, and no two are assumed equal. Open groups one at a time and choose alignment yourself.
- Windows are clipped to the scene. Missing territory is omitted, not padded.
- Scale and offset are recorded as `scale_factor` and `add_offset` (B04 carried 0.0001 and -0.1) but never applied by Tile-Ripper. xarray's default open applies them; see [Output, values and masks](output.md).

```python
import xarray as xr

ds = xr.open_zarr(
    "sentinel.zarr", group="observations/000000",
    consolidated=False, chunks=None, mask_and_scale=False,
)
print(ds.attrs["source"]["requested_band"], ds.time.values)
raw = ds["values"]                # stored UInt16 codes
usable = raw.where(ds["valid"] == 1)
```

Use a result only if the command exits successfully and `manifest.json` says `"status": "complete"`. An interrupted run may leave a hidden `.NAME.partial-*` sibling; never use it.

## Limits

- Query geometry must be EPSG:4326, longitude then latitude, within ±80° latitude and at most 5° per axis. Polygons must be finite, closed, simple and two-dimensional. Antimeridian crossings fail.
- Only the Earth Search collection `sentinel-2-c1-l2a` is supported. Planetary Computer's Sentinel-2 collection is not a substitute: processing baselines and offsets can differ.
- Query geometry is transformed only to WGS84 Universal Transverse Mercator (UTM) grids (EPSG:32601 to 32660 and 32701 to 32760) in the same hemisphere and within ±6° of the zone's central meridian. Nearby overlapping zones can be mixed; each observation keeps its own.
- Bands: B01 to B09, B8A, B11 and B12, or the Earth Search common names (`red`, `nir`). B10 and derived indices are not supported.
- Direct flags cap a request at 8 items, 8 assets and 1 million output pixels. For more, use a JSON request with custom limits.
- No cloud mask, compositing, reflectance conversion, reprojection, resampling or mosaicking. Unsupported rasters fail; there is no fallback to another reader.
- GeoTIFF output needs a GDAL build.
- `--buffer-mib` sets a payload budget, not a limit on process memory.

## Details and evidence

- [Sentinel workflow](../native-sentinel/workflow.md): geographic and raster contract, edge rules for the polygon mask.
- [Scientific output workflow](../scientific-output/workflow.md): store layout, independent-reader checks, completion rules.
- [Reader selection and Rust API](../native-http/cli.md): `--gdal-free`, supported COG profile, library use.
- [Support contract](../alpha/contract.md) and [Supported data](supported-data.md).
