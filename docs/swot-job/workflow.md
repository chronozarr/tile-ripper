# Explicit SWOT acquisition jobs

Build one independently readable scientific Zarr v2 dataset from one to four
explicit native observations. Acquisition requires Rust and the native CLI;
Python is used only for independent verification and ordinary downstream analysis.

```sh
cargo build --locked --release --no-default-features \
  --features native-hdf5,authenticated-http --bin tile-ripper

./target/release/tile-ripper plan examples/swot-job.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint ./swot-job-checkpoint --checkpoint-mib 8 \
  --checkpoint-entries 2048 > job-plan.json

./target/release/tile-ripper fetch examples/swot-job.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint ./swot-job-checkpoint --checkpoint-mib 8 \
  --checkpoint-entries 2048 --output ./swot-subsets.zarr > job-receipt.json
```

Use a private, caller-owned netrc file already authorized for Earthdata. Its
contents stay outside the manifest, plan, checkpoint and output. Authentication
is explicitly opted in; there is no environment-based credential discovery.
Neither `--auth` nor `--checkpoint` is required for ordinary local fixture files.
`--auth configured --auth-policy policies.json --auth-ca ca.pem --netrc FILE`
supports existing exact-object HTTPS policies; the public policy file contains
`{"sources": [CONFIG_FOR_SOURCE_1, CONFIG_FOR_SOURCE_2]}`. Every canonical source
must have exactly one matching policy. Redirect edges and credential forwarding
use the existing access contract unchanged.

The example pins the already qualified held-out UTM01W selection and original
UTM19T selection, in that order. Their native grids and geographic regions
differ. These observations are not an aligned time series. Optional asset ETags
assert an expected source version and fail if it differs. The checked-in ETags
record the acceptance versions; a changed provider object requires deliberate
review of the caller's request and a new checkpoint namespace.

`plan` inspects metadata and can persist verified metadata/coordinate ranges
when checkpointing is requested. It reports native source dimensions, selected
windows/strides, exact coordinate definitions, variables, quality, CRS, source
identity, decoder diagnostics, output groups, memory admission, disk allocation,
known costs and unsupported operations. Inspection JSON is not replayable.
`--human` gives a short summary and points to the full JSON details. Jobs accept
`plan` and `fetch`; their source streams are an internal implementation detail.

## Manifest

See [the complete runnable manifest](../../examples/swot-job.json). The versioned
contract has `job_schema: 1`, `profile: "swot_raster_d_pgd0"`, an `observations`
array and aggregate `limits`. Each observation has a stable `id`, an existing
typed `Request` and an explicit `quality_policy` from the existing SWOT API.
Its request contains exactly one HDF5 asset, `/y` and `/x` index selection,
positive counts and strides, the five qualified variable paths, a native grid,
callback sink and source budget. The callback sink describes source delivery;
the job supplies the scientific output adapter. Source root metadata must
identify PGD0, the raw types/quality definitions must match, and explicit source
`/crs` mapping attributes and WKT must be present.

Execution, group listing and receipts follow manifest order, including input
supplied out of time order. IDs are unique ASCII identifiers, 1–64 characters,
using letters, digits, underscore or hyphen. Repeated canonical sources are
rejected, including a repeated source under a new ID. Local aliases are resolved
when checking duplicates. Use separate jobs for repeated acquisitions of the
same object. Distinct sources remain distinct even when their timestamps or
values coincide. No observations are silently merged.

Manifest JSON is limited to 64 KiB; source plans have at most 4,096 nodes. Each
source inspection is limited to 256 KiB serialized metadata and each completion
receipt to 512 KiB. Output/result bookkeeping is bounded separately. Unknown
fields, credential fields, signed/query-bearing source URLs, unsupported
processing releases, invalid quality definitions, geographic selections,
resampling, time filtering and empty index counts fail. A nonempty selection
with no accepted pixels is valid and preserves all raw arrays.

## Aggregate admission

Execution is serial and synchronous. The next decoder cannot run while the
writer retains the current tile. No stream queue, completed-observation cache
or caller-retained tile exists in the job executor.

For every entry, admission enforces:

```
source Request.budget.buffer_bytes
  + 32 MiB bounded job/metadata/receipt reservation
  + writer reservation
  <= limits.memory_bytes

writer reservation = edge * edge * 32 + edge * 32 + 65,536 bytes
```

