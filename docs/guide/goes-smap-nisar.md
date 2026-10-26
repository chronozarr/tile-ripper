# GOES, SMAP and NISAR

Tile-Ripper reads small subsets of three satellite products: GOES-19
ABI (Advanced Baseline Imager) cloud and moisture imagery, SMAP soil
moisture and NISAR radar backscatter. Each request returns one acquisition on its native grid,
as a Zarr store of stored values, masks and coordinates. Nothing is
reprojected, resampled or calibrated.

## Before you start

Uses the build from [Installation](../installation.md). Commands
assume `tile-ripper` is on your PATH; from a source checkout use
`./target/release/tile-ripper`. Run commands from the repository root
so `examples/...` paths resolve. If a request fails with `unknown
scientific product profile`, your build lacks `native-hdf5`; rebuild
as described in Installation.

| Family | Account needed |
| --- | --- |
| GOES-19 | No. Data come from NOAA's public bucket. |
| SMAP | Yes, a free NASA Earthdata login (data host: NSIDC). |
| NISAR | Yes, a free NASA Earthdata login (data host: ASF). |

For SMAP and NISAR, set up a login first. See
[Earthdata login](earthdata-login.md). Every `plan` and `fetch` on
them needs `--auth earthdata --netrc "$HOME/.netrc"`. `discover` is
public catalog search and needs no login.

## Request names

Run `tile-ripper products` and look under `array_profiles` for the
exact list. The names are:

| Request name | What it selects |
| --- | --- |
| `goes19-abi-cmipc-c13` | GOES-19 CONUS band 13, brightness temperature |
| `goes19-abi-cmipc-c01` | GOES-19 CONUS band 1, reflectance factor |
| `goes19-abi-cmipm1-c02` | GOES-19 Mesoscale-1 band 2, reflectance factor |
| `smap-spl3smp-e-v6-am`, `-pm` | SMAP global grid, AM or PM pass |
| `smap-spl3smp-e-v6-north-am`, `-north-pm` | SMAP northern polar grid |
| `nisar-gcov-p1-fa-hhhh` | NISAR frequency A HHHH (horizontal transmit and receive) backscatter |

## Choosing pixels and one observation

Every request names a product, an area of interest (`aoi`), a time
interval and bands. Two ways to pick pixels:

- **Geographic.** A WGS84 bounding box (`"kind": "bounds"`) or a
  polygon. Tile-Ripper tests each native pixel center against your
  shape and reads the smallest native index window that holds the
  included centers. Pixels inside that window but outside your shape
  stay in the output, and `aoi_mask` marks which centers were inside.
- **Native index.** `"kind": "array_slice"` with `start`, `count` and
  `step` along the source rows and columns. Every pixel you select is
  included in `aoi_mask`, even ones that look at space.

A request resolves exactly one acquisition (one observation). Leave
`observations` out of the request and `plan` finds it from your time
interval. If more than one matches, it stops with the error `array
acquisition admits one observation per job`. Narrow the interval, or
run `discover` and copy one entry into `observations`. For several
acquisitions, run several requests.

## GOES-19

No account needed. This example runs anonymously. It selects a 5 by 4
pixel window, about 10 km across in South Carolina, from one CONUS
(contiguous US) scan. The bands are `CMI` (Cloud and Moisture
Imagery) and `DQF` (data quality flag).

```sh
tile-ripper discover --product goes19-abi-cmipc-c13 \
  --bbox '-80,34,-79.9,34.1' \
  --start 2026-10-24T16:00:00Z --end 2026-10-24T16:05:00Z \
  --bands CMI,DQF --limit 2

tile-ripper plan examples/product-goes-abi.json \
  --gdal-free --human

tile-ripper fetch examples/product-goes-abi.json --gdal-free \
  --output ./goes-c13.zarr > goes-completion.json
```

`discover` lists NOAA files for an interval of at most one hour (up to
32 files) and errors rather than truncating. The plan shows the chosen
file, the native window and "Included native centers: 20". It also
shows "Estimated unique encoded data bytes: 142742". Whole source
chunks are read, so this tiny window reads about 143 kB of a 4 MB
file.

Choose a new output path; existing paths are refused. Use a result
only if the command exits successfully and
`goes-c13.zarr/manifest.json` says `"status": "complete"`. The completion
record printed to stdout is large, so redirect it. An interrupted run can
leave a hidden `.NAME.partial-*` sibling; never use it.

### What you get (GOES)

The store has `manifest.json` and one group,
`observations/<observation id>`, holding:

