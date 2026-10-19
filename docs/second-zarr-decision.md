# Validation efficiency and second scientific Zarr decision gate

Tile-Ripper now retrieves **NASA POWER IMERG daily precipitation and liquid
precipitation probability** from the provider's public v2 store through the
existing `plan`/`fetch` Rust library and CLI. Safe validation is cheaper; the shared
planner remains intact. No new dependency, downloader, infrastructure, third
format, deployment, merge or publication was introduced.

## Measured validation efficiency

At `407e642`, raw metadata bodies were already cached per prepared instance.
Repeated HEADs, not repeated metadata GETs, dominated the tiny WeatherBench2
workload. Root/array `zarr.json` absence probes were revalidated five times even
though execution explicitly constructs v2 arrays from pinned v2 metadata. Those
admission results cannot change the fixed v2 decode. Initial hybrids/v3 still fail;
actual metadata, coordinate and data versions remain protected. No TTL, content
hash shortcut, provider immutability guess or shared speculative cache was added.

Per-key/phase attempts, bytes, statuses, retries and times now distinguish planning,
preparation validation, execution pre/post validation, execution and output
finalization. WeatherBench2 prepare plus two fetches:

| Phase | Actual HTTP requests |
|---|---:|
| Planning (includes admission probes and selected data HEAD) | 13 |
| Preparation dependency validation | 7 |
| Pre-execution validation, two fetches | 14 |
| Scientific execution, two fetches | 2 |
| Post-execution validation, two fetches | 14 |
| Output finalization | 0 |
| Total | **50**, previously **75** |

Exactly 25 redundant HEADs disappear; bytes remain **1,231,715**, including one
metadata/coordinate discovery and two 580,139-byte scientific chunk payloads.
The equivalent xarray baseline makes 46 requests after applying the same
admission-only rule to its root-array absence check. Remaining coordinate/metadata
HEADs are meaningful for a mutable prefix; a consolidated metadata ETag does not
establish the identity of independent coordinate/data objects.

Three repeated public runs per reader in sequential before/after batches:

| Implementation | Cold prepare + fetch | Warm prepared fetch | Process, two fetches |
|---|---:|---:|---:|
| Tile-Ripper 407e642 | 1.949 s | 0.857 s | 2.967 s |
| Current Tile-Ripper | 1.005 s | 0.304 s | 1.486 s |
| Current-batch xarray | 1.106 s | 0.417 s | 1.827 s |

These are point measurements, not a statistically established general speedup.
Public TLS/provider/network/OS caches were not controlled; the baseline also varied
between batches. A separate **interleaved 10 ms/request loopback control** isolates
the mechanism: 60→35 requests, identical 3,627 body bytes, median process time
1.059→0.735 s, cold 0.594→0.397 s and warm 0.329→0.203 s. Removed injected delay
alone is 25×10 ms = 0.250 s. The measured improvement is supported by exact request
counts and this control, not just a few milliseconds on the public endpoint.

[Before](validation-efficiency-before.json), [after](validation-efficiency-weatherbench.json),
[fixed-latency control](validation-efficiency-controlled.json).

## Second source and scientific correctness