The edge is `limits.output_chunk_edge`, between 1 and 1,024. It bounds every
encode call independently of source tile size, including destination chunk
read/modify/write scratch. Raw data, independently computed source masks and
derived quality are written in bounded pieces. Every nonempty raw chunk is
stored explicitly, including fill-equivalent chunks, to preserve NaN payloads.

The existing source admission includes its active decoder, encoded-range cache,
prepared metadata/coordinates, authentication session, checkpoint index/write
scratch and tile assembly. Those resources are released before the next source
is prepared. The 32 MiB job reservation covers the bounded metadata and receipts
that survive across observations, output array metadata, serialization and
coverage records. Limits are refused rather than increased automatically.
Per-source inspection/execution reports retain the limiting decoder variable,
component sizes and sufficient source-budget estimate. A source budget is a cap;
the observed working reservation can be smaller.

These are conservative working reservations, not a process RSS ceiling.
Allocator, TLS/HTTP runtime, OS/filesystem cache and caller-owned input objects
remain outside that reservation model; the acceptance harness samples process
RSS separately. `observed.working_reservation_peak_bytes` combines the source
decoder/delivery execution peak with the job/writer reservations. Coordinate
preparation diagnostics are retained separately; the admission cap above bounds
the full lifecycle. This field is not a measurement of allocated heap.

Output file keys have a separate aggregate
`limits.max_output_objects` admission limit. This milestone does not add an
output-byte or free-space reservation: output filesystem failures abort the
transaction, and source checkpoints remain available for retry.

## Checkpoint ownership and restart

CLI `--checkpoint-mib` and `--checkpoint-entries` are **aggregate job limits**.
`--checkpoint-entry-mib` remains a maximum individual source-range size. The
default aggregate limits are 64 MiB and 2,048 range records; the example requests
8 MiB. The job creates a private `tile-ripper-job-v1` namespace under the supplied
private directory and holds its exclusive lock for the entire operation.

The job identity fixes observation IDs, order, canonical sources, expected
versions, selections, normalized variable paths and explicit interpretation/
quality policy. It excludes destination names, output chunk edge and resource
limits. Each acquisition gets a separately locked, version-bound source
namespace named by its identity digest.

After charging the root identity, remaining logical disk bytes are divided
equally among sources; range entry limits are divided equally too, rounding
down. Identity files and range headers count toward disk quotas. At most one
range write is active. Pending bytes are admitted against the same source quota.
All retained source namespaces are inventoried against current quotas before
any source adds bytes, so lowering a limit cannot hide another source's retained
usage. Filesystem blocks, directory/inode overhead and unrelated caller files
are outside these logical-byte counters. There is no quota borrowing, eviction,
unbounded persistent cache or unrelated-file cleanup.

Source checkpoint ranges survive success, failure and cancellation. The caller
owns explicit cleanup after dropping the job; removal is never automatic. The
root permits only the same acquisition set/order/policy. A changed acquisition
identity or source generation requires a fresh directory. Larger limits may be
requested explicitly on retry; smaller limits must accommodate all retained
files first. Single-source legacy checkpoints have their own namespace and are
not imported or scanned by the job.

On restart, create a new output destination and use the same source-checkpoint
directory. Each source receives a fresh authenticated session, fresh canonical
version checks and an online authorization proof before reuse. Every reused
range is verified. All scientific delivery and calculations restart at zero;
all output arrays are regenerated. Nothing appends to an earlier calculation.
There is no exactly-once processing, offline success, partial-output resume or
persistent completed-observation reuse.

## Output and completion

The destination has a root group, `observations/<id>` groups and `manifest.json`.
Each native observation contains:

| Array | Stored output type | Meaning |
| --- | --- | --- |
| `wse`, `wse_uncert` | Float32 | Raw source values, including fills/NaN bits |
| `wse_qual` | UInt8 | Raw summary quality |
| `wse_qual_bitwise`, `n_wse_pix` | UInt32 | Raw flags and contributing counts |
| `source_valid_<variable>` | UInt8 | Independent source missingness for each raw variable |
| `quality_mask` | UInt8 | Separately identified derived policy, 0 excluded / 1 accepted |
| `x`, `y` | Float64 | Exact selected native coordinate values |
| `t`, when present | Float64 scalar | Raw source time; original units/calendar retained |
| `spatial_ref` | Int32 scalar container | Source CRS attributes; scalar zero is metadata scaffolding |

