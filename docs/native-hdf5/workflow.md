# Native HDF5 / NetCDF-4 array acquisition

Run these commands from the checkout on `codex/acquisition-planner`. This
session reused `/Users/jakegearon/.codex/worktrees/acquisition-planner/tile-ripper`.

This is an opt-in reader for the [precise admitted profile](report.md). GDAL
remains the existing default feature. The HDF5 adapter requires neither GDAL,
libhdf5, libnetcdf nor Python in the Rust runtime.

```sh
cargo build --release --locked --no-default-features --features native-hdf5 \
  --bin tile-ripper --example hdf5_subset_oracle
./target/release/tile-ripper plan docs/native-hdf5/goes19-request.json --human
./target/release/tile-ripper stream docs/native-hdf5/goes19-request.json --gdal-free
mkdir -p prototype-output/native-hdf5/demo
./target/release/examples/hdf5_subset_oracle \
  docs/native-hdf5/goes19-request.json dump prototype-output/native-hdf5/demo \
  > prototype-output/native-hdf5/demo/stdout.json
```

The example requests `/RRQPE[2400:2784,2400:2784]` from one archived NOAA GOES-19
NetCDF-4 object. It streams native UInt16 rainfall codes. Packing attributes
provide millimeters per hour; the original code values remain unchanged. The
provided observation label is the independent CF decoding of `/t`:
`2025-04-10T12:05:06.527315Z`. The reader preserves the original Float64 time and
its units; it does not infer calendars or filter by time.

The new request selector uses **dataset dimension order**, `[row, column]`:

```json
{
  "kind": "array_slice",
  "start": [2400, 2400],
  "count": [384, 384],
  "step": [1, 1],
  "axes": ["/y", "/x"]
}
```

Use `format: "hdf5"`, absolute `Variable::Dataset` paths, `grid: native`,
`time: null` and a callback sink. All counts and steps must be positive. The
last requested index must lie inside the dataset. Each supplied coordinate
path must address a supported one-dimensional array matching that dimension.
Paths are explicit user declarations; this adapter does not resolve dimension
scales into inferred coordinate choices. Mixed HDF5/raster requests and file
sinks fail preparation. `chunk_edge` remains a common request guard; HDF5 output
pieces follow intersections with source chunks rather than a rechunking grid.

`Prepared::execute`, `into_stream` and `ChunkStream::finish()` retain the shared
lifecycle. Every emitted chunk is provisional until successful `finish()`.
Cancellation and errors invalidate completion; blocking HTTP requests already
in flight can finish before cancellation takes effect. Dropping joins the
producer. Retaining chunks in a callback remains the caller's memory use.

`Chunk.window.x/y` are the first source indices and width/height count delivered
samples. For strides, subsequent samples advance by the request's steps.
`Chunk.coordinates` carries exact **stored** axis values represented as Float64;
all admitted coordinate integers fit exactly. `pixel_center()` returns those
values. The `HDF5:stored_axes:...` label identifies axis paths; the affine fields
are index-space placeholders. Use axis packing and projection metadata in
`explain().sources[].object_identities.datasets[]` for geolocation. GOES x/y
storage codes need their own scale and offset to become viewing angles. A
consumer must not interpret this label as an EPSG CRS or a geographic affine.
The public `Chunk` fields and `Pixels` variants are unchanged. Only the
explicit axes and optional root `/t` values are acquired automatically; other
auxiliary-coordinate references remain attributes for explicit acquisition.

Provenance includes variable/group path, source dtype and shape, chunk shape,
axis paths/types/attributes, optional grid mapping and `/t`, physical ranges,
ETag/size and scientific attributes. Attribute raw bytes, shape and type are
retained even when array/reference values are not interpreted. UInt16/Float32
samples preserve storage values, byte-order-independent numbers, signed zero
and Float32 NaN payload bits. Scientific scale/offset is never applied by the
Rust reader. Validity follows a single CF scalar missing marker and NaNs;
HDF5 storage initialization alone does not invalidate a value. Files combining
CF `_FillValue` with implicit HDF5 initialization fail explicitly because the
upstream reader can substitute the attribute for the actual default. CF quality and
valid-range rules remain preserved attributes for explicit consumer analysis.

## Physical reads and policies

The upstream v1 B-tree collector scans the **entire chunk index**, then filters
entries. Preparation retains those bounded metadata ranges for reuse. It does
not fetch unselected scientific chunks or stage the whole object. Dense
attributes are read using the library's heap/attribute API. Other chunk-index
families fail explicitly. A file whose index/metadata exceeds the budget is
rejected. There is no larger/full-file fallback.

`TILE_RIPPER_HDF5_RANGE_POLICY` accepts:

| Policy | Payload retrieval |
| --- | --- |
| `individual` | One compressed range per present logical chunk |
| `adjacent` (default) | Join physically adjacent chunks in consecutive logical order; zero gap overfetch |
| `bounded` | Also allow gaps up to 16 KiB and extra bytes up to 25% of selected bytes |

