# Find SWOT Raster D granules and make an explicit native job

Run from the repository checkout on `codex/acquisition-planner`. Build the native
CLI, then search a bounded WGS84 region and RFC3339 interval:

```sh
cargo build --locked --release --no-default-features \
  --features native-hdf5,authenticated-http --bin tile-ripper

./target/release/tile-ripper discover \
  --collection SWOT_L2_HR_Raster_D --bbox -73,41,-70,44 \
  --start 2024-02-16T02:45:00Z --end 2024-02-16T03:00:00Z \
  --limit 20 --output candidates.json
```

Discovery is anonymous CMR metadata access. It accepts no authentication flags,
reads no netrc, follows no redirects, and never opens an archive scientific
object. The collection concept ID is resolved on each search from the exact
short name, version D and POCLOUD provider. `--concept-id ID` optionally asserts
the expected resolved identity. The verified collection and revision are in the
result; the client has no built-in collection concept ID.

Review `status`, `reasons`, `catalog_hits`, `statistics` and `candidates`.
`complete` means the bounded search reached its reported end; it does not mean
every candidate is usable. `truncated` and `failed` retain the inspected subset,
explicit reasons and a nonzero CLI exit status. Narrow the query or deliberately
adjust a limit. A candidate limit never silently becomes the four-source job
limit. Changing CMR metadata between pages remains possible: Search-After
pagination is not an atomic catalog snapshot.

Each candidate distinguishes:

| Field | Claim |
| --- | --- |
| `catalog_relevance` | Reported by CMR for the supplied bbox and interval |
| `data_access` | An exact, unsigned NASA archive GET DATA locator is supported, missing or ambiguous |
| `processing_family` | Catalog producer ID, CRID and filename appear to identify Raster D PGD0, or are unsupported/ambiguous |
| `scientific_profile` | `not_inspected` during discovery |
| `native_selection` | `unresolved` during discovery |
| `exclusions` | Concrete missing, invalid, unsupported or conflicting metadata |

Catalog coverage retains reported WGS84 rectangles and outer polygon boundaries.
It can be approximate, and does not establish native-grid coverage, useful WSE,
or river-specific measurements. The catalog family check accepts recognizable
100 m and 250 m product identities as candidates; it does not qualify their
scientific data. Only existing native inspection determines structural support.
No automatic science support or quality-policy changes follow from discovery.

Select one to four granules by stable CMR concept ID, with an explicit observation
ID, native `/y,/x` window, positive strides, quality policy and resource limits.
Use [the runnable two-control selection](../../examples/swot-discovery-selection.json)
as the complete schema. Its catalog paths are relative to the current working
directory. Up to four discovery reports can supply candidates from distinct
regions or intervals. An identical granule repeated across reports is accepted
as one choice; conflicting records and repeated job sources are refused.

```sh
./target/release/tile-ripper discover-job \
  examples/swot-discovery-selection.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --output my-swot-job.json > my-swot-job-review.json

./target/release/tile-ripper plan my-swot-job.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint ./my-private-checkpoint > my-swot-plan.json

./target/release/tile-ripper fetch my-swot-job.json \
  --auth earthdata --netrc "$HOME/.netrc" \
  --checkpoint ./my-private-checkpoint --output ./my-swot.zarr \
  > my-swot-receipt.json
```

`discover-job` uses the existing `Job::plan` and fresh exact-object authenticated
session factory. Configured policies also use the normal job `sources` policy
array, CA and explicit netrc flags. No credentials or signed routes enter the
catalog result, job, review or output. Source preparation reads bounded native
HDF5 metadata and coordinates; it does not stream the five science arrays.

Generation succeeds only after every explicit selection passes native profile
inspection and aggregate admission. It binds native descriptor, cycle, pass,
scene, CRID, product counter, PGE version (when reported), object size (when
reported) and rounded granule times to the selected catalog identity. It pins
the observed strong ETag in the generated job, retaining any caller-supplied
expected-ETag assertion. The review includes source dimensions, native CRS,
coordinate definitions, root coordinate ranges, explicit selections, policies,
source identities and admission. Only then is the new manifest file written.
The command refuses existing output files. A failure writes no admitted job.

