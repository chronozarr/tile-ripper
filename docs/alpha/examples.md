# Small scientific examples

Run from the checkout after [installation](../installation.md). Use a new output
path each time. These are anonymous public sources; bounds are west,south,east,north
and both date endpoints are inclusive. Inspect plan estimates before fetching.

## Sentinel-2: native red/NIR, no cloud masking

```sh
tile-ripper plan --source https://earth-search.aws.element84.com/v1 --format stac --collection sentinel-2-c1-l2a --bbox '-79.06,35.91,-79.05,35.92' --start 2025-07-28T00:00:00Z --end 2025-07-28T23:59:59Z --variables B04,B08 --human
```

For the verified polygon selection use:

```sh
tile-ripper plan examples/unified-stac.json --human
tile-ripper fetch examples/unified-stac.json --output ./alpha-sentinel --human
find ./alpha-sentinel -name '*.tif' -exec gdalinfo {} \;
```

Inspect each TIFF's scale/offset before interpreting reflectance. Nodata is not
cloud masking; retain Copernicus/provider attribution for derived products.

## ERA5: two native times in Kelvin

```sh
tile-ripper plan examples/weatherbench2.json --human
tile-ripper fetch examples/weatherbench2.json --output ./alpha-era5 --human
```

```python
import xarray as xr
ds = xr.open_zarr("alpha-era5/era5.zarr", consolidated=False, chunks=None)
print(ds.time.values, ds["2m_temperature"].attrs)
print(ds["2m_temperature"].values)
```

This is a coarse 64×32 regridded ERA5 product, not native high-resolution ERA5.
Selects intersecting cells in 0–360 longitude; output centers can exceed bounds.
Kelvin is preserved; no Celsius conversion is performed.

## IMERG: daily precipitation and liquid probability

```sh
tile-ripper plan examples/imerg.json --human
tile-ripper fetch examples/imerg.json --output ./alpha-imerg --human
```

Open `alpha-imerg/imerg.zarr` with the same xarray pattern. `IMERG_PRECTOT` is
mm/day; `IMERG_PRECLIQUID_PROB` is %, not a precipitation amount. This selects
one daily timestamp, not an instantaneous measurement or an accumulated total
computed by Tile-Ripper. Two chunks transfer about 1.35 MB despite the tiny AOI.
Inspect NaNs and units, and cite NASA POWER/IMERG according to provider guidance.
