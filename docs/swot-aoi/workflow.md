# Geographic AOI to native SWOT Raster pixels

This workflow resolves a geographic area against the inspected source grid.
The acquired values stay on that grid. Each observation has its own coordinates,
CRS, source version and group inside one transactionally published Zarr v2 store.
No reprojection, interpolation, alignment, mosaic or usable-WSE inference occurs.

## Run

```sh
cargo build --release --locked --no-default-features \
  --features native-hdf5,authenticated-http --bin tile-ripper

# Anonymous bounded discovery; this reads no credentials or archive science.
./target/release/tile-ripper discover \
  --collection SWOT_L2_HR_Raster_D --bbox -73,41,-70,44 \
  --start 2024-02-16T02:45:00Z --end 2024-02-16T03:00:00Z \
  --limit 20 --output ./fresh-candidates.json

# Review candidates and explicitly choose one to four identities. The supplied
# example uses the two frozen control catalogs and real geographic polygons.
./target/release/tile-ripper discover-job examples/swot-aoi-selection.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --output ./geographic-job.json > ./geographic-review.json

./target/release/tile-ripper plan ./geographic-job.json \
  --auth earthdata --netrc "$HOME/.netrc" > ./native-plan.json

./target/release/tile-ripper fetch ./geographic-job.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint ./private-geographic-ranges --checkpoint-mib 32 \
  --output ./native-swot.zarr > ./completion.json
```

For a fresh discovery, set the selection file's `catalogs` to the fresh report
paths and choose the required `concept_id` values from those reports. Do not
calculate array indices. The executable manifest contains the resolved indices.
The acceptance harness below performs fresh CMR searches and the entire workflow.

Each selected observation requires exactly one of `window` and `aoi`:

```json
{
  "concept_id": "G3927661761-POCLOUD",
  "observation_id": "additional_20240216",
  "aoi": {
    "kind": "bounds",
    "crs": "EPSG:4326",
    "bbox": [-71.85, 41.53, -71.84, 41.54]
  },
  "quality_policy": {"kind": "summary", "accepted": ["good", "suspect"]},
  "expected_etag": null
}
```

`aoi` also accepts the existing Spatial polygon schema
`{"kind":"polygon","crs":"EPSG:4326","coordinates":[outer,hole,...]}`.
For a normal GeoJSON geometry, use an explicit CRS wrapper:
`{"crs":"EPSG:4326","geometry":{"type":"Polygon","coordinates":[outer,hole,...]}}`.
Both normalize to the same manifest representation. Features, MultiPolygons,
collections, projected input and implicit CRS are unsupported. The unchanged
index alternative is `"window":{"start":[row,column],"count":[rows,columns],"step":[row_stride,column_stride]}`.
Combining selectors or omitting both fails before authenticated inspection.

The example's source budget, aggregate job limits and explicit policies follow
the [existing job contract](../swot-job/workflow.md). Keep exact-object
authentication, secret paths, checkpoint disk/entry limits and new output names.
Source inspection and fetch do not grant arbitrary URL authentication.

## Geographic and source-grid contract

| Item | Supported behavior |
| --- | --- |
| Geographic axes | WGS84 **longitude, latitude**, in degrees. This is an explicit application order even though the EPSG authority's formal geographic axis order differs. |
| Edges | Straight segments in longitude/latitude, including bbox parallels and meridians; no great-circle interpretation. |
| Topology | One valid simple, explicitly closed Polygon; concavity and disjoint valid holes allowed. |
| Membership | Native **pixel centers**, transformed back into the original geographic polygon; closed outer and hole boundaries included. Hole interiors excluded. No all-touched rule. |
| Roundoff | Boundary-distance tolerance `1e-10` degrees, about 11 micrometres of latitude. This is a coordinate roundoff band, not a pixel-size buffer. |
| Native CRS | Actual `/crs` canonical WGS84 UTM WKT1 encoding with matching inspected CF numerical parameters. EPSG:32601–32660 / 32701–32760; explicit easting/northing metres. Alternate WKT encodings, units, datums or parameter conflicts fail. |
| Source grid | PGD0, 100 m descriptor; actual rank-one `/y` and `/x` stored centers. Both axis directions supported. Uniform signed 100 m spacing within `1e-6` m; identity coordinate packing only. |
| Nonuniform axes | Rejected, including monotonic nonuniform arrays. Curvilinear/nonlinear grids, duplicate and nonmonotonic centers are rejected. |
| Geographic domain | Longitude strictly inside `(-180,180)`, latitude `[-80,80]`, at most 5° per axis, within 6° of each source's UTM central meridian and in its hemisphere. AOIs touching/crossing the antimeridian are rejected, as are polar, larger or distant requests. |
| Geometry bounds | At most 512 input positions, 100,000 densified positions, depth 24, 8,192 centers per native axis, 16,777,216 source centers and 256,000,000 center/vertex tests. Existing manifest, plan-node, memory, metadata and output-object caps also apply. |

