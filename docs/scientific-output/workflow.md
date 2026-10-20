# GDAL-free Sentinel scientific output

The ordinary CLI can discover Sentinel-2 observations by polygon, date and band,
stream native UInt16 pixels into Zarr v2, and publish a portable local dataset.
It uses the existing `zarrs 0.22.10` writer. No intermediate GeoTIFF, whole-scene
buffer, raster reprojection, resampling or mosaicking is involved.

## Install and acquire

From this source checkout, with Rust 1.92, a C/C++ compiler, CMake, pkg-config and
CA certificates:

```sh
cargo install --path . --locked --no-default-features --features native-cog \
  --root /absolute/path/to/new-native-install
```

GDAL, PROJ, libtiff and Python are unnecessary for acquisition. The binary still
uses the platform C/C++ runtime and bundled codec/TLS dependencies. macOS ARM64
and Linux ARM64/glibc are the exercised platforms. See [installation](../installation.md).

Save this GeoJSON **Polygon geometry** as `polygon.geojson`:

```json
{"type":"Polygon","coordinates":[[[-79.06,35.91],[-79.05,35.91],[-79.052,35.92],[-79.06,35.91]]]}
```

```sh
tile-ripper fetch \
  --source https://earth-search.aws.element84.com/v1 --format stac \
  --collection sentinel-2-c1-l2a --aoi polygon.geojson \
  --start 2025-01-02T00:00:00Z --end 2025-01-13T23:59:59Z \
  --variables B04,B08,B11 --gdal-free --workers 2 --buffer-mib 16 \
  --output ./sentinel.zarr --human
```

Put the isolated installation's `bin` directory in PATH or use its full executable
path. The validated request produced six observation groups: B04/B08/B11 on
2 and 12 January 2025, including native 10 m and 20 m grids. Catalog contents can
change; inspect the reported source identities. `plan` with these same flags and
`--output` explains the intended groups without acquiring pixel payloads.

The bundled JSON request also works, with a different date and two bands:

```sh
tile-ripper fetch docs/native-sentinel/request.json --gdal-free \
  --output ./another-sentinel.zarr
```

Direct flags retain the existing eight-item/eight-asset/one-million-output-pixel
limits. Use the existing JSON schema for larger explicit limits and chunk sizes.
Native profile and geographic restrictions remain those in the
[Sentinel workflow](../native-sentinel/workflow.md). Unsupported rasters fail;
there is no automatic fallback to a GDAL reader.

## Open with independent tools

Python is optional and only needed here for analysis:

```python
import xarray as xr
import zarr

root = zarr.open_group("sentinel.zarr", mode="r")
print(root.tree())
ds = xr.open_zarr(
    "sentinel.zarr", group="observations/000000", consolidated=False,
    chunks=None, mask_and_scale=False,
)
print(ds.time.values, ds.attrs["source"]["requested_band"])
raw = ds["values"]                  # stored UInt16, no automatic calibration
valid = ds["valid"] == 1
analysis_values = raw.where(valid)
```

Zarr-Python 2.18.3 and 3.1.3 with xarray 2025.6.1 were independently tested. `mask_and_scale=False`
keeps the stored dtype and both 0/1 masks; time still decodes normally. With xarray's
default CF decoding, `scale_factor`, `add_offset` and source fill may instead
produce calibrated floating-point values. Calibration is never applied by the
Tile-Ripper writer. The [validation receipt](validation.json) records independent
GDAL/PROJ and xarray checks, including microsecond acquisition timestamps.

## Layout and scientific semantics

The destination is an ordinary, unconsolidated Zarr v2 root group:

```text
sentinel.zarr/
  .zgroup, .zattrs, manifest.json
  observations/
    000000/                         # one source asset and band
      .zgroup, .zattrs
      values/                       # UInt16 [time=1, y, x]
      valid/                        # UInt8 [1, y, x], 0 invalid, 1 valid
      aoi_mask/                     # UInt8 [1, y, x], 0 excluded, 1 included
      x/, y/                        # Float64 native pixel centers
      time/                         # Int64 nanoseconds since Unix epoch
      spatial_ref/                  # scalar CF grid mapping and exact source CRS
    000001/ ...
```

