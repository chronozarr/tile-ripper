# Case contract: 20LRP south of the equator

| Item | Observed contract |
| --- | --- |
| CMR granule | `G3953708293-LPCLOUD`; `OPERA_L3_DIST-ANN-HLS_T20LRP_2025_20260109T150354Z_30_v1` |
| Catalog/MGRS identity | `20LRP` |
| Native grid authority | Each named inspected GeoTIFF: `ProjectedCSTypeGeoKey=32620`, EPSG:32620, 30 m, affine `[30, 0, 799980, 0, -30, -1099980]` |
| Geographic setting | Near 60°W, 10°S despite EPSG:32620's published northern area of use |
| Prohibited conversion | No EPSG:32720 relabel and no 10,000,000 m northing shift |
| Catalog contradiction | `HORIZONTAL_CS_NAME=UTM, WGS84, UTM ZONE 21`; retained as non-authoritative evidence because it conflicts with both TIFF GeoKeys and MGRS 20LRP |
| Temporal-selection authority | CMR `granules.json`: `2025-01-03T14:02:38.837Z` to `2025-12-29T14:25:53.638Z` |
| Richer provider evidence | UMM reports finer-precision endpoints (`.837653Z` to `.638110Z`), retained separately rather than silently substituted |
| Independent-source cap | 8 MiB per TIFF, 2 MiB per HTTP range; only LP DAAC, URS, and LP DAAC CloudFront redirect origins admitted |
| Acquisition settings | 128 MiB explicit buffer; 64-pixel chunk edge |

The qualification is not a claim that EPSG area-of-use language overrides an
observed native product grid. It tests that delivery preserves the producer
grid and raw arrays exactly, including its negative y coordinate values.