Raw auxiliary values are never replaced by quality-masked ones. Source units,
fills, packing attributes, quality definitions, CRS, processing identifiers,
root times and per-object provenance are retained. Array/group
`source_attributes` retain typed source attribute descriptions and raw bytes,
including uninterpreted storage attributes. Destination dimension annotations
remain valid even when a source attribute has the same name. Index selection
has no AOI mask, and the derived quality mask is never labeled source validity.
There is no calibration, reprojection, resampling, mosaic or implicit alignment.

Fetch writes only into a newly owned sibling staging directory. Every source
must deliver complete, nonoverlapping coverage and pass its normal pre/post
execution validation. After all observations are written, a serial sweep opens
each source again with a fresh caller-owned session, compares its complete
identity and performs a fresh authorization GET plus final conditional HEAD
(or local stat validation). The sweep happens before final root metadata,
manifest writing and publication synchronization. Validation timings and
transport counters are in each source receipt.

These checks establish individual source versions over their validation
intervals. They cannot make an atomic snapshot of a changing NASA collection;
objects can change between checks or after their final validation. A mutation
of an earlier source during a later acquisition is tested and refuses the job.

Only an entirely completed job receives a complete manifest and checked atomic
no-replace rename to the requested destination. Existing files, directories and
links are refused; a racing creator is preserved. Ordinary failure/cancellation
removes owned staging and retains source checkpoints. Abrupt process death can
leave an owned `.partial-*` sibling; it is never reused or automatically swept.
Restart creates fresh staging. The caller can remove a known abandoned stage
after confirming its owning process is gone. CLI failures return a
nonzero status and, once fetch has started, a bounded JSON failure receipt on
stdout. Per-source deliveries in it remain provisional. Signal cancellation is
shared with acquisition/writing; preparation still uses the existing bounded
request timeouts before its next cancellation check.

Publication reuses the existing file synchronization, bottom-up directory sync,
pre-publication device request, no-replace rename, parent sync and final device
request. Rename is the commit point. If synchronization fails after rename, the
complete destination is retained and the error explicitly reports unconfirmed
durability. Job cancellation and publication races are tested on macOS/Linux ARM64;
the existing checkpoint evidence also tests abrupt persistence-boundary exits.
These checks do not establish power-loss durability. The returned CLI/API
receipt adds publication/total timings that cannot appear in their own
pre-publication manifest.

## Open with ordinary tools

```python
import xarray as xr
import zarr

root = zarr.open_group("swot-subsets.zarr", mode="r")
print(root["observations"].group_keys())

ds = xr.open_zarr(
    "swot-subsets.zarr",
    group="observations/held_out_20240217",
    consolidated=False,
    mask_and_scale=False,
    decode_times=False,
)
assert str(ds.wse_qual_bitwise.dtype) == "uint32"
accepted_wse = ds.wse.where(ds.quality_mask.astype(bool))
```

`mask_and_scale=False` and `decode_times=False` preserve the stored numeric
representation on opening. Apply source calibration or CF time interpretation
explicitly if the analysis needs it. This example applies the stored derived
mask in a new analysis expression; it does not alter raw output arrays.

## Rust and verification

The typed public entry points are `swot_job::Manifest`, `Job::new` for local/
anonymous sources and `Job::authenticated` with a caller-owned exact-object
session factory plus optional aggregate checkpoint options. `Job::plan` and
`Job::fetch` implement the same path as the CLI. Fetch accepts a shared
`Arc<AtomicBool>` cancellation flag. See [the compiled example](../../examples/swot_job.rs):

```sh
cargo run --locked --release --no-default-features \
  --features native-hdf5,authenticated-http --example swot_job -- \
  fetch examples/swot-job.json ./rust-swot.zarr "$HOME/.netrc" ./rust-checkpoint
```

For deterministic local gates, use Python 3.10 and
`pip install -r bench/swot-job-requirements.txt`, then:

```sh
python bench/swot_job.py offline
cargo test --locked --no-default-features --features native-hdf5,authenticated-http
cargo clippy --locked --all-features --all-targets -- -D warnings
```

The offline real-product gates require the preserved pinned captures named in
the harness. Native held-out reads use the original capture; guarded references
use the corrected separate reference capture. Missing reference extents are
fatal. `python bench/swot_job.py live` is an explicit bounded provider operation,
separate from offline tests. Raw runs/datasets/traces stay under
`prototype-output/swot-job/`; compact receipts are versioned here. See
[the decision report](report.md) for exact acceptance evidence and limitations.