Catalog times and native source granule/coverage times remain separately named.
They can differ at subsecond precision. The optional `catalog` observation field
is retained in normal job inspections, receipts and Zarr observation attributes.
It supplies reported provenance and additional identity assertions; conditional
source-version validation remains authoritative. Fetch reauthenticates and
revalidates even after successful generation. Checkpointing starts with normal
`plan`/`fetch`, using the final pinned manifest.

Each observation is an independent native grid. Open it directly:

```python
import xarray as xr

held = xr.open_zarr("my-swot.zarr", group="observations/held_out_20240217",
                   consolidated=False, mask_and_scale=False, decode_times=False)
original = xr.open_zarr("my-swot.zarr", group="observations/original_20240216",
                       consolidated=False, mask_and_scale=False, decode_times=False)
accepted_wse = held.wse.where(held.quality_mask.astype(bool))
```

The existing [SWOT job contracts](../swot-job/workflow.md) still govern raw stored
values, masks, scientific replay, source-version checkpoints, aggregate memory
and logical disk admission, and all-or-nothing scientific Zarr publication.

## Limits and API

One bbox must be ordered within `[-180,180] × [-90,90]`, at most 30 degrees per
axis and 400 square degrees. The positive interval is at most 31 days. Split
antimeridian queries explicitly into separate bounded searches. No AOI-to-native
pixel translator, reprojection, mosaicking or implicit alignment is supplied.

| Resource | Default | Hard ceiling |
| --- | --- | --- |
| Returned candidates (`--limit`, required) | Caller chooses | 100 |
| Granule page size (`--page-size`) | 20 | 100 |
| Granule pages (`--max-pages`) | 10 | 10 |
| Inspected records (`--max-records`) | 400 | 1,000 |
| Response body (`--response-kib`) | 1,024 KiB | 1,024 KiB |
| All response bodies including retries (`--total-kib`) | 8,192 KiB | 8,192 KiB |
| Query time (`--timeout-seconds`) | 120 s | 120 s |
| Per-request time (`--request-seconds`) | 15 s | 30 s |
| Retries per request (`--retries`) | 1 | 2 |
| Concurrent requests | 1 | 1 |
| Collection results | Exactly one required | 2 inspected |
| Parsed JSON per response | — | 100,000 nodes; 16 KiB per string; serde depth limit |
| Retained footprint per candidate | — | 16 rectangles, 16 outer boundaries, 256 total points |
| Discovery report / selection / job input | — | 4 MiB / 64 KiB / 64 KiB |

At most 33 HTTP attempts can occur (one collection request plus ten granule
pages, each with at most two retries). A single sentinel byte may be read to
detect a body cap; the report records it. Reads stop immediately when that
happens. Rate limiting ends the run without an immediate retry; other retryable
transport/5xx failures use the same total deadline and byte accounting. Server
error bodies, rejected raw URLs and pagination tokens are not emitted. Bounds
apply to retained/parsed metadata and input bodies, not process RSS, allocator
overhead, or TLS/HTTP runtime allocations.

The typed Rust API is `swot_discovery::{Query, Limits, CmrClient, Report,
Candidate}`. `CmrClient::discover` is usable without native-HDF5 features.
`bridge::{JobSelection, Selection, NativeWindow, draft}` requires `native-hdf5`;
`Draft::inspect_authenticated` additionally requires `authenticated-http` and
accepts the existing fresh exact-object session factory. A `Draft` explicitly
has no source inspection/readiness claim. `ReadyJob` contains the inspected,
version-pinned `swot_job::Manifest` and review.

Frozen tests use the explicit `--cmr-fixture` / `CmrClient::loopback_fixture`
entry point, restricted to numeric loopback HTTP origins. This does not broaden
accepted archive URLs or the Earthdata redirect policy.

```sh
cargo test --locked --no-default-features \
  --features native-hdf5,authenticated-http --test swot_discovery
python bench/swot_discovery.py offline
# Explicit bounded provider operations, separate from frozen tests:
python bench/swot_discovery.py live --netrc "$HOME/.netrc"
python bench/swot_discovery.py additional --netrc "$HOME/.netrc"
```

See [the decision report](report.md), [predeclared protocol](protocol.json),
[official CMR API](https://cmr.earthdata.nasa.gov/search/site/docs/search/api.html)
and [NASA Raster D product metadata](https://podaac.jpl.nasa.gov/dataset/SWOT_L2_HR_Raster_D).
