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
| `products` | Print named profiles, array descriptors and supported acquisition routes as JSON. Also accepts `--json`. |
| `discover` | Find source items. Use `--product` for a named product, or `--collection` to search SWOT (Surface Water and Ocean Topography) Raster granules in the NASA Common Metadata Repository (CMR). |
| `discover-job` | Build a SWOT job from a selection file. Writes the job to `--output`; review JSON goes to standard output. |

`discover` also takes `--bbox`, `--start`, `--end`, `--limit` and `--output`, and takes no credentials. `--bands` applies to `--product`. `discover-job` also takes `--auth`, `--netrc` and `--output`.

## Supported routes

Run `tile-ripper products --json` before constructing a new request. The existing
profile lists are retained; the additive `routes` description identifies the
generic STAC adapter, explicit COG inputs and named-product provider routes. It
reports build features, access requirements and the limits of the Chronozarr
handoff. The listing is available without contacting a provider.

The generic STAC adapter supports Earth Search's `sentinel-2-c1-l2a` collection.
Planetary Computer's `sentinel-1-rtc` is a different collection and is not admitted
by that adapter. The named `opera-rtc-s1` profile is a separate OPERA product.
Explicit COG inputs require a JSON request with `source.kind: "assets"` and
`assets[].format: "cog"`, as in `examples/cog-request.json`. There is no
`--format cog` convenience flag. They use the URLs supplied in that request; an
object version pin does not supply authorization to read that object.

Chronozarr's `convert_tile_ripper` bridge requires completed, ordinary
uncompressed per-observation COG Zarr v2 output with the required scientific
source metadata. The selected observations must share one exact native grid and
collection, compatible band calibration and unique acquisition times. Consult
the route's bridge description before choosing an output. See the
[Chronozarr bridge contract](https://github.com/chronozarr/chronozarr/blob/main/docs/tile-ripper.md).

## Options

### Selection

Do not combine these with a request file.

| Flag | Meaning |
| --- | --- |
| `--source URL_OR_PATH` | Zarr store, or the endpoint of a catalog for `--format stac`. |
| `--format zarr_v2\|stac` | `zarr_v2` for a Zarr store; `stac` for a SpatioTemporal Asset Catalog (STAC) API. |
| `--collection NAME` | STAC collection. Required with `--format stac`; not valid with `zarr_v2`. |
| `--bbox W,S,E,N` | Bounds in EPSG:4326 (WGS84 longitude and latitude). Crossing the antimeridian is not supported. On a Zarr source whose longitudes run 0–360, such as ERA5, give 0–360 values; a negative longitude fails. Use this or `--aoi`, not both. |
| `--aoi FILE` | Area of interest (AOI): a GeoJSON Polygon geometry, not a Feature or collection. Zarr sources accept `--bbox` only. |
| `--start RFC3339 --end RFC3339` | Inclusive time bounds in RFC 3339 form, such as `2020-06-01T00:00:00Z`. Start must not be later than end. |
| `--variables NAME[,NAME]` | Array names for Zarr sources; band names for STAC. |

### Output

| Flag | Meaning |
| --- | --- |
| `--output NEW_DIRECTORY` | Destination for `fetch`; required with selection flags. Choose a new path; existing paths are refused. |
| `--output-format zarr_v2\|geotiff\|native` | Output format. Needs a directory destination: `--output`, or a request file with a directory sink. See Output format below. |
| `--human` / `--json` | Readable summary/errors, or JSON (the default). If both are supplied, the last mode flag wins. |
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

## JSON failures

In default/`--json` mode, a failed command exits with status 1 and writes one JSON
error document to **standard error**. Successful JSON continues to use standard
output. Failed streams can already have emitted provisional chunk records; only
a successful final completion record makes those chunks usable.

The error document has `schema_version: 1`, `document_type: "error"` and an
`error` object. That object includes a stable `code`, `stage`, safe `message`,
`retryable` and `http_status`. A null retryability or status means it is unknown
or unavailable, not a successful request. Typed failures can include a sanitized
source reference and additional details. Credentials and signed access grants
are excluded from the JSON diagnostic.

The first typed cases are:

| Code | Meaning |
| --- | --- |
| `INVALID_ARGUMENTS` | The CLI request or options are invalid. |
| `CONFIGURATION_ERROR` | A required reader/build configuration is unavailable. |
| `UNSUPPORTED_STAC_COLLECTION` | The collection is outside the supported STAC adapter; details identify supported collections. |
| `SOURCE_IDENTITY_HTTP_ERROR` | The provider's response prevented source identity validation; `http_status` records the actual response. |
| `SOURCE_IDENTITY_TRANSPORT_ERROR` | Source identity validation failed at the transport layer; no HTTP status may be available. |
| `OPERATION_FAILED` | An otherwise untyped failure. Retryability is unknown; arbitrary underlying error text is withheld from this JSON fallback. |

Retryability describes whether another attempt may be appropriate. It does not
retry a command, switch providers or replace an accepted source pin. The existing
bounded request retries and source-version checks still apply.

Use `--human` for readable diagnostics. For automation, capture the two streams
separately:

```sh
tile-ripper plan request.json --json > plan.json 2> error.json
```

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
