# Geographic SWOT acquisition decision gate

Implemented and independently verified on `codex/acquisition-planner`, continuing
from `cb69adb`. A scientist can discover Raster D observations, supply WGS84
bounds or a Polygon with holes, inspect the resolved native selection, and fetch
a complete version-pinned scientific Zarr v2 dataset without calculating indices.
The implementation is local and unreleased.

The [runnable workflow and exact contract](workflow.md),
[geographic selection example](../../examples/swot-aoi-selection.json),
[generated three-source manifest](generated-job.json),
[source inspections](live-review.json), and [complete live acceptance](live-workflow.json)
provide the inputs and evidence. The final executable also passed a complete
[checkpoint reconstruction and independent recheck](final-binary-reconstruction.json).

## What changed

`swot_aoi` adapts normal native preparation to inspected source shape, actual
stored x/y centers and an explicitly checked source CRS. It transforms AOI edges
forward for coverage diagnostics and inverse-transforms every source-global
center for membership in the original geographic polygon. The smallest
containing index window is therefore independent of output chunks, tile edges,
axis direction and corner-only projection approximations.

Discovery selections accept exactly one native `window` or geographic `aoi` per
observation. A normal GeoJSON Polygon can be wrapped with an explicit WGS84 CRS.
The generated manifest contains explicit unit-stride native indices, the original
normalized geometry, fixed selection policy, grid/coordinate fingerprints,
included-center count and mask hash. Plan/fetch recheck the selected coordinates,
CRS, count and mask against the pinned source. A zero-center selection yields an
explicit non-ready review and no executable manifest; candidates are not replaced.
The typed local resolver also returns no observation for that case.

The existing scientific writer adds a separate UInt8 `aoi_mask` and preserves all
five raw variables, five source-validity masks, the quality mask, exact projected
coordinates, CRS, time and original attributes. It never enforces the AOI by
rewriting WSE, uncertainty or integer flags. Requested accepted-AOI statistics use
`aoi_mask & source_valid_wse & quality_mask`.

The geographic provenance participates in both job and source checkpoint
identities. Exact-object authentication, strong version checks, fresh scientific
execution, serial source ownership, final validation sweeps and atomic all-or-nothing
publication retain the existing implementation. The core HDF5 reader/decoder,
authentication policy and checkpoint storage implementation were not changed.

## Existing machinery and new qualification

| Component | Reused authority | New gate |
| --- | --- | --- |
| CMR discovery and catalog bridge | Existing bounded anonymous CMR queries and native/catalog identity checks | Fresh discovery of all three selected concepts; geographic/native outcomes reported separately |
| Coordinate extraction | Existing bounded native HDF5 preparation and exact stored Float64 coordinate delivery | Complete x/y arrays agree between isolated h5py and NetCDF-C processes; their complete-axis hashes match the native resolver |
| Geometry/transform libraries | Existing `geo` and `proj4rs` versions used by native Sentinel-2 | New source-global SWOT center oracle, CRS admission, reversed axes, geographic boundary tolerance and real-grid partial/empty cases |
| Sentinel rasterization | Its affine grids, GDAL burn-edge conventions and prior independent tests remain distinct | SWOT does not reuse its affine/window rounding or boundary policy |
| Scientific output and recovery | Existing typed Zarr writer, source masks, policies, transactions and source-range checkpoints | AOI-mask separation, polygon identity changes, changed chunk/tile edges, later-source AOI failure, complete real reconstruction |

The new mathematical contract supports canonical WGS84 UTM north/south CRS
definitions. Real scientific qualification is limited to the three pinned
100 m PGD0 selections below. The southern-zone transform is a synthetic geometry
qualification, not a claim about an additional real SWOT product.

## Real observations and distinct outcomes

All three actual granule footprints intersect their requested AOIs, include
native centers, pass the existing native scientific profile, and pass scientific
acquisition plus both independent readers. Their native grids remain separate.
Row/column intervals below are source-global and end-exclusive.

| Observation / CMR concept | Actual native CRS / source shape | Rows × columns | AOI centers | Accepted AOI WSE |
| --- | --- | --- | ---: | ---: |
| `held_out_20240217` / `G3927559976-POCLOUD` | EPSG:32601 / 1779 × 1780 | `[0,128)` × `[656,784)` | 13,671 | 1,094 |
| `original_20240216` / `G3927359490-POCLOUD` | EPSG:32619 / 1580 × 1580 | `[600,728)` × `[400,528)` | 13,671 | 2,533 |
| `additional_20240216` / `G3927661761-POCLOUD` | EPSG:32619 / 1578 × 1577 | `[785,797)` × `[793,801)` | 91 | 33 |