- `CMI`: Int16 codes, as stored in the source. The attributes
  `scale_factor`, `add_offset` and `_FillValue` (-1) are kept but not
  applied. Band 13 is brightness temperature in kelvin; bands 1 and 2
  are unitless reflectance factor.
- `DQF`: Int8 flag codes. 0 is good; 1 conditionally usable;
  2 out of range; 3 no value; 4 focal plane temperature threshold
  exceeded.
- `x`, `y`: Int16 packed **scan angles in radians**, not metres. They
  are angles from the satellite's view, with `scale_factor` and
  `add_offset`.
- `longitude`, `latitude`: Float64 values Tile-Ripper computes by
  navigating each pixel center. NaN where the pixel looks at space.
- `native_row`, `native_column`: zero-based indices into the full
  source grid. Also `t` (scan midpoint time), `band_id`,
  `band_wavelength` and `spatial_ref` (projection parameters).
- Masks, all UInt8 0 or 1 and independent of each other:
  `source_valid_CMI` and `source_valid_DQF` (0 where the source holds
  its fill marker), `aoi_mask`, `earth_visible` (0 for space) and
  `quality_goes_dqf_good` (1 where `DQF` is exactly 0).

`quality_goes_dqf_good` looks only at `DQF`. It does not include the
area or source validity, so combine masks yourself. A native-index
window over space shows this: fill codes, `source_valid_*` 0,
`earth_visible` 0, `aoi_mask` 1, NaN coordinates.

```python
import xarray as xr
import zarr

path = "goes-c13.zarr"
root = zarr.open_group(path, mode="r")
name = next(iter(root["observations"].group_keys()))
group = f"observations/{name}"

# Stored codes: CMI stays Int16 (for example 3310).
raw = xr.open_zarr(path, group=group, consolidated=False,
                   chunks=None, mask_and_scale=False)

# Physical values: xarray applies scale and offset (293.03 K).
ds = xr.open_zarr(path, group=group, consolidated=False,
                  chunks=None)
use = ((ds.aoi_mask == 1) & (ds.earth_visible == 1)
       & (ds.source_valid_CMI == 1)
       & (ds.quality_goes_dqf_good == 1))
kelvin = ds.CMI.where(use)
```

Do not mix the two opens in one expression. Their `x` and `y` labels
differ (codes against radians), so label alignment gives empty
results.

### Limits (GOES)

- GOES-19 Level 2 cloud and moisture imagery only, tested on real
  files: CONUS band 13 and band 1, and Mesoscale-1 band 2. Other
  bands, scan modes and ABI products (L1b, full disk and others) are
  not supported.
- One observation per request. A band 2 example request is
  [goes-geographic-request.json](../products/wave4/goes-geographic-request.json).
- Polygons must be closed and may have holes. Edges crossing the
  antimeridian (180° longitude) are rejected.

## SMAP

Needs an Earthdata login. The example selects a small box in
southeast Texas from the AM (descending) pass of 2026-10-23.

```sh
tile-ripper discover --product smap-spl3smp-e-v6-am \
  --bbox '-96,29.9,-95.5,30.5' \
  --start 2026-10-23T00:00:00Z --end 2026-10-23T23:59:59Z \
  --bands soil_moisture,retrieval_qual_flag --limit 2

tile-ripper plan examples/product-smap.json --gdal-free \
  --human --auth earthdata --netrc "$HOME/.netrc"

tile-ripper fetch examples/product-smap.json --gdal-free \
  --auth earthdata --netrc "$HOME/.netrc" \
  --output ./smap-am.zarr > smap-completion.json
```

`--bands` takes any distinct subset of `soil_moisture`,
`retrieval_qual_flag`, `surface_flag` and `freeze_thaw_fraction`. For
the PM pass change `product` to `smap-spl3smp-e-v6-pm`. For the
northern polar grid use the `north` names, as in
[smap-am-ordinary-request.json](../products/wave4/smap-am-ordinary-request.json).

### What you get (SMAP)

One group per observation (`observations/<observation id>`) with:

- `soil_moisture` and `freeze_thaw_fraction` as Float32, and
  `retrieval_qual_flag` and `surface_flag` as UInt16, for the bands
  you requested. Soil moisture is in cm³/cm³. PM output keeps the
  source's `_pm` suffix, for example `soil_moisture_pm`.
- Fill is -9999 for the Float32 layers and 65534 for the flag words.
  `source_valid_<name>` is 0 where a value holds the fill marker.
