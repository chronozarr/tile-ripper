# When ordinary scientific tools are preferable

Tile-Ripper consolidates source selection, physical-read inspection, native
extraction and provenance. Its codecs and raster operations come from established
libraries. It has not demonstrated universal speed or scientific capabilities
beyond those libraries.

| Workflow | Tile-Ripper user decisions | Ordinary alternative | Prefer the alternative when |
| --- | --- | --- | --- |
| Sentinel-2 | endpoint/collection, AOI, inclusive dates, band names, new destination; native separate outputs | STAC client search/pagination, choose assets; rasterio/GDAL transform AOI, window read, pixel-center mask and GeoTIFF write | You need broad raster dtypes, reprojection, masks or subsequent analysis |
| ERA5 | exact supported store, geographic rectangle, times, variables, destination | xarray + Zarr v2 + HTTP filesystem; select indices and write a Zarr store | You already use Python/xarray, need CF conventions beyond the narrow profile, or analyze arrays interactively |
| IMERG | same, native `IMERG_PRECTOT` / probability variables, no unit conversion | xarray/Zarr/numcodecs decode source fixed-scale-offset and Blosc, select and write without repeating lossy filter | You need aggregation, broader variable/dimension handling or a full analysis environment |

These are categories of decisions, not a user-study count. Tile-Ripper direct flags
remove JSON authoring, but source compatibility and scientific selection remain
user decisions. Building the recommended complete native build requires Rust,
a C/C++ toolchain (compiler, CMake and pkg-config) and CA certificates, not
GDAL; GDAL is needed only for the default build and for GeoTIFF output, and
that build also needs a GDAL runtime installation. See
[installation](installation.md). A Python user
with xarray already installed may have a lower setup cost using that environment.
Rasterio wheels can simplify native dependencies; their GDAL build may differ from
the system library. There is no paid service or account requirement for the examples.

## Ordinary xarray approach

For an already configured Python environment with xarray, Zarr v2, numcodecs and
an HTTP-capable fsspec backend, the core extraction looks like this:

```python
import fsspec
import numpy as np
import xarray as xr

url = "https://storage.googleapis.com/weatherbench2/datasets/era5/1959-2023_01_10-6h-64x32_equiangular_conservative.zarr/"
ds = xr.open_zarr(fsspec.get_mapper(url), consolidated=True, chunks=None,
                  decode_cf=False)
# Positive-area cell intersection, including partially intersecting cells.
indices = {}
for name, low, high in [("longitude", 100, 110), ("latitude", -10, 0)]:
    c = ds[name].values
    edges = np.r_[c[0]-(c[1]-c[0])/2, (c[:-1]+c[1:])/2,
                  c[-1]+(c[-1]-c[-2])/2]
    indices[name] = np.flatnonzero((np.maximum(edges[:-1], edges[1:]) > low) &
                                  (np.minimum(edges[:-1], edges[1:]) < high))
times = xr.decode_cf(ds[["time"]]).time.values
indices["time"] = np.flatnonzero((times >= np.datetime64("1960-02-02T12:00:00")) &
                                 (times <= np.datetime64("1960-02-02T18:00:00")))
subset = ds[["2m_temperature"]].isel(indices).transpose("time", "latitude", "longitude").load()
for array in subset.variables.values():
    array.encoding = {}  # choose output encoding explicitly; never repeat lossy storage filters
subset.to_zarr("./new-xarray-subset.zarr", mode="w-", consolidated=False)
```

This is an illustrative ordinary extraction, not the timed equivalent baseline:
it lacks dependency/ETag validation, bounded decoding, a physical-read plan and
completion/provenance envelope. For IMERG it also needs updated subset coverage
attributes. Output encoding/chunks must be selected explicitly to match the
comparison. The **executed** matched baseline in `bench/cf_zarr.py::baseline`
uses the existing bounded HTTP mapping, independently interpreted native indices,
durable writes, matching native chunks/codec and before/after identity checks;
`bench/usability.py` validates its files against the Rust output. It reuses a
manifest template outside timing rather than independently constructing provenance.
No new transport or dependency was added for this milestone.

## Ordinary STAC + GDAL approach

Use a STAC library to search `sentinel-2-c1-l2a` with the AOI and interval, follow
pagination, select the `green` asset for B03, and keep item metadata. Then open
that URL with rasterio/GDAL, transform the densified AOI into the native CRS,
read its native pixel envelope, mask outside pixel centers to source nodata,
and write the original CRS/transform/scale/offset. `bench/usability_stac.py`
executes that independent rasterio operation for the new request, including
writing a reference GeoTIFF. It is given the selected source/window and therefore
its timing excludes discovery and source-validation work; it is not presented
as an equivalent end-to-end benchmark. The controlled, fully matched baseline
remains `bench/stac_workflow.py` and `docs/external-usability-cog-checks.json`.

A bare `gdal_translate -srcwin …` command is shorter, but does not discover
observations, map a geographic AOI, reproduce a polygon mask or retain catalog
provenance. Native values in the independently masked reference and the acquired
GeoTIFF match exactly. Different observations and native grids stay separate.

## Measurements and their limits

See [the current gate](external-usability-decision.md) for new-request numbers,
[raw public Zarr observations](external-usability-public.json),
[public COG validation](external-usability-stac.json), and
[earlier repeated equivalent benchmarks](second-zarr-benchmark.json).
The new public checks are single observations, not repeated performance trials.
Rust CLI wall time includes process startup; the in-process xarray first-fetch
time includes preparation and writing but excludes interpreter startup. Neither
OS nor provider caches were cleared. These numbers do not establish a speedup.
HTTP body-byte counters exclude headers/TLS. Real GDAL network bytes/requests were
not instrumented and remain unavailable, rather than substituted with estimates.