The control polygons have a concave notch and a hole. They were constructed
inside the previously qualified capture envelopes so repeated tests need no new
control science downloads. Scientist-facing selections contain only geographic
positions. The third AOI, `[-71.85,41.53,-71.84,41.54]`, was fixed geographically
before its native window was known. It was selected from the prior additional
candidate list by a fixed first-candidate rule, with no substitution.

Two additional outcomes demonstrate why the stages cannot be collapsed:

- A held-out AOI intersects the actual CMR footprint and includes **13,671 native
  centers**, passes the profile and independent science checks, but contains
  **zero accepted WSE cells**. Its completed dataset is valid.
- A tiny live AOI intersects that same footprint but lies between stored centers.
  The resolver finds **zero native centers**, emits `not_ready_zero_native_centers`,
  returns nonzero and writes no job. Final five-variable selection admission and
  scientific acquisition are explicitly not attempted; usable WSE remains unknown.

See the [stage-separated outcomes](profile.json),
[frozen zero-quality result and fault gates](frozen-workflow.json), and
[live zero-center review](live-zero-centers.json).

## Independent geographic and scientific evidence

[108 geometry oracles](geometry-oracles.json) compare **181,548 source-global
centers** across UTM 19N, 1N and a southern synthetic UTM 55S grid. They cover
closed outer/hole boundaries, both sides of the explicit roundoff band, concave
and irregular edges, reversed winding, all four axis-direction combinations,
partial overlap, empty intersections and curved geographic edges. Forward
coordinates agree with independent PROJ and GDAL transformations within
`1.87e-9` m in this matrix. Native masks and bounds match PROJ/GEOS exactly.

The reference inverse-transforms the complete source grid with PROJ, then uses
GEOS coverage/boundary distance. It never consumes the native resolver's window
or a window-local affine. On both real control grids, separate partially
overlapping AOIs resolve to `[0:3,0:3]`; entirely outside AOIs resolve to no window.
Those checks use independently read complete source coordinate arrays.

[All observed geometry differences are retained](discrepancies.json):

- 44 GDAL-default rasterizer comparisons differ at ties. Every differing center
  lies within the declared `1e-10` degree band of the true geographic boundary.
  GDAL directional burn edges and the forward polygon approximation differ from
  this closed geographic coverage contract; the independent GEOS oracle agrees
  exactly with native inclusion.
- 12 corner-only projection comparisons differ from the true geographic-edge
  result. This demonstrates why transformed corners cannot safely select pixels.
- There are **zero unexplained native-versus-independent center discrepancies**.

Final scientific output was opened with Zarr-Python and xarray and compared
bit-for-bit against isolated h5py 3.12.1/HDF5 1.12.2 and NetCDF-C 4.9.3/HDF5 1.14.6
processes. The comparisons include all five raw variables, five source masks,
quality masks, complete-axis fingerprints, selected x/y, time, interpreted and
stored attribute bytes, CRS, AOI masks and counts. Capture guards enforce every
independent file read, with zero misses during successful qualification.

Changing native tile edge 32→19 and destination chunk edge 64→17 preserves the
global AOI mask and all science. A changed polygon rejects the previous scientific
checkpoint even when the discrete mask happens to remain unchanged. A polygon
change that alters centers also rejects stale mask provenance in a fresh job.
A later observation's wrong AOI hash aborts the complete dataset after the first
observation was written provisionally; no destination or ordinary staging remains.

## Bounded third-source qualification

The third source retains the same PGD0 product profile. Its fixed 12 × 8 window
crosses a native 789-row chunk boundary and needs **4,148,750 encoded science
bytes from nine distinct physical chunks**, despite containing only 91 AOI centers.
The summary layer is decoded twice during alignment, so the decoder reports ten
source-chunk calls; this is distinct from nine encoded physical reads.

The first 4 MiB mirror cap stopped acquisition without publishing output. Its
[failure and measured requirements](additional-initial-cap-failure.json) are
preserved. A [documented 6 MiB amendment](additional-capture-budget-v2.json) resumed
the same version and geography. The native capture totals **4,415,080 bytes in
27 successful range GETs**.

NetCDF-C's eager metadata open then exhausted the original 128-request reference
cap after 59,697 additional bytes. That [failure](additional-reference-initial-cap-failure.json)
also remains preserved. A [separate bounded amendment](additional-capture-budget-v3.json)
allowed up to 192 requests under the unchanged 512 KiB byte cap. It completed at
**129 requests / 60,721 added bytes**, all guard-observed metadata misses.
Reference-added bytes were kept separate from the sealed native capture.
The [third-source scientific qualification](additional-qualification.json) then passed;
no product-profile requirement was relaxed.

## Measured cost