- `x`, `y`: Float64 pixel centers in metres on the **Equal-Area
  Scalable Earth (EASE) Grid 2.0**, about 9 km spacing. The global grid is
  EPSG:6933 and the northern polar grid is EPSG:6931.
  `native_row`, `native_column`, `longitude` and `latitude` are
  derived from these.
- `aoi_mask`, and `quality_smap_retrieval_recommended` if the request
  lists the policy `smap_retrieval_recommended`. The policy needs
  `retrieval_qual_flag` in `bands`. It marks retrieval words of
  exactly 0 or 8, the values NSIDC's version 6 guide recommends. The
  raw word stays in `retrieval_qual_flag`, so you can still see
  which condition bit is set when it is 8.

Soil moisture is not averaged, scaled or filtered by valid range, and
AM and PM are never combined.

### Limits (SMAP)

- SPL3SMP_E (SMAP enhanced daily 9 km radiometer soil moisture)
  version 6, release R19240 only. Other releases and SMAP L4 are not
  supported.
- Only the global (EPSG:6933) and northern polar (EPSG:6931) grids.
  A northern polar query must lie in the northern hemisphere and
  must not wrap across the antimeridian.
- Time interval at most 31 days; one observation per request.

## NISAR

Needs an Earthdata login. NISAR here means the **provisional** L2
geocoded covariance (GCOV) product, collection
`NISAR_L2_GCOV_PROVISIONAL_V1`. The example granule is about 7 GB, but
only the chunks covering your selection are read.

```sh
tile-ripper discover --product nisar-gcov-p1-fa-hhhh \
  --bbox '-86.43,36.61,-86.41,36.63' \
  --start 2026-10-22T10:55:00Z --end 2026-10-22T10:57:00Z \
  --bands HHHH --limit 8

tile-ripper plan examples/product-nisar-geographic.json \
  --gdal-free --human --auth earthdata --netrc "$HOME/.netrc"

tile-ripper fetch examples/product-nisar-geographic.json \
  --gdal-free --auth earthdata --netrc "$HOME/.netrc" \
  --output ./nisar-hhhh.zarr > nisar-completion.json
```

`examples/product-nisar.json` selects the same product by native
index instead (an 8 by 8 window of rows and columns).

### What you get (NISAR)

- `HHHH`: Float32 radar backscatter as **gamma0 in linear power**,
  already radiometrically terrain corrected by the source. There is
  no dB conversion and no second calibration. Missing values are NaN.
  Zero is a real value, not missing.
- `x`, `y`: Float64 pixel centers in metres on the Universal
  Transverse Mercator (UTM) zone 16N grid (EPSG:32616), at 10 m or 20 m spacing, with `native_row`,
  `native_column`, `longitude` and `latitude`.
- `source_valid_HHHH` (0 where NaN) and `aoi_mask`. NISAR has no
  quality policy, and the source's own quality layers are not read.
- A selection can include pixels but no usable values, for example a
  corner of all NaNs. That is still a valid, complete result.

### Limits (NISAR)

- Only frequency A HHHH of the provisional product, processing
  release CRID (composite release ID) P05023, product version
  1.0.11. Frequency B, HVHV,
  other covariance terms, complex data and other releases are not
  supported.
- Geographic selection works only on the two inspected UTM zone 16N
  grids, 10 m and 20 m spacing, with at most 5° per axis. Other grids
  and zones fail; native-index selection remains available.
- Time interval at most 31 days; one observation per request.

## Limits that apply to all three

- No reprojection, resampling, mosaicking, compositing, calibration
  or unit conversion. Every observation keeps its own grid.
- No resume. `--checkpoint` is supported only for SWOT, not for these
  products.
- Support covers the inspected file layouts. A different file that
  looks similar can still fail with a specific reader error.

## Details and evidence

Each family was tested against independent readers (h5py, NetCDF-C and
PROJ) on real files. No speed advantage over tuned GDAL or xarray is
claimed.

- [GOES-19 report](../products/wave3/goes.md),
  [SMAP report](../products/wave3/smap.md),
  [NISAR report](../products/wave3/nisar.md)
- [GOES, SMAP and NISAR results](../products/wave3/qualification.md)
  and [geographic selection results](../products/wave4/qualification.md)
- [Support contract](../alpha/contract.md) and
  [coverage roadmap](../products/roadmap.md)
- Request files: [GOES](../../examples/product-goes-abi.json),
  [SMAP](../../examples/product-smap.json),
  [NISAR native index](../../examples/product-nisar.json),
  [NISAR geographic](../../examples/product-nisar-geographic.json)