Shape inspection reads a one-variable one-pixel native **plan**, then a bounded
one-variable plan that reads the complete actual stored x/y arrays. Neither
executes science-payload chunks. A third ordinary selected five-variable plan
establishes scientific/profile and resource admission. Every preparation has a
fresh exact-object session, and the intermediate source versions must match.
The core HDF5 reader, decoder and source authentication policy are unchanged.

Geographic edges are also transformed forward with adaptive subdivision:
segments span at most 0.001° per axis and their projected midpoint error must be
at most 1 mm. The resulting polygon supplies projected bounds, diagnostic area
and grid-overlap coverage. These approximations **never prune candidate centers**.
Every full-source center is tested against the original polygon by inverse UTM
transformation. Thus a curved projected geographic edge cannot silently lose
centers through a corner-only transform or window-local rounding.

The explicit native window is the smallest index rectangle containing all
included source-global centers, with unit strides. Raw values outside the AOI
but inside this rectangle remain present. Reversed axes keep source index order.
Coverage area includes half-pixel grid extents and describes projected geometry;
it is not valid WSE area or an ellipsoidal area measurement.

If any explicitly selected granule contains zero included centers,
`discover-job` emits `not_ready_zero_native_centers`, returns nonzero, and writes
no executable manifest. It does not drop or substitute candidates. Positive
centers with zero quality-accepted WSE are a valid completed scientific result.

## Inspect and analyze

The review contains source CRS, shape, actual axis direction, original AOI,
projected coverage, included-center count, explicit selected window, separate
inspection/resolution timings, source traffic, final selected source chunks,
encoded/decoded requirements, working reservations and native compatibility.
Metadata inspection leaves usable-WSE status unknown.

The manifest retains the normalized original polygon, fixed mask policy,
resolved indices, complete-axis fingerprints, CRS mapping fingerprint, selected
coordinate fingerprints, center count and mask hash. Plan and fetch verify the
selected source grid, coordinates, count and mask against the pinned object.
The complete geographic provenance participates in the scientific checkpoint
identity. Even a subpixel polygon edit with an unchanged discrete mask rejects
reuse of the prior scientific checkpoint. A fresh job with the same discrete
mask remains valid if its other assertions agree.

Each geographic observation contains:

- Five raw science variables, preserving Float32 and unsigned integer bits.
- Five `source_valid_*` masks for original missingness.
- The existing `quality_mask`, with the selected summary/bitwise policy.
- A distinct UInt8 `aoi_mask`, with 1 for included centers and 0 otherwise.
- Exact native x/y, source CRS, time, attributes and source/catalog provenance.

```python
import xarray as xr

ds = xr.open_zarr(
    "native-swot.zarr", group="observations/original_20240216",
    consolidated=False, mask_and_scale=False, decode_times=False,
)
analysis_mask = (
    ds.aoi_mask.astype(bool)
    & ds.source_valid_wse.astype(bool)
    & ds.quality_mask.astype(bool)
)
accepted_wse = ds.wse.where(analysis_mask)
```

Use that composition when accepted-AOI statistics are wanted. Raw arrays are
never rewritten to enforce geographic or quality acceptance. The existing
quality policy already consults source validity as required by its definitions;
keeping the separate masks makes each reason inspectable.

Independent native grids remain separate observation groups. Publication still
requires all observations, successful scientific completion, the final serial
source-version sweep, synchronization and atomic no-replace publication.
Checkpoint replay reauthenticates, reconstructs all science and rewrites the
complete output; it reuses source bytes rather than completed observations.

## Reproduce the gates

Use the existing independent Python reference environment plus PROJ/GEOS/GDAL:
`numpy`, `h5py`, `netCDF4`, `zarr`, `xarray`, `psutil`, `pyproj`, `shapely`, `rasterio`.
The acquisition executable requires none of these Python packages.

```sh
cargo build --release --locked --no-default-features \
  --features native-hdf5,authenticated-http \
  --bin tile-ripper --example swot_aoi_oracle
python bench/swot_aoi.py geometry
python bench/swot_aoi.py offline

# Explicit bounded provider operations, after the frozen gates pass:
python bench/swot_aoi.py additional --netrc "$HOME/.netrc"
python bench/swot_aoi.py live --netrc "$HOME/.netrc"
```

Frozen tests use identity-pinned mirrors and guarded independent references;
missing sparse source bytes are fatal. The third source uses a separately
preserved native capture and a reference-only metadata extension. The original
capture-cap and reference-request-cap failures remain in this directory beside
their measured explanations and bounded amendments.

PROJ's [axis/unit conventions](https://proj.org/en/stable/development/quickstart.html)
and GEOS/Shapely's [closed coverage predicate](https://shapely.readthedocs.io/en/2.1.0/reference/shapely.covers.html)
describe the independent reference semantics. See [the measured decision gate](report.md)
for the actual qualification scope and remaining limitations.
