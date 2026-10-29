# Southern OPERA DIST-ANN-HLS native-grid qualification

This evidence set isolates the real annual HLS specimen
`OPERA_L3_DIST-ANN-HLS_T20LRP_2025_20260109T150354Z_30_v1`
(`G3953708293-LPCLOUD`). It exists because the tile is geographically south of
the equator while its producer GeoTIFFs explicitly encode **EPSG:32620**.

The original GeoTIFF contract is deliberately retained: projected CRS
EPSG:32620 (GeoKey `ProjectedCSTypeGeoKey=32620`), affine
`[30, 0, 799980, 0, -30, -1099980]`, no false northing, and a negative native
y axis. This is not renamed to EPSG:32720 and does not receive a 10,000,000 m
northing shift. Its actual geographic footprint is near 60°W, 10°S; EPSG area
of use does not replace producer metadata or native pixel geometry.

CMR's `HORIZONTAL_CS_NAME` instead says `UTM, WGS84, UTM ZONE 21`, which
contradicts MGRS `20LRP` and every inspected TIFF GeoKey. The receipt preserves
that catalog text as evidence, but the named GeoTIFF grid plus MGRS identity
govern the coordinate contract; catalog text must not relabel the source.

`cmr-receipt.json` captures the fresh CMR identity and advertised all-layer
asset set. The verified `granules.json` discovery representation is the
selection/time-bounds authority: `2025-01-03T14:02:38.837Z` through
`2025-12-29T14:25:53.638Z`. The receipt separately preserves the richer UMM
provider endpoints (`2025-01-03T14:02:38.837653Z` through
`2025-12-29T14:25:53.638110Z`); they are not described as string-identical or
silently substituted into the public temporal contract.
`source-receipt.json` records strongly ETag-bound, authenticated bounded header
and 16 × 16 native sample inspection for each of the 21 COGs. Pixel arrays are
kept only under ignored `prototype-output/dist-ann-southern/source-samples/`.

Both LAST-DATE TIFFs carry `Update_Date=2025-12-29T14:15:39.774933Z`,
which differs from the CMR coverage endpoint. These fields are preserved
separately, including the producer's input DIST-ALERT lineage and layer
description. The [official DIST product specification, Tables 4-2 and 4-4](https://lpdaac.usgs.gov/documents/1766/OPERA_DIST_HLS_Product_Specification_V1.pdf)
defines LAST-DATE pixels as days of last quality-assessed observations and
the CMR interval as input-year HLS coverage; it does not require the TIFF
update timestamp to equal the interval endpoint. Acquisition still uses the
catalog coverage interval, with no instantaneous annual observation invented.

The isolated harness is [dist_ann_southern_qualification.py](../../../bench/dist_ann_southern_qualification.py).
It uses credentials only for URS redirects, admits only LP DAAC/URS/LP DAAC
CloudFront origins, caps each independent object read at 8 MiB, reserves a
128 MiB acquisition buffer, uses 64-pixel chunks, and checks raw source bits,
source-valid/AOI/quality masks, native axes, all 21 layer source versions,
durable Zarr reopening, and terminal `stream --values` source-version
validation.

## Results and limits

[The southern receipt](qualification-receipt.json) passes all 21 layers for
a 30 × 31 pixel source window, durable Zarr reopening, and completed value
streaming using binary SHA-256
`e6e1967faa2b864f8cc1a757eacdbf35fdb86a488adb1d4da13a0720c93d6162`.
Every output retains the original EPSG:32620, zero false northing, negative
y coordinates and native GeoTIFF evidence. Both status layers contain only
class 0 in this southern window; this case does not demonstrate a southern
confirmed-disturbance detection.

[The northern regression](north-regression.json) compares all 21 layers in
a small confirmed-disturbance window against independent source reads and
the retained northern output, then streams the final binary and validates
its source versions and new temporal provenance. Northern sources receive
no extended-coordinate marker, and their original TIFF metadata is unchanged.
The retained Zarr was created by the preceding intermediate build; the final
build's northern delivery check is a stream replay. An initial final-build
stream failed closed during source revalidation; a bounded retry using the
same pins completed successfully. No source identity checks were relaxed.

[Acceptance](acceptance.json) binds the implementation and fixtures to the
Rust 1.91/1.92 feature matrices, the existing Wave 3/4 science replay and
documentation checks. Controlled fixtures cover conventional southern UTM,
rejection of an unqualified generic northern-UTM/southern-geographic request,
explicit projected native bounds and unchanged generic GDAL projection.
Source-level checks reject shifted or mismatched annual MGRS grids and cover
an M-band grid snapped just north of the equator; the M-band case has no live
archive qualification here.

The admission is private to the annual DIST product and depends on inspected,
version-bound source evidence. It adds no public override and does not widen
the native adapter's ordinary geographic bounds, zone or hemisphere rules.
This evidence qualifies the named southern specimen and convention, not every
southern tile, product version, selection shape or quality class. The work is
local and unreleased.

Run capture before qualification with an explicitly authorized mode-0600 netrc:

```sh
python bench/dist_ann_southern_qualification.py --netrc /absolute/path/to/private.netrc --capture
python bench/dist_ann_southern_qualification.py --netrc /absolute/path/to/private.netrc --binary /absolute/path/to/tile-ripper --qualify
```

The northern receipt under `docs/products/opera-expansion/` is a distinct
conventional-northern specimen and is not modified by this case.
