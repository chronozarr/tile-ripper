# ERA5, IMERG and GFS (Zarr)

Tile-Ripper can subset three specific public Zarr stores: a low-resolution ERA5 reanalysis from WeatherBench2, NASA POWER's daily Integrated Multi-satellite Retrievals for GPM (IMERG) precipitation, and a NOAA Global Forecast System (GFS) analysis published as Zarr version 3. It reads only the source chunks that cover your area and times and writes a small local Zarr store of Float32 values. Support is for these exact stores and layouts, not for any ERA5, IMERG or Zarr dataset.

| Store | Request file | Variables tested |
| --- | --- | --- |
| WeatherBench2 ERA5, 64×32 equiangular grid, 6-hourly 1959–2023 (Zarr v2) | [`examples/weatherbench2.json`](../../examples/weatherbench2.json) | `2m_temperature` (K) |
| NASA POWER IMERG daily spatial store (Zarr v2) | [`examples/imerg.json`](../../examples/imerg.json) | `IMERG_PRECTOT` (mm/day), `IMERG_PRECLIQUID_PROB` (%) |
| dynamical.org NOAA GFS analysis v0.1.0 (Zarr v3), a derived dataset, not NOAA's official GRIB2 archive | [`examples/zarr-v3-gfs.json`](../../examples/zarr-v3-gfs.json) | `temperature_2m` (°C) |

Dynamical's current catalog also publishes Icechunk repositories. They use the
separate opt-in [Icechunk workflow](../icechunk/workflow.md), with explicit
snapshot selection and distinct materialized/virtual source profiles. The
legacy plain-Zarr example here does not imply support for those repositories.

## Before you start

Uses the build from [Installation](../installation.md). Commands assume `tile-ripper` is on your PATH; from a source checkout use `./target/release/tile-ripper`. Run them from the repository root, because `examples/...` paths are relative to it.

No account is needed: the stores are public over anonymous HTTPS. Python is optional and only used to read the result.

## Fetch

`plan` reads metadata and coordinates, never data chunks. `fetch` writes to a new directory; choose a new path, because existing paths are refused.

```sh
tile-ripper plan examples/weatherbench2.json \
  --gdal-free --human
tile-ripper fetch examples/weatherbench2.json \
  --gdal-free --output ./era5-subset --human
tile-ripper fetch examples/imerg.json \
  --gdal-free --output ./imerg-subset --human
tile-ripper fetch examples/zarr-v3-gfs.json \
  --gdal-free --output ./gfs-subset --human
```

In test runs the ERA5 plan estimated 96 bytes of values (24 values) but 580,139 encoded bytes read. IMERG gave 30 values for 1,347,071 bytes, and GFS 8 values for 3,985,993 bytes. Whole source chunks are read, and each chunk spans far more than your subset (ERA5's covers the globe and 100 times). The ratio comes from the stores' layout.

### Change area, time and variables

Edit these three parts of a request file and leave `source`, `grid` and `budget` alone. `--output` replaces the file's `sink` path.

```json
"spatial": {
  "kind": "bounds", "crs": "EPSG:4326",
  "bbox": [350, 35, 359, 45]
},
"time": {
  "kind": "interval",
  "start": "2000-07-01T00:00:00Z",
  "end": "2000-07-01T18:00:00Z"
},
"variables": [{"kind": "dataset", "path": "2m_temperature"}]
```

- `bbox` is `[west, south, east, north]`. Every grid cell that overlaps it is selected, so returned cell edges can extend beyond the box. Run on ERA5, this example returned 2 longitudes, 2 latitudes and 4 times.
- ERA5 longitudes run from 0 to 354.375° east and are not wrapped. A box of `[-10, 35, -1, 45]` selected one longitude, the cell centered on 0°; use 350 to 360 for that area, and split a box that crosses 0° into two requests.
- `time` is an inclusive interval, or explicit instants as in the GFS file: `{"kind": "instants", "values": ["2023-01-15T00:00:00Z"]}`.
- `variables` names a top-level array. Only the variables in the table were tested.
- GFS takes its box in the store's own coordinates. Keep the example's `crs` string (a sphere of radius 6,371,229 m); replacing it with EPSG:4326 changes the scientific metadata. The selection flags (`--bbox`) cover Zarr version 2 and SpatioTemporal Asset Catalog (STAC) sources only; for version 3 use the JSON file.

## What you get

The output is a plain directory, not a store itself. It holds one `<asset id>.zarr` group (`era5.zarr`, `imerg.zarr` or `gfs.zarr`) next to `manifest.json`:

```text
era5-subset/
  manifest.json
  era5.zarr/   2m_temperature, latitude,
               longitude, time
```

- Values are Float32 with NaN for missing, exactly as stored in the source. There is no unit conversion: ERA5 is in kelvin, IMERG in mm/day and %, and this GFS store in degrees Celsius. Read `units` on each variable.
- Dimensions are time, then latitude and longitude (`lat` and `lon` in IMERG), in source coordinate order: the GFS latitudes run 34.25 down to 34.0. Time keeps the source's encoding.
- IMERG's source storage filter is decoded once; the output holds the decoded Float32 values.
- GFS input is Zarr version 3; its output is an uncompressed Zarr version 2 group.

```python
import xarray as xr

ds = xr.open_zarr(
    "era5-subset/era5.zarr", consolidated=False, chunks=None
)
print(ds["2m_temperature"].attrs["units"], dict(ds.sizes))
```

Use a result only if the command exits successfully and `manifest.json` says `"status": "complete"`. An interrupted run can leave an incomplete directory; choose a new path and run again.

## Limits

- Exact stores only. Other ERA5 archives (including ECMWF's ARCO Zarr), other WeatherBench2 resolutions, other IMERG products and other GFS Zarr stores are not supported. `plan` refuses a layout outside the profile before any data chunk is read.
- ERA5 and IMERG profile: Zarr version 2, time-first Float32 with NaN fill, regular geographic coordinates. Extra dimensions, projected or curvilinear coordinates, arbitrary calendars, other data types and codecs outside the tested set (none, gzip, Blosc with LZ4 or Zstd, one fixed-scale-offset filter) fail.
- The source does not declare a precise datum. EPSG:4326 is Tile-Ripper's interpretation, with no datum transformation; check the provider's convention before combining grids.
- GFS profile: top-level UInt16 or Float32 arrays, three dimensions, time first, regular spatial axes, at most one level of sharding (chunks grouped into larger files). Whole-shard decoding and polygon masks are not supported.
- In Zarr version 3 a missing chunk reads as the array's storage fill value, which Tile-Ripper treats as a valid stored value (an omitted UInt16 chunk with fill 0 gives valid zeros) unless the metadata declares it missing; Float32 NaN is always invalid.
- No reprojection, resampling, calibration, unit conversion, compositing or longitude wrapping. Zarr sources take a bounding box, not a polygon.
- Each object's version (ETag) is checked before and after the run. That is not a snapshot of the whole store.

## Your own Zarr

A local Zarr version 2 store in Tile-Ripper's custom-grid layout (UInt16 arrays, time first, an immutable directory) is also supported. See [Local Zarr](../zarr-workflow.md); other layouts fail.

## Details and evidence

- [ERA5](../cf-zarr-workflow.md), [IMERG](../second-zarr-workflow.md) and [Zarr version 3 input](../zarr-v3-input/workflow.md) workflows with the exact layouts.
- [Supported data](supported-data.md), [roadmap](../products/roadmap.md) and [support contract](../alpha/contract.md).
