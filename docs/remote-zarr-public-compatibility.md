# Real public Zarr compatibility assessment

The [WeatherBench 2 ERA5 data guide](https://weatherbench2.readthedocs.io/en/latest/data-guide.html)
identifies a public, scientific Zarr v2 reanalysis store:

[1959–2023 ERA5, six-hourly, 240×121](https://storage.googleapis.com/weatherbench2/datasets/era5/1959-2023_01_10-6h-240x121_equiangular_with_poles_conservative.zarr/.zgroup).
The assessed variable is `2m_temperature`, avoiding the separate pressure-level
axis used by other variables. This is a metadata/coordinate compatibility probe,
not a successful Tile-Ripper temperature extraction.

We fetched ten small metadata objects (at most 64 KiB each) and three coordinate
objects (at most 256 KiB encoded/decoded each). No temperature data chunks were
requested. [Observed objects, headers and coordinate samples](remote-zarr-public-metadata.json)
are saved directly, with their actual byte counts. A separate bounded
[Tile-Ripper HTTPS plan probe](remote-zarr-public-plan.json) reaches the store and
fails explicitly on the missing custom georeferencing requirement before any
scientific data fetch.

| Property | Observed public store | Current contract / consequence |
|---|---|---|
| Storage/version | Public HTTPS, Zarr v2, strong ETags | Transport can reach it; S3 credentials or v3 are not needed |
| Root attributes | `{}`; no `tile_ripper_grid` | First rejection: explicit CRS/affine marker required |
| Variable dimensions | `(time, longitude, latitude)` | Requires `(time,y,x)`; aliases alone do not fix axis order |
| Variable shape/chunks | `[93544,240,121]` / `[8,240,121]` | Native data chunk is 929,280 decoded bytes at Float32 |
| Values / nodata | `<f4`, `fill_value: "NaN"`, units K | UInt16 buffers/integer fill cannot preserve these values or validity |
| Compression | Blosc, LZ4, level 5, shuffle 1 | Unsupported; both temperature and coordinate arrays need this existing codec |
| Longitude | `<f8`, 240 values, 0…358.5, ascending | Name differs from x; documented 0–360 convention must be explicit |
| Latitude | `<f8`, 121 values, −90…90, ascending | Name differs from y and orientation differs from the required negative y step |
| Coordinate attributes | Dimension names; no CRS/grid marker or explicit coordinate units in sampled attrs | Do not guess CRS/orientation from arbitrary names; use documented conventions or explicit mapping |
| Time | `<i8`, hours since 1959-01-01, proleptic Gregorian | Dtype/calendar match; epoch/unit conversion is still needed |
| Time axis admission | 93,544 × 8 = 748,352 decoded bytes | Existing axis cap needs a buffer of at least 11,973,632 bytes (e.g. 16 MiB) |

Adding only `tile_ripper_grid` would **not** unlock this dataset. Casting temperature
to UInt16, changing NaN to zero, pretending latitude decreases, or treating hours
as Unix seconds would produce incorrect science and is not an acceptable workaround.
No such implementation changes were made in this milestone.

The smallest complete follow-up for this particular real variable is a narrow
interoperability profile: explicit coordinate-role/axis-order and orientation
mapping, documented geographic interpretation, CF time-unit conversion, native
Float32/NaN buffers and the mature Blosc/LZ4 codec. Target one regional, time-bounded
`2m_temperature` subset and compare against independent xarray. Preserve actual
coordinates and values, and disclose any axis permutation. This needs no generic
reprojection/mosaic engine, new format, S3 signing or Zarr v3.

A first incremental step can remove reliance on a custom marker through explicit
coordinate mapping, but it must not be advertised as complete public-data support
until dtype, codec and time conventions are also covered. The typed UInt16
executor is an intentional present limitation of both acquisition paths.

Recommend that focused interoperability work before a third scientific format.
Of the original backend choices, an S3/object-store adapter is the closest
follow-on once private/versioned-store access is actually required; v3 and
SWOT/NISAR HDF5 should follow a concrete dataset need. Hosting our existing test
layout through R2 would validate public HTTPS reproducibility, not resolve these
scientific compatibility gaps. Per instruction, no Cloudflare/R2 or domain
configuration/deployment was attempted.
