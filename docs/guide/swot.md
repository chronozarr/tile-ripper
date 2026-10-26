# SWOT Raster

Tile-Ripper can read small windows of the Surface Water and Ocean
Topography (SWOT) Level 2 high-rate (HR) Raster, Version D. You get
one Zarr dataset with one group per observation. Each group holds
raw water surface elevation (WSE) and its quality layers on the
file's own grid, plus masks. Nothing is resampled, aligned or
calibrated.

## Before you start

Uses the build from [Installation](../installation.md). Commands
assume `tile-ripper` is on your PATH; from a source checkout use
`./target/release/tile-ripper`. Run commands from the repository root
so `examples/...` paths resolve.

An Earthdata login is required to read the data. Set it up with
[Earthdata login](earthdata-login.md). Searching the catalog with
`discover` needs no account. `discover-job`, `plan` and `fetch` all
need `--auth earthdata --netrc "$HOME/.netrc"`.

**Only one processing release is supported: PGD0**, a composite release
identifier (CRID). SWOT file names and metadata carry this ID. According
to PO.DAAC's
[SWOT tutorial](https://podaac.github.io/tutorials/quarto_text/SWOT.html),
`PGD0` marks reprocessed Version D products measured from 30 March
2023 to 27 April 2025, and `PID0` marks forward-processed Version D
data from 28 April 2025 on (dates from PO.DAAC, not from Tile-Ripper's
tests). Tile-Ripper accepts only `PGD0` files, at
100 m resolution. Newer `PID0` files are listed by `discover` as
`unsupported processing release`, and Version C is not supported.

## Fetch

The steps are: `discover` (find granules), `discover-job` (choose one
to four observations and write a job file), `plan` (inspect), `fetch`
(write the dataset).

**Quickest start.** The bundled job already selects two 128 by 128
pixel windows (two UTM zones, two dates).

```sh
tile-ripper plan examples/swot-job.json --gdal-free --human \
  --auth earthdata --netrc "$HOME/.netrc"

mkdir -m 700 ./swot-checkpoint
tile-ripper fetch examples/swot-job.json --gdal-free \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint ./swot-checkpoint \
  --output ./swot-subsets.zarr > swot-completion.json
```

**Your own area.** First search NASA's Common Metadata Repository
(CMR). This is anonymous and reads no archive files.

```sh
tile-ripper discover --collection SWOT_L2_HR_Raster_D \
  --bbox -73,41,-70,44 \
  --start 2024-02-16T02:45:00Z --end 2024-02-16T03:00:00Z \
  --limit 20 --output ./swot-search.json
```

The box can be at most 30° per axis and 400 square degrees, the
interval at most 31 days, and `--limit` at most 100. Split queries
that cross the antimeridian. Read `status` and `reasons` in the
output. `complete` means the search finished, not that every granule
is usable. If the search is `truncated` or `failed`, the command exits
nonzero; narrow the query. Each entry says whether the data link and
the release look supported. CMR coverage is approximate, and says
nothing about whether the pixels hold useful water.

Next, copy `examples/swot-aoi-selection.json`, set `catalogs` to your
search file, and list one to four observations. Each needs a
`concept_id` from your search, an `observation_id` of your choice
(letters, digits, `_` and `-`), a `quality_policy`, and exactly one of
`aoi` (a WGS84 bounding box or polygon) or `window` (native rows and
columns):

```json
{
  "concept_id": "G3927661761-POCLOUD",
  "observation_id": "additional_20240216",
  "aoi": {"kind": "bounds", "crs": "EPSG:4326",
          "bbox": [-71.85, 41.53, -71.84, 41.54]},
  "quality_policy": {"kind": "summary",
                     "accepted": ["good", "suspect"]},
  "expected_etag": null
}
```

```sh
tile-ripper discover-job my-selection.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --output ./my-job.json > ./job-review.json

tile-ripper plan ./my-job.json --gdal-free --human \
  --auth earthdata --netrc "$HOME/.netrc"

tile-ripper fetch ./my-job.json --gdal-free \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint ./swot-checkpoint --output ./my-swot.zarr \
  > my-completion.json
```

`discover-job` reads each file's metadata and writes the job file only
if every selection works. It refuses an existing output file, and
accepts only the selection, the Earthdata flags and `--output` (no
`--gdal-free`, `--human` or `--checkpoint`). A geographic selection
becomes native row and column indices in the job file, so you never
calculate indices. If a chosen granule has no pixel centers inside
your shape, it reports `not_ready_zero_native_centers` and writes no
job file.

Choose a new output path; existing paths are refused. Use a result
only if the command exits successfully and `manifest.json` inside the
store says `"status": "complete"`. Interrupted runs may leave a hidden
`.NAME.partial-*` sibling; never use it, and delete it once you are
sure no run is still using it.

### Resuming with `--checkpoint`

`--checkpoint DIRECTORY` saves the source bytes that Tile-Ripper has
already downloaded from NASA. The directory must be private and owned
by you (`mkdir -m 700`). If a run is interrupted, repeat the same
`fetch` with the same job file and checkpoint directory and a **new**
`--output`.

What it does: each saved piece is checked, and a fresh login and
source-version check still happen before reuse. Repeating the fetch
reuses those bytes instead of downloading them again.

What it does not do: the stream starts again from the first tile and
every output array is rewritten. Nothing continues from a partial
output, and it does not work offline. Checkpoint directories survive
success, failure and cancellation, and are never cleaned up
automatically. Delete the directory yourself when you are done. A
changed job or source file needs a fresh directory. Resume is
available for SWOT only.

## What you get

`--output` is a Zarr store with `manifest.json` and
`observations/<id>/`, one group per observation, in job-file order.
Each observation is its own native grid. Different observations are
not aligned and are not a time series.

| Array | Stored type | Meaning |
| --- | --- | --- |
| `wse`, `wse_uncert` | Float32 | Water surface elevation, uncertainty |
| `wse_qual` | UInt8 | Summary quality: 0 good, 1 suspect, 2 degraded, 3 bad, fill 255 |
| `wse_qual_bitwise`, `n_wse_pix` | UInt32 | Detailed flag bits, contributing pixel counts |
| `source_valid_<name>` | UInt8 | 1 where the source value is not missing, one per layer |
| `quality_mask` | UInt8 | Your quality policy: 1 accepted, 0 excluded |
| `aoi_mask` | UInt8 | Geographic selections only: 1 for centers inside your shape |
| `x`, `y` | Float64 | UTM pixel-center coordinates in metres |
| `t`, `spatial_ref` | | Source time; projection (CRS) attributes |

Raw values are never rewritten: fills and NaN bit patterns are kept
exactly. The masks are separate arrays, so you choose how to combine
them. A geographic selection reads the smallest row and column window
that holds your pixel centers. Pixels inside the window but outside
your shape are present; `aoi_mask` marks them 0.

```python
import xarray as xr
import zarr

path = "swot-subsets.zarr"
root = zarr.open_group(path, mode="r")
print(list(root["observations"].group_keys()))

ds = xr.open_zarr(path, group="observations/original_20240216",
                  consolidated=False, chunks=None,
                  mask_and_scale=False, decode_times=False)
ok = (ds.source_valid_wse.astype(bool)
      & ds.quality_mask.astype(bool))
wse = ds.wse.where(ok)
```

`mask_and_scale=False` and `decode_times=False` keep the stored
numbers. Apply any source scale and offset, or time decoding,
yourself. For a geographic selection, also require
`ds.aoi_mask.astype(bool)`.

The quality policy is your choice. The `good` plus `suspect` example is
a demonstration taken from the PO.DAAC cookbook, not a universal
rule. Suspect data can have large errors. Tile-Ripper does **not**
decide whether a pixel gives a usable water surface elevation. A
selection with pixels but no accepted values is a valid result, and
all raw arrays are still written.

## Limits

- Version D, 100 m, processing release PGD0 only. Version C, `PID0`
  and other releases are not supported. `discover` also lists 250 m
  granules, but they are not tested and not supported; a geographic
  selection on one fails.
- A job has one to four explicit observations, processed one at a
  time. There is no alignment, resampling, mosaic or time series.
- Geographic selection takes one simple polygon (holes allowed) or a
  bounding box, in WGS84 longitude and latitude, at most 5° per axis,
  latitude within ±80°, and within 6° of each file's UTM central
  meridian and in its hemisphere. Boxes that touch or cross the antimeridian, and polar
  areas, are rejected. Only uniform 100 m axes in WGS84 UTM are
  accepted. A native `window` uses positive strides.
- Memory limits in the job file are refused, not raised
  automatically. A selection too large for them fails with an error
  that lists the memory components; raise the limits in the job file
  yourself.
- Per-file ETag checks cannot make an atomic snapshot of a changing
  NASA collection. CMR search results can also change between pages.
- Authentication and `--checkpoint` work on macOS and Linux only.

## Details and evidence

The evidence covers the named files and selections, not every file
that looks the same. Tested against independent readers (h5py and
NetCDF-C) on real files.

- [Geographic selection](../swot-aoi/workflow.md) and
  [its results](../swot-aoi/report.md)
- [Jobs and output](../swot-job/workflow.md) and
  [discovery](../swot-discovery/workflow.md)
- [Transfer resume](../transfer-resume/workflow.md)
- [Quality policies](../swot-quality/policies.md) and
  [source tests](../held-out-qualification/report.md)
- Examples: [job](../../examples/swot-job.json),
  [geographic selection](../../examples/swot-aoi-selection.json),
  [native window selection](../../examples/swot-discovery-selection.json)