[Provider documentation](https://power.larc.nasa.gov/docs/services/aws/) identifies
public NASA POWER analysis-ready data; [inspected metadata](second-zarr-inspection.json)
was examined before support was written. The daily IMERG spatial store is genuinely
Zarr v2, anonymous HTTPS, separate `(time,lat,lon)` Float32 variables, NaN fill,
Float64 centers and Int64 days since 1998-01-01 with proleptic Gregorian calendar.
It uses Blosc/Zstd bitshuffle and a Float32-to-Float32 fixed-scale-offset filter.
The already pinned zarrs library implements both; Cargo dependencies are unchanged.

This contrasts with WeatherBench2's long axis names, different axis order/time
units, plain Blosc/LZ4 and absence of a storage filter. IMERG longitude centers
contain producer Float32 rounding despite Float64 storage. Declared nominal
spacing permits bounded rounding residuals; actual coordinate midpoints select
cells and actual vectors are carried through callbacks. No irregular/curvilinear
engine was introduced. Correctly labelled units and case-normalized semantic
standard names are admitted. No separate band dimension exists here, so none was
invented; requested variables remain distinct named arrays.

The [complete request](../examples/imerg.json) retrieves both variables for
2020-06-01 and bounds `[-80.02,35.02,-79.68,35.28]`: one time × three latitudes ×
five longitudes × two variables = **30 native logical Float32 values**. Independent
xarray indexing and Numcodecs decode match **exactly**, including NaNs, raw/decoded
time coordinates, dimensions, variable identities, units and attributes. The
mathematical minimum, planned keys, actual scientific GETs and successful decodes
all agree on **two** source chunks, each requested/decoded once per extraction:
`IMERG_PRECTOT/8187.1.0` and `IMERG_PRECLIQUID_PROB/8187.1.0`.

Each extraction transfers **1,347,071 scientific bytes** and materializes
12,960,000 logical chunk bytes to deliver 120 value bytes. This amplification is
intrinsic to 900×1800 source spatial chunks, not extra chunk reads. Three repeated
cold/warm runs per reader validate 12 equivalent extractions (360 values).
Fourteen independent controlled cases additionally cover reversed axes, missing
chunks/NaNs, empty selections, irregular centers, invalid units/filter widths and
transient HTTP failure. The Float32 storage filter is decoded once; output stores
those exact logical values without lossy re-quantization. Subset extent/time
coverage fields are recalculated; original source attributes/array encoding are
retained in `manifest.source_metadata`.

## IMERG benchmark

Equivalent output values, coordinates, units, chunks, destination codecs and
per-object durable synchronization; the xarray baseline also omits repeat lossy
filtering. Reference and manifest-template acquisitions are outside timing.

| Reader | Cold | Warm | Process, two fetches | Peak RSS |
|---|---:|---:|---:|---:|
| Tile-Ripper | 3.588 s | 1.169 s | 4.894 s | 64.6 MiB |
| xarray/Zarr | 2.935 s | 1.203 s | 4.516 s | 106.9 MiB |

Rust makes **36 HTTP requests** and xarray **31**, with both transferring
**2,704,205 body bytes** per preparation
and two fetches: 10,063 discovery bytes plus two identical scientific payloads.
The five extra Rust requests are initial v3 admission probes; both retain the
same actual metadata/coordinate/data version checks. Both write 18 files; actual diagnostic/serialization sizes differ and are measured.
The current-batch xarray process median is about 0.38 s (8%) lower. Cold also
favors the baseline; warm differs by only about 34 ms. No Rust speed advantage
is claimed. These small, network-variable samples do not establish a general
performance ranking. Rust uses bounded native buffers;
xarray materializes selected output values. Language/runtime memory contributes
to RSS, so these peaks are not a general scaling claim.

Representative cold Rust stage medians: source retrieval 0.708 s,
fused read/decode 0.736 s, output storage writes 0.127 s,
output setup 0.095 s, finalization 0.005 s and first usable native output
1.225 s from fetch start. Individual read/decode minus retrieval
includes codec/filter and library assembly; it is not isolated codec CPU.
Source/version network round trips and chunk transfer dominate; output persistence
is smaller. Nested timers are not additive.
[Raw measurements](second-zarr-benchmark.json).

## Safety and regression gate

**32 ordinary Rust tests** pass, with all-target Clippy warnings denied and format
checks. Existing WeatherBench2's 24 orientation/codec cases and 17 failure/edge
cases, the independent local Zarr oracle, and COG scientific/geometry/failure/
snapshot regressions pass. **28 HTTP checks** retain bounded gzip/bomb/Blosc
rejection, version changes, missing-key appearance, cancellation and cache
isolation. New checks prove coordinate mutation and metadata mutation during
execution still fail, while adding an unrelated v3 admission marker after
preparation leaves the pinned v2 scientific result unchanged. Initial hybrid
admission remains rejected. [Regression evidence](second-zarr-regression.json).

Encoded/decoded sizes and filter element width are validated before large
allocation. Gzip fixed output sizing, checksum/EOF checks and Blosc header limits
remain. The new filter cannot change element width; nonfinite/zero/overflowing
Float32 parameters are rejected. Source filter CPU/memory is bounded by admitted
native chunk size. No new codec algorithm or downloader exists. Blosc still has
no checksum; per-object versions are not an atomic dataset snapshot. The buffer
budget is not a process RSS sandbox. Completion/cleanup behavior is retained.

## Project decision

1. **Meaningfully format-independent?** Yes within native rectilinear extraction:
   COG and two independently produced scientific Zarr layouts share requests,
   plans, windows, chunk provenance, typed buffers, callbacks and completion. This
   is bounded evidence, not universal raster/datacube support.
2. **Which conventions?** Custom UInt16 grids plus marker-free regular geographic
   Float32 axes, both axis directions/orders, latitude/longitude and lat/lon,
   compatible coordinate metadata, three verified time encodings, none/gzip and
   admitted Blosc/filter combinations. Precise datum is still a disclosed profile
   interpretation. Arbitrary calendars, projected/curvilinear grids, packed band
   dimensions, other scientific dtypes, v3 and authentication remain unsupported.
3. **Planner without provider cases?** Yes. No provider URLs or dataset-specific
   planning branches exist. Dimension roles/codec admission belong in the Zarr
   adapter; shared Window/ChunkObject logic remains. Actual center vectors are a
   general library extension needed by rounded source grids, not an IMERG planner.
4. **Safe reuse improved performance?** Existing cached bodies remain reused.
   Removing non-dependency revalidation saves 25 HEADs without losing v2 version
   checks; the fixed-latency control demonstrates the benefit. Parsed coordinates
   are still decoded again in a few stages; this small cost did not justify a new
   caching architecture.
5. **Remaining bottlenecks?** Meaningful sequential version round trips, source
   chunk amplification, public network variability and some filter CPU. Metadata
   hashes alone cannot establish the version of independent scientific objects.
6. **Concrete workflow?** A scientist can plan and retrieve native temperature or
   paired precipitation/probability subsets by bounds/date/variables with a Rust
   library or copyable CLI command, preserving coordinates, units, missingness and
   provenance, without custom Python acquisition code.
7. **Next milestone?** Improve the library/CLI for outside users: clearer request
   discovery/templates, capability/error reporting and reproducible onboarding.
   Do that before Zarr v3 or SWOT/NISAR HDF5; add either only for a demonstrated
   user dataset need. No further format implementation starts at this gate.

[Runnable commands, exact profile and reproduction](second-zarr-workflow.md).
No paid resources, Cloudflare deployment, ChronoZarr modification, merge or
publication occurred. The tested work is committed locally and this milestone stops.