Merged spans are at most 256 KiB and at most `buffer_bytes / 16`, with up to 32
members. A larger individual chunk still requires successful working-memory
admission. HDF5 settings are separate from Zarr settings. Groups stay inside
one dataset/file binding, preserve delivery order, and release all encoded
buffers/decoder state before releasing admission. Encoded bytes are reused
only within a physical group; no persistent cache is introduced.

Metadata reads and entries are bounded before payload retrieval. Cache bytes
and reader/codec/output work are charged conservatively; aggregate prepared
state must fit half the buffer, and pending chunk-plan bookkeeping has its own
buffer-based cap. The scheduler's reservation is a working-memory model, not a
process RSS limit. Runtime, HTTP machinery, explain/receipt copies, the bounded
DAG and caller retention are separately relevant. Stream queue capacity adds
its documented output-buffer allowance; examples use rendezvous capacity 0.

Transport uses existing strong ETags, conditional exact ranges, retry rules,
strict framing and pre/post validation. `transport.phases` separates
`hdf5_chunk_index_discovery` from other preparation and payload execution.
`source_data_requests/bytes` counts payload GETs including policy overfetch.
Actual successful/error response-body bytes, attempts and retries are retained.
Transport counters accumulate for a prepared acquisition; repeat executions
reacquire payloads.
ETags pin identity; they do not authenticate arbitrary raw bytes. DEFLATE
checks and available Fletcher32 checks are verified by the established decoder.

## Independent Python verification

The oracle writes each native chunk's bytes and validity mask, followed by one
JSON receipt **after** successful finish. Compare them with h5py:

```sh
uv run --no-project --python 3.12 --with h5py --with netCDF4 \
  --with requests --with psutil python bench/hdf5_verify.py \
  'https://noaa-goes19.s3.amazonaws.com/ABI-L2-RRQPEF/2025/100/12/OR_ABI-L2-RRQPEF-M6_G19_s20251001200211_e20251001209519_c20251001210009.nc' \
  prototype-output/native-hdf5/demo/stdout.json prototype-output/native-hdf5/demo
```

Redirect the oracle's stdout to `demo/stdout.json` when using that command.
The Python reader uses bounded conditional ranges for a URL. It compares every
native value bit, axis value and packing attribute and computes an unpacked
mean from emitted chunks. No intermediate whole-file copy is required for
that analysis. The benchmark's separately labelled full-file reference control
exists only for independent comparisons and is never an acquisition fallback.

## Validation recipes

Create a reference environment from `docs/native-hdf5/reference.txt`, or use
`uv run --no-project --python 3.12 --with-requirements docs/native-hdf5/reference.txt`
before each Python command below:

```sh
python bench/hdf5_input.py science
python bench/hdf5_input.py faults
python bench/hdf5_input.py public
python bench/hdf5_resources.py
python bench/hdf5_input.py benchmark
python bench/hdf5_summary.py
```

`public` is opt-in and retrieves one small NOAA object for its explicitly
labelled full-file oracle, separately from the native selective request. All
other policy/fault/resource tests use loopback or local synthetic files.
`public --reference PATH` explicitly reuses a frozen independent whole-file
control whose size and MD5 match the native pinned object, avoiding another
whole-file provider transfer. Native acquisition still uses fresh ranges. The
index control reuses that frozen real-product file. Timing experiments must run
without concurrent builds or validation work. A `TILE_RIPPER_HDF5_BINARY` path
can select a frozen copy of the release oracle; each cell records its SHA256.

Rust checks use `cargo test --locked` and warning-denying `cargo clippy
--locked --all-targets -- -D warnings` under default, `--no-default-features`,
native-cog, native-hdf5 and `--all-features`. Native features use
`--no-default-features --features NAME`.

Inherited regressions use `bench/hdf5_regressions.py` with `remote`, `cf`,
`cog-stream`, `checks` and `faults` in the existing Zarr **v2** reference
environment. `v3-checks` and `v3-faults` use the Zarr **v3** reference environment
recorded in `docs/zarr-v3-input/reference.txt`. They require fresh ignored
work directories. Build their GDAL-free binaries with:

```sh
cargo build --release --locked --no-default-features \
  --features native-cog,native-hdf5,experimental-output \
  --bin tile-ripper --example zarr_subset_oracle --example native_cog_oracle \
  --example remote_zarr --example durable_output_candidate \
  --target-dir target/hdf5-regression
cargo build --release --locked --manifest-path consumers/native-stats/Cargo.toml \
  --no-default-features --features native-cog --target-dir target/hdf5-regression
```

Their fresh receipts are under `docs/native-hdf5/inherited/`; prior milestone
receipts stay untouched. The dgov plan was structurally compiled with its
identity bundler and executed directly in the existing worktree. Automated
agent dispatch/merge was not used.
