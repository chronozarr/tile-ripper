# External usability and first public-release readiness gate

Base: `fa1ee6c`, branch `codex/acquisition-planner`. Local candidate only.
No format, dependency, decoder, downloader, cloud service or infrastructure added.
No merge, registry publication, binary release or deployment.

## Gate answers

1. **Can an outside scientist reasonably use it now?** A scientist with Rust and
   system GDAL can follow the corrected README to acquire useful native subsets
   without Python. Direct flags remove request-file authoring for common queries.
   This is a supported-profile pre-alpha claim, not independently established
   newcomer usability: the required Sonnet trial could not authenticate, and no
   human trial was available.
2. **Clearest advantage?** One inspectable `plan` → `fetch` lifecycle across
   catalog COG and scientific Zarr, with native scientific outputs, explicit
   processing decisions, minimum Zarr chunk selection and completion/provenance.
   There is no demonstrated consistent speed advantage. Existing Python users
   may be better served by xarray; broader raster work belongs in GDAL/rasterio.
3. **What blocks a public alpha?** Complete the independent docs-only and human
   trials; test clean-machine supported OS/runtime combinations; settle project
   licensing/package metadata (there is no root project licence declaration);
   review dependency/security and distribution requirements. API is still a
   prototype, and supported profiles need to remain explicit. No licence was
   invented on the owner's behalf. No release was authorized or produced.
4. **Portable Rust binary?** Not today as a standalone executable. The clean
   macOS arm64 build succeeds, but it dynamically links Homebrew GDAL. Recipients
   need the matching runtime and its resources. Local Cargo source installation
   is the smallest current distribution route. Linux, Intel macOS and Windows
   packaging were not validated. [Installation evidence](external-usability-build.json)
   and [distribution assessment](installation.md) separate tested facts from proposals.
5. **What may be advertised with qualifications?** Earth Search Sentinel-2 C1
   L2A UInt16 native GeoTIFF extracts; WeatherBench2 ERA5 64×32 Float32 geographic
   Zarr subsets; NASA POWER IMERG daily Float32 geographic Zarr subsets; existing
   narrow local UInt16 paths. Native grids/values, times, NaNs/nodata and source
   metadata are preserved on tested profiles. This is not arbitrary STAC/CF/Zarr,
   and not a mosaic/composite/reprojection service. Cloud masking is not provided.
6. **Next milestone?** A bounded source-alpha hardening pass: restore Sonnet
   authentication, run actual independent docs-only trials and one scientist
   trial, test installations on declared platforms, and resolve licensing and
   package metadata. Do this before binary release engineering or a third format.

## Audit and implemented changes

The old README incorrectly said Zarr was planning-only, showed an obsolete
`pixels.data()` API, started with Python fixture creation and mixed historical
support statements with current behavior. Installation omitted platform/runtime
limitations; common selections required hand-written JSON. Errors such as
“unknown option” lacked a recovery route, and human completion omitted the output
path and available measured traffic.

The new README follows the requested newcomer order, starts with a real public
query, explains opening outputs, names supported profiles and links detailed
historical evidence separately. CLI help documents one parameter vocabulary for
both formats: source/format, collection where applicable, rectangle or STAC
GeoJSON Polygon, inclusive dates, variables, output, buffer budget and dry run.
The parser creates the **existing public Request** and calls the same acquisition
implementation. JSON requests and legacy aliases remain accepted. Ambiguous
mixed JSON/selection flags, malformed bounds/dates, empty variables and duplicate
options fail before network work. Existing destination directories now fail
before metadata inspection. Raw argument values are not echoed in parser errors.

JSON remains the default for scripts; `--human` adds two concise stderr progress
messages and prints an absolute destination, output record count and measured
Zarr HTTP request/body-byte totals. Unknown GDAL network totals are identified as
unavailable. Logical selection, native CRS/windows, timestamps, physical reads,
estimated compressed data and estimated logical output values are inspectable.
Encoded output size is explicitly unknown. Estimates exclude headers/coordinates/
metadata/encoding where stated. Source-validation limits are explained in both
human output and documentation. No public Request/Plan schema or planner logic changed;
inspection JSON gains additive fields only.

## New public requests and independent scientific oracles

These were primary-agent scripted trials, **not** independent usability trials.
None reused the original example region/variable combination unchanged.