Groups are ordered by acquisition instant, asset ID and numeric source band.
Their source item, requested band, asset key, timestamp, footprint and object
version are explicit attributes. Direct COG requests retain their numeric band
and source identity; an undated direct asset has a missing time coordinate.
Each observation retains its own native CRS, affine, dimensions and selected
source pixel window. No equivalence of neighboring source grids is assumed.
Open groups separately and choose alignment explicitly in analysis software.

`values` contains native stored samples, with the existing pixel-center AOI mask
setting excluded cells to representable source nodata. Skipped wholly excluded
chunks use that same fill. `aoi_mask` distinguishes excluded centers from nodata
inside the AOI; `valid` combines AOI inclusion and source-nodata validity. Windows
are clipped to source coverage: absent source territory is omitted, never padded
into an invented aligned scene. A genuinely empty selection produces an empty
root/observations hierarchy with a complete manifest.

WGS84 UTM grid mappings include standard CF projection and named-datum fields;
independent `pyproj.CRS.from_cf` reconstructs the source EPSG identity. `x`/`y`
are affine pixel centers in the native coordinate units. The exact six-term GDAL
affine and source CRS are retained in group attributes and `spatial_ref`.
Rotated grids are rejected by this output adapter. Other direct unrotated CRS
strings are preserved, but CF parameter generation is limited to WGS84 UTM and
EPSG:4326; arbitrary CRS interoperability is not claimed.

TIFF scale/offset take precedence for CF calibration attributes; STAC raster-band
fields supply an absent TIFF field. Both originals, acquisition/processing
metadata and source-version identities remain in provenance. This does not
resolve conflicting provider calibration or create reflectance composites.

## Incremental execution and completion

The adapter consumes the existing native `ChunkStream` with rendezvous delivery,
then requires successful `finish()`. It verifies every planned delivery, writes
values and masks immediately through zarrs, and streams coordinate vectors in
bounded slices. It retains no scene-sized pixel buffer. Destination chunks are
limited to one quarter of `buffer_bytes` in UInt16 payload; explicit writer
buffers are separately reported. Metadata, plans, zarrs bookkeeping and runtime
allocations remain outside the scheduler's working reservations. `buffer_bytes`
is not a process RSS limit. Output object bookkeeping is admitted against
`max_nodes` before creating a staging directory.

All new COG-to-Zarr work occurs in a unique hidden sibling such as
`.sentinel.zarr.partial-PID-ID`. Every zarrs object write synchronizes its file.
After stream completion, the writer finalizes metadata and `manifest.json`,
synchronizes staged directory entries, then publishes through an atomic
**no-replace** rename on macOS/Linux. The destination can only refer to completed
data. Existing files, directories and symlinks are refused, including a destination
created by another process just before publication. No automatic replacement or
backup policy is provided; choose a new name.

Acquisition errors, consumer/write errors, final source-version conflicts and
cooperative SIGINT/SIGTERM/SIGHUP remove the owned staging directory and return
failure. Cancellation can wait for an in-flight bounded HTTP operation to return.
If cleanup fails, the error identifies the incomplete staging path. SIGKILL or
power loss may retain a hidden sibling; retries ignore it and never adopt it.
Inspect such a sibling before removing it. Trust only the requested published
destination with its complete manifest and a successful CLI exit. There is no
resumption of interrupted writes. Parent-directory sync after publication is
best effort: power-loss name durability and nonlocal filesystem semantics are
not certified. Per-object ETags/local size and mtime checks are inherited; they
are not an atomic snapshot of a changing catalog or collection of objects.

Default GDAL GeoTIFF output and existing direct-Zarr acquisition retain their
previous layout. In a GDAL build, choose this new COG sink explicitly with
`--output-format zarr_v2` or `sink.kind = zarr_directory`. `--gdal-free` chooses
it for ordinary COG/STAC directory fetches and still rejects a GDAL-linked build.

Evidence and limits: [decision report](report.md), [real Sentinel check](live.json),
[deployment](deployment.json), [performance](benchmark.json),
[COG/Zarr/streaming regressions](regressions.json).