These are bounded observations with provider latency included, not general speed
claims. [The profiling receipt](profile.json) retains per-source encoded and
decoded chunk requirements, working estimates, CPU/RSS, source operations,
wire requests, bytes, mask/coordinate timings and checkpoint usage.

| Operation | Wall time | Source response-body bytes | Wire HTTP requests |
| --- | ---: | ---: | ---: |
| Three CMR searches | 2.57 s reported service time | 180,410 catalog bytes | 12 CMR requests |
| Geographic resolution + three-source job generation | 89.33 s | 2,408,825 | 244 |
| Ordinary selected three-source plan | 32.68 s | 1,021,395 | 92 |
| First checkpointed acquisition, validation and durable output | 62.10 s | 7,426,579 | 159 |
| Complete scientific reconstruction from checkpoint | 32.46 s | 48 | 78 |

Geometry transformation and source-global membership/mask resolution itself
totals **1.392 s**: 0.578, 0.468 and 0.346 s per granule. The two shape/full-axis
metadata passes total 55.66 s; selected five-variable admission is another 32.23 s.
Compared with the ordinary three-source plan, geographic generation adds about
56.64 s, mostly repeated authenticated metadata inspection. This is meaningful
overhead; it does not come from an expensive polygon rasterizer or science-payload
download during discovery. Anonymous candidate discovery reads no archive object.

The first aligned native tile arrives 1.545 s after scientific execution starts
for the first source, excluding its 10.995 s preparation. The first complete
observation arrives 15.555 s into fetch. Acquisition/writing takes 48.325 s and
the final source sweep 13.735 s; CLI wall time includes synchronization and
no-replace publication. A final rebuilt-executable reconstruction also passed
in 45.68 s with 48 source bytes; its slower latency is separately recorded.

Generation uses 1.753 CPU s and samples 29,638,656 bytes peak RSS. The first fetch
uses 0.656 CPU s, samples 44,875,776 bytes peak RSS and observes a 72,800,657-byte
working-reservation peak. The admitted aggregate cap is 101,257,216 bytes against
134,217,728 allowed. Geometry is charged within the existing job bookkeeping
reservation. Reservations are not an RSS ceiling.

The checkpoint contains 84 retained ranges, 7,462,000 logical file bytes and
7,757,824 allocated filesystem bytes, under its aggregate 32 MiB quota. The first
output contains 146 files and 2,524,390 logical bytes, including provenance.
The main live workflow uses 10,856,847 source-body bytes; including the separate
third-source/reference captures, zero-center test and final-binary replay gives
**15,762,946 bytes**, below 16 MiB. These source counts exclude catalog bodies,
redirect bodies and wire framing. Logical data-request counters can include
checkpoint hits; the wire counters above include archive/login/resource redirects.

## Regressions and remaining limits

[Verification](verification.json) records the exact commands, hashes and platform
results: 145 all-feature Rust tests passed with eight explicit existing/helper
skips; the native-HDF5/authentication subset passed 90 tests with five such skips.
The 38 authentication scenarios, 32 checkpoint scenarios and 19 multi-observation
fault/restart cases passed again. All 16 combinations of GDAL, native COG, native
HDF5 and authenticated HTTP compile across all targets; experimental output is
covered by the all-feature run. Active COG and Zarr v2/v3 regressions pass in that
same suite. A clean Linux ARM64 build passes 27 focused tests and links no GDAL,
HDF5, NetCDF or PROJ shared library. Strict Clippy, Rust formatting and Python
lint/format/compilation checks pass.

The supported geographic/product limits are explicit in the [contract](workflow.md).
In particular: one simple Polygon or bbox; no antimeridian contact/crossing or
polar AOIs; canonical WGS84 UTM CRS encoding; uniform unpacked 100 m axes; bounded
geometry and grids; no nonuniform or curvilinear grids; no general CRS ingestion,
reprojection, interpolation, mosaics, cross-granule alignment, NISAR or additional
authentication methods. All selected observations must have native centers to
generate an executable job. Real scientific qualification applies to the named
source versions and selections, not every matching future granule.

The next general-purpose milestone should be a shared, version-bound **native
grid inspection and geographic-selection API across the already qualified
SWOT, COG and CF-Zarr paths**. Start by eliminating repeated authenticated metadata
preparations through reusable immutable source-grid inspection, then retain the
same source-global center oracles and resource admission. The measured bottleneck
supports that work before adding more formats or scientific products.

The final independently verified local dataset is:

```text
/Users/jakegearon/.codex/worktrees/acquisition-planner/tile-ripper/prototype-output/swot-job/aoi-final-binary-checkpoint-reconstruction-rskhh4j8/observations.zarr
```

Use the xarray example in the workflow to open an observation group. Full raw
receipts, frozen mirrors and output stores remain under ignored `prototype-output/`;
compact evidence and runnable harnesses are versioned.