| Request | Result | Measured acquisition | Measured HTTP bodies / requests |
| --- | --- | --- | --- |
| ERA5 2m_temperature, 1960-02-02 12–18 UTC, 100–110°E / 10°S–0° | 2 times × 2 lat × 3 lon, 12 Float32 values; one minimum source chunk | Rust CLI 1.170 s; xarray prepared-first-fetch 0.624 s | both 650,906 bytes; Rust 35 / xarray 31 requests |
| IMERG_PRECTOT, 2021-01-03, 8.02–8.18°E / 45.02–45.18°N | 1 time × 2 lat × 2 lon, 4 Float32 values; one minimum source chunk | Rust CLI 3.266 s; xarray prepared-first-fetch 2.516 s | both 956,341 bytes; Rust 23 / xarray 19 requests |
| Sentinel-2 B03, 2025-07-28, −79.040–−79.038° / 35.920–35.922° | native 23×19 UInt16; 401 valid centers; EPSG:32617 | timed CLI repeat 2.161 s | real GDAL HTTP totals unavailable |

Zarr's independent xarray `assert_identical` covers native values, NaNs,
coordinates, variable units/attributes and corrected subset group attributes;
normal CF-decoded opening verifies time usability. Planned/read chunk keys match
an independently enumerated minimum, once per key. CLI peak RSS was 52,248,576
bytes (ERA5) and 65,585,152 bytes (IMERG); payload budgets are not RSS ceilings.
The baseline runs in-process, so this new trial does not measure its peak RSS or
interpreter startup. Earlier repeated process-isolated comparisons remain the
performance evidence. Neither these single public observations nor timing a
weaker ordinary workflow supports a speedup claim.

The COG oracle independently reads the original URL through rasterio/GDAL and
constructs/densifies the geographic AOI, masks by pixel center, writes a reference
GeoTIFF and checks every value, CRS, transform, nodata, scale and offset. Its
1.094 s reference operation excludes STAC discovery, source-version checking and
provenance generation; it is not an equivalent end-to-end baseline. Source
objects are read by ranges, not intentionally downloaded whole. Controlled GDAL
comparison retains measured traffic and equivalent selection semantics.

[Raw Zarr trials](external-usability-public.json),
[raw COG trial](external-usability-stac.json),
[ordinary alternatives and comparison limits](alternatives.md).
Reproduce in the existing frozen Python environments with `bench/usability.py`
and `bench/usability_stac.py`; use fresh output directories (scripts intentionally
refuse overwriting previous results). No Python environment is needed by the CLI.

## Usability observations and missing independent gate

The scripted Zarr workflows required one plan/dry-run, one fetch and one xarray
open/check each. Measured plan-plus-first-fetch times were 2.993 s (ERA5) and
5.156 s (IMERG), excluding build, source discovery and authoring the request.
Initial public COG success was not stopwatch-instrumented; 2.161 s is the subsequent
timed run, not a measured first-success time. These are machine timings, not
scientist task-completion times. The typed Rust example and human CLI output were
also executed against ERA5 and compared to the same independent reference.

The attempted separate Sonnet docs-only agent returned `401 OAuth access token
has expired` before reading docs or attempting any acquisition. Thus manual-step
counts, first-success time and explainability for an independent newcomer remain
**unmeasured**, not zero or passed. No substitute agent or human trial is claimed.
No requested format expansion or authentication change was made to bypass this.

## API and regression assessment

`acquisition::prepare` / `Prepared::{explain,human,execute,fetch}` remain the shared
library entry points; CLI translation is a binary-local module. The new typed
`examples/scientific_subset.rs` constructs and fetches a real query without CLI
internals. Runtime source bindings and codec execution remain private. Owned
UInt16/Float32 buffers, coordinates, masks and provenance support synchronous
backpressure; consumers retaining buffers are responsible for their memory.
No ChronoZarr coupling was introduced. `cog::LayoutCache` has an overly specific
name and legacy root-level execute/fetch APIs remain public rough edges; an API
stability pass should clarify/deprecate those only with deliberate compatibility
review, not refactor the planner for this usability milestone.

37 ordinary Rust tests, formatting, all-target Clippy, a clean-source binary build,
controlled STAC/GDAL validation, 28 HTTP transport/safety cases and 14 paired-profile
cases pass. The HTTP tests retain malformed gzip/bomb, transient failures, changed
metadata/coordinates/data, source isolation, cancellation and cleanup checks.
[HTTP evidence](external-usability-http-checks.json),
[profile evidence](external-usability-profile-checks.json),
[COG evidence](external-usability-cog-checks.json).
The source selection/writers/codecs were not optimized or expanded. Existing
repeated performance evidence remains historical; no speculative tuning or broad
new benchmark study was performed.
