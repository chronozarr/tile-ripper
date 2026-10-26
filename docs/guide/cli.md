# CLI reference

A request comes from a JSON request file or from selection flags, never both. Output is JSON by default; add `--human` for a readable summary.

Commands assume `tile-ripper` is on your PATH. From a source checkout, use `./target/release/tile-ripper`. Run from the repository root, because bundled example paths such as `examples/weatherbench2.json` are relative to it:

```sh
tile-ripper plan examples/weatherbench2.json --human
```

## Commands

| Command | Purpose |
| --- | --- |
| `plan` | Inspect a request without writing output. Reads metadata and source identities and can use the network. |
| `fetch` | Write the selected subset to a new directory. |
| `stream` | Print chunk records as they are read. The final completion record ends the stream. |
| `products` | Print the named product profiles and array descriptors as JSON. |
| `discover` | Find source items. Use `--product` for a named product, or `--collection` to search SWOT (Surface Water and Ocean Topography) Raster granules in the NASA Common Metadata Repository (CMR). |
| `discover-job` | Build a SWOT job from a selection file. Writes the job to `--output`; review JSON goes to standard output. |

`discover` also takes `--bbox`, `--start`, `--end`, `--limit` and `--output`, and takes no credentials. `--bands` applies to `--product`. `discover-job` also takes `--auth`, `--netrc` and `--output`.

## Options

### Selection

Do not combine these with a request file.

| Flag | Meaning |
| --- | --- |
| `--source URL_OR_PATH` | Zarr store, or the endpoint of a catalog for `--format stac`. |
| `--format zarr_v2\|stac` | `zarr_v2` for a Zarr store; `stac` for a SpatioTemporal Asset Catalog (STAC) API. |
| `--collection NAME` | STAC collection. Required with `--format stac`; not valid with `zarr_v2`. |
| `--bbox W,S,E,N` | Bounds in EPSG:4326 (WGS84 longitude and latitude). Crossing the antimeridian is not supported. Use this or `--aoi`, not both. |
| `--aoi FILE` | Area of interest (AOI): a GeoJSON Polygon geometry, not a Feature or collection. Zarr sources accept `--bbox` only. |
| `--start RFC3339 --end RFC3339` | Inclusive time bounds in RFC 3339 form, such as `2020-06-01T00:00:00Z`. Start must not be later than end. |
| `--variables NAME[,NAME]` | Array names for Zarr sources; band names for STAC. |

### Output

| Flag | Meaning |
| --- | --- |
| `--output NEW_DIRECTORY` | Destination for `fetch`; required with selection flags. Choose a new path; existing paths are refused. |
| `--output-format zarr_v2\|geotiff\|native` | Output format. Needs a directory destination: `--output`, or a request file with a directory sink. See Output format below. |
| `--human` / `--json` | Readable summary, or JSON (the default, stable for scripts). |
| `--dry-run` | Same as `plan`. Metadata may still download. |
| `--cache FILE` | Metadata cache. Loaded if the file exists; saved after the run. |
| `--discover-only` | With `plan` only. Runs discovery and skips preparing the request. |
| `--values` | Adds native values to stream records, as base64 little-endian bits plus masks and axes. Works only for named-product streams and SWOT streams with `--quality-policy`. |
| `--quality-policy POLICY_JSON` | SWOT only: an aligned five-layer quality policy for `plan` and `stream`. |

### Performance

| Flag | Meaning |
| --- | --- |
| `--workers N` | Concurrent reader operations, from 1 to 64. |
| `--per-host N` | Concurrent reader operations per host, from 1 to 64. |
| `--buffer-mib N` | Working buffer for payload bytes. Default 32. Not a process memory limit. |
| `--backend native\|gdal` | Cloud-optimized GeoTIFF (COG) reader. Overrides `TILE_RIPPER_COG_BACKEND`. `gdal` uses the Geospatial Data Abstraction Library (GDAL). |
| `--gdal-free` | Require a build without GDAL. Uses the native COG reader when it is compiled in. |
| `--provider auto\|planetary_computer\|earthdata\|pinned` | Route override for named product requests. |

### Access and credentials

| Flag | Meaning |
| --- | --- |
| `--auth earthdata --netrc "$HOME/.netrc"` | Earthdata login for native HDF5 (Hierarchical Data Format 5) sources. The netrc file must be private. See [Earthdata login](earthdata-login.md). |
| `--auth configured --netrc "$HOME/.netrc" --auth-policy PUBLIC_JSON` | Advanced. Uses an exact-object policy file. |
| `--auth-ca PEM` | Advanced. PEM certificate authority file for a configured policy. |

Explicit `--auth` and `--netrc` are required together. Credentials are read only from the file you pass to `--netrc`. Tile-Ripper does not read `.netrc`, `NETRC` or `TILE_RIPPER_AUTH_NETRC` on its own.

### Resume

| Flag | Meaning |
| --- | --- |
| `--checkpoint DIRECTORY` | Opt-in resume of authenticated source ranges. Requires explicit `--auth` and `--netrc`. Use a private directory. |
| `--checkpoint-mib N` | Size limit for the checkpoint, in MiB. Requires `--checkpoint`. |
| `--checkpoint-entry-mib N` | Size limit for one checkpoint entry, in MiB. Requires `--checkpoint`. |
| `--checkpoint-entries N` | Maximum number of checkpoint entries. Requires `--checkpoint`. |

### Output format

| Value | Meaning |
| --- | --- |
| `zarr_v2` | Zarr v2 output. GDAL-free COG output defaults to this. |
| `geotiff` | GeoTIFF output. Needs a build with GDAL. |
| `native` | Keep the source's format: Zarr sources are copied as `<asset id>.zarr`; COG/STAC sources are written as GeoTIFF, which needs a GDAL build. |

Named products accept tiled single-band UInt8, UInt16, Int16 and Float32 COGs. Sentinel-2 and direct COG requests accept tiled single-band UInt16 only. Other layouts fail; there is no GDAL fallback.

## Notes

- **Request files.** `plan`, `fetch` and `stream` accept a request JSON file, such as `examples/weatherbench2.json`, which keeps the full request schema. Selection flags (`--source`, `--format`, `--collection`, `--bbox`, `--aoi`, `--start`, `--end`, `--variables`, `--buffer-mib`) cannot be combined with a file. Output, performance and access flags can.
- **Completion.** Use a result only if the command exits successfully and `manifest.json` says `"status": "complete"`. An interrupted run may leave a hidden `.NAME.partial-*` sibling. Never use it.
- **Streaming.** `stream` prints newline-delimited JSON (NDJSON) records. Chunk records are provisional until the final completion record arrives. Do not use streamed values from a run that ends without one.
- **GDAL.** GDAL remains the default backend. `--backend native` in a build that includes both readers still links GDAL. Use `--gdal-free` only with a build that excludes GDAL. The canonical build in [Installation](../installation.md) excludes GDAL.
- **No processing.** Tile-Ripper does not resample or composite data, does not reproject it implicitly, and does not apply calibration. Values keep their stored types and units. See [Output, values and masks](output.md).

## See also

- [Quickstart](../quickstart.md)
- [Supported data](supported-data.md)
- [Output, values and masks](output.md)
- [Earthdata login](earthdata-login.md)
- [Sentinel-2](sentinel-2.md)
- [Landsat, HLS and OPERA](landsat-hls-opera.md)
- [GOES, SMAP and NISAR](goes-smap-nisar.md)
- [SWOT Raster](swot.md)
- [ERA5, IMERG and GFS (Zarr)](zarr.md)
- [Reader selection and Rust API](../native-http/cli.md)
