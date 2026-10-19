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
