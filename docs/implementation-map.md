# Ownership and the next adapter boundary

The single Cargo package avoids a premature multi-crate workspace. Its public
modules are the boundaries that could become crates after independent users
need them:

| Module | Owns | Does not own |
|---|---|---|
| model | request, capabilities, DAG nodes, estimates | transport/authentication |
| planner | validation, time selection, native extraction DAG, node budget | codecs, HTTP |
| cog | GDAL layout inspection, local metadata cache, native window/tile math | a TIFF decoder |
| zarr | v2 array/chunk-key planning with explicit coordinate metadata | decoding/materialization |
| execute | serial read dispatch, one-buffer callback flow, GeoTIFF sink | discovery/composites |
| main | JSON/file arguments and CLI GDAL settings | selection algorithms |

`FormatPlanner::capabilities` and `plan_asset` are implemented by both adapters.
`Read` is an enum, not a bag of optional offset/key fields. Each node has an ID,
dependencies, asset ID and operation. `Operation::Decode` documents the
responsible reader; GDAL retrieval/decode are fused, not independently
schedulable. No external plan can be submitted to `execute`: it replans, and
its internal executor only runs planner-produced native windows.

The current dispatcher enumerates two built-in adapters. A registry and an
executor adapter trait should be introduced with the first third-party
format, not represented as a plugin promise today. New physical read variants
in the schema are design space, not executed code paths. A Zarr plan correctly
fails materialization before any sink callback. Unsupported request semantics
are rejected rather than rewritten.

For genuinely incremental execution, split `plan` into a discovery page stream
and per-asset fragments: append stable IDs scoped by job/asset, resolve layouts
under byte permits, then execute ready fragments while later pages arrive.
An optional global pass can refine coalescing/deduplication, with its extra
latency visible. Do not require it before the first useful output. Persistent
plans need identity checks and a schema/adapter version; current JSON is only
diagnostic.

Next physical adapters should accept a runtime credential resolver separately
from opaque object IDs in the plan. They must expose selection and codec
support per asset, immutable object identity, and observed I/O counters. A
conditional whole-object fallback must require explicit disk/staging budget;
it must not silently bypass random-access limits. Layout discovery and data
retrieval may both perform I/O and need separate accounting.


## Executable Zarr milestone (plan schema 2)

`zarr_acquire` now validates the defined local time/y/x dataset layout, resolves
coordinate/date selections, executes zarrs dataset slices and writes native
subset Zarr groups. `zarr` retains the original root-array planning probe and
routes groups to the executable adapter. `execute` dispatches both read kinds
through the same fallible native-plane callback; `spatial` owns shared window
math. Layout/callback variable selectors replace mandatory band numbers.
See `zarr-workflow.md` and `zarr-decision-gate.md` for current capability limits;
the planning-only statements above describe the original milestone.


## Unified lifecycle milestone

`acquisition` resolves source discovery/direct bindings once, preserves the
native plan and version checks, renders inspection JSON/human summaries and
publishes a shared completion/provenance envelope. CLI aliases use it too.
Catalog configuration stays outside the logical planner; formats still own
native grids, coordinates, codecs and execution. Zarr now plans source chunks
rather than output-sized slices and retrieves each selected key once. See
ADR 0003 and the unified decision gate for current evidence and limits.

## Bounded native Zarr output writing

The native output adapter now assembles partial callback regions in a shared,
byte-bounded destination-chunk LRU. Eviction and finalization use zarrs' existing
full-chunk encoding/storage path. The planner and one-read-per-source-key
execution order are unchanged; COG does not use the buffer. `zarr_io` adds thin
store timing delegation and the existing zarrs metrics adapter counts output I/O.
`Chunk` exposes allocation-free native pixel-center/validity helpers; the existing
synchronous prepared callback remains the bounded consumption interface.

See `docs/output-write-decision.md` and `docs/output-write-workflow.md` for the
measured gate, equivalent-output protocol, limitations and stream assessment.


## Remote HTTP Zarr

`src/storage.rs` provides private prepared local/HTTP object bindings, bounded
metadata cache, conditional versions and transport accounting using reqwest and
zarrs storage traits. `zarr_acquire.rs` interprets consolidated metadata and
reuses existing native selection/extraction. `acquisition.rs` owns preparation,
sealing, pre/post validation, cleanup and completion. Public Request/Plan remain
format-neutral; COG continues through GDAL. See `remote-zarr-workflow.md` for
strict layout/backend limits and `remote-zarr-decision.md` for measurements.


## Conventional geographic Float32 profile

`zarr_acquire.rs` recognizes exact conventional latitude/longitude roles, validates
regular axes and verified time units, maps native source dimension order into the
existing chunk-key planner, and normalizes output dimensions. `spatial::window`
handles both affine directions. `pixels.rs` carries UInt16/Float32 native samples
through the existing shared synchronous callback. `zarr_decode.rs` admits fixed
decoded sizes, uses flate2 into a fixed output buffer, and validates Blosc headers
before established zarrs decoding. Original source/output metadata is retained;
only the internal gzip input array view is uncompressed. No second planner,
downloader or format was introduced. See `cf-zarr-workflow.md`.


## Validation efficiency and satellite precipitation profile

`storage.rs` separates cached admission probes from actual snapshot dependencies
and groups HTTP observations by lifecycle phase; `acquisition.rs` marks validation
and execution phases. Actual v2 metadata, coordinates, conditional scientific
GETs and missing-key checks retain their existing versions. `zarr_acquire.rs`
recognizes lat/lon aliases, the verified daily time encoding, declared spacing
with bounded coordinate rounding, and the chosen existing codec/filter profile.
`spatial::coordinate_window` selects cells using exact center midpoints. Zarr
callbacks carry actual coordinate vectors in `Chunk::coordinates`. The format
adapter recalculates subset extent metadata and stores original metadata in the
manifest; output Float32 has no repeat lossy filter. The logical planner and
public request schema remain unchanged. No provider URL appears in source logic.
