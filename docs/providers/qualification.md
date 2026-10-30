# Qualified provider routing

This milestone continues local `codex/acquisition-planner` from `fef0068`.
HLS L30 and S30 v2 now have independently examined Planetary Computer and
NASA Earthdata routes. Provider choice precedes physical preparation. The
existing COG reader, source validation and atomic scientific writer remain the
acquisition authority. No ChronoZarr changes or remote publication are included.

## Summary

Named products accept `--provider auto|planetary_computer|earthdata|pinned`
(or `"provider"` in product JSON). For example:

```sh
tile-ripper plan hls-request.json --provider auto --human
tile-ripper plan hls-request.json --provider earthdata --auth earthdata --netrc /private/path/.netrc --human
```

Fresh automatic Landsat/HLS discovery prefers credential-free Planetary Computer
access, including accountless runtime signing. HLS uses `hls2-l30`/`hls2-s30` v2
collections; legacy HLS v1.4 is excluded. OPERA retains its current Earthdata
routes. Explicit authorization is considered only when that provider is selected;
ambient credentials are never searched. No general authenticated-route speed
advantage is established, so authorization alone does not change the default.
Supplied observations pin their physical source and are never replaced. An
explicit override that conflicts with them fails; mixed-provider observations
require separate preparations.

JSON and human plans expose the provider, policy, credentials, coverage limits,
and immutable physical source identity. HLS automatic discovery compares both
bounded catalogs, records actual observation IDs/counts and failures, and
prioritizes complete coverage. With authorization,
a strict observation superset wins; incomparable sets require an explicit
selection. Without authorization, NASA-only identities are disclosed and require
a fresh authorized plan. Explicit overrides query only their selected catalog.
Cross-provider scientific evidence is limited to independently compared samples bound to exact canonical URLs and
ETags; other assets have unknown cross-provider equivalence. A matching name,
tile or date cannot create an equivalence group. The observed PC signer grants
container read/list access; the engine confines every request to the selected
canonical object. Exact read-only blob grants are also accepted; write grants
are rejected. Signatures stay in memory. Changed objects, expired signatures and unavailable sources fail and require an
explicit fresh plan. Prepared acquisitions never switch provider, and product
checkpoints remain unsupported. The sections below give the workflow, evidence and limits.

## Normal use

Build the native executable:

```sh
cargo build --locked --release --no-default-features --features native-cog,native-hdf5,authenticated-http
target/release/tile-ripper plan docs/providers/hls-s30-discovery-request.json --provider auto --human --gdal-free
target/release/tile-ripper plan docs/providers/hls-s30-request.json --provider planetary_computer --human --gdal-free
target/release/tile-ripper fetch docs/providers/hls-s30-request.json --provider planetary_computer --output NEW.zarr --gdal-free
```

JSON `provider` accepts `auto`, `planetary_computer`, `earthdata`, or `pinned`.
The CLI override takes precedence. `pinned` requires supplied local/public
physical observations. Explicit supplied observations are already a physical
choice: selecting another provider requires explicitly rediscovering them.

For an authorized NASA search, use a discovery request without `observations`:

```sh
target/release/tile-ripper plan docs/providers/hls-s30-discovery-request.json --provider earthdata --auth earthdata --netrc PRIVATE_FILE --human --gdal-free
```

Neither catalog discovery nor automatic mode opens ambient credentials. A
netrc is consulted only for an actually selected protected route, following an
explicit API argument or `--auth earthdata --netrc PRIVATE_FILE`.

## Policy and exact observation coverage

Automatic HLS discovery compares two bounded catalogs, with no science fetch
to choose a provider. Each catalog must pass independent product/version,
asset and completeness checks. The inspection reports actual observation IDs,
counts, failures, and the difference between provider sets.

Without authorization, a complete usable PC result is preferred. NASA-only
identities are disclosed; that result is not claimed to exhaust NASA's
coverage. If only protected observations exist, discovery explains the required
authorization and preparation refuses access before inspecting science assets.

With explicitly configured authorization, a strict observation superset wins
before convenience or speed. Equal identity sets prefer credential-free PC.
Incomparable sets require an explicit choice; neither is silently truncated to
the other. A failed catalog has unknown coverage, never an empty observation
set. A complete surviving catalog can be selected with that uncertainty shown.

This policy does not equate different observations or processing versions.
Exact source interpretation is independently checked during preparation.
There is no demonstrated general material end-to-end authenticated speed
advantage, so no static speed exception is installed. The policy is deliberately
conservative, with no learning service or performance database.

[Discovery receipts](discovery-validation.json) include the recent
`HLS.L30.T17SPU.2026291T154740.v2.0` coverage gap: NASA lists it while PC does
not. PC returned HTTP 404 for the exact item, and its corresponding bounded
search was empty. That is observation-specific evidence, not a latency or
archive-completeness estimate. Separate S30 and older L30 matched searches are
recorded with both authorization states.

## Scientific gate

[Independent equivalence evidence](equivalence.json) compares actual NASA
production objects and PC `hls2-l30` / `hls2-s30` objects:

| Product | Exact granule | Complete selected sample |
| --- | --- | --- |
| L30 v2 | `HLS.L30.T15VUE.2022231T172609.v2.0` | B04 and Fmask, x1792/y1792, 32×32 each |
| S30 v2 | `HLS.S30.T59UNU.2026295T235129.v2.0` | B04 and Fmask, x1124/y1659, 32×32 each |

All selected native values agree. Native dimensions, CRS, affine, dtype,
fill, scale, offset, band description, original processing/sensing/input-scene
tags and band metadata agree. Both samples retain the correct mission inputs;
the older L30 tile combines two Landsat 9 source scenes, whose original
identifiers and sensing timestamps remain intact. Sample arrays themselves
are in the receipt, alongside object sizes, ETags, canonical locators and
bounded range hashes. Independent rasterio/GDAL reads supply the references;
Tile-Ripper output is never used as its own oracle.

The built-in attestation binds the exact product, granule, band, canonical
URL and inspected strong ETag. Changed versions, another URL, another band,
legacy HLS 1.4, or an unexamined granule do not inherit the group. The attached
scope explicitly describes selected-sample science equality. No whole-file
byte identity, whole-observation equality or archive-wide equivalence is
claimed. Equal sizes and different provider ETag strings do not establish
whether complete encodings are identical. Unknown sources remain unknown
across providers, although their own product route can pass physical admission.

The signed/unsigned access distinction is measured: unsigned HLS Azure HEADs
returned HTTP 409, accountless signing returned a usable URL, and signed
HEAD/range requests exposed strong object versions. PC's observed delegated
SAS grants container read/list (`sp=rl`, `sr=c`); the engine confines use to
the exact canonical object path. It also accepts a read-only blob grant.
Write grants, query-based object changes, changed authorities/paths, duplicate
authorization fields and expired signatures are rejected. The signing policy
is `pc-exact-object-request-v1`. A token's authorization scope is broader than
the engine's allowed request scope; these are not conflated.

## Pinning, failures and security

Prepared source provenance records provider, canonical unsigned URL,
product/granule/band, access-policy version, strong ETag, source size, grid,
dtype, missingness and calibration. Signed URLs remain private runtime
locators. Existing source checks apply before, during and after acquisition;
provisional streams cannot claim completion after a version failure, and
durable output is published only after final validation.

There is no automatic signature renewal or provider fallback. An unavailable
signer, expired grant or unavailable object fails with fresh-plan guidance.
Changing provider requires a new preparation and version validation. Product
COG checkpoints remain explicitly unsupported; mixed-provider checkpoint
replay is rejected before any download. Existing authenticated HDF5 range
checkpoints retain their separate contracts.

An initial live signing attempt returned HTTP 504 and stopped without
fallback. Live development also caught an overly narrow validator that expected
a blob-only grant; that implementation error was corrected against the
observed container grant before acceptance. It is not counted as a provider
reliability failure. An exploratory header dump exposed a temporary NASA
redirect URL in the chat tool output. Saved scripts and receipts exclude
temporary URLs and credentials; subsequent inspection prints only canonical
identities and nonsecret grant characteristics.

## Performance and reproducibility

`bench/provider_qualification.py` records independent source inspection,
signing/auth overhead, original-byte capture, native acquisition, output and
CPU/RSS. Science capture is capped at 32 MiB and 240 range requests per
four-asset matched case, with no whole-object science GET. Captures used
2,621,440 bytes/8 GETs for L30 and 2,097,152 bytes/8 GETs for S30. Captured
pages preserve original offsets and fail closed on uncaptured reads.

[Measurement summary](performance.md) and [controlled measurements](mirror-performance.json) run identical native
selections and durable output, one worker/host, 5 ms per loopback request and
a shared 100 Mbit/s link. Two interleaved rounds separate decoder/output work
from access overhead. They include exact local HEAD/GET and response-byte
counts. [Live measurements](live-performance.json) execute the same pinned
science requests through real providers and retain failures separately.

Catalog lookup and separate signing/auth inspection costs are in the
equivalence and discovery receipts. Each live execution's wall time includes startup, access, metadata/version inspection, acquisition and durable output. The first-output timer begins at the execution stage after preparation; it is not mislabeled as a process-start timer. Native encoded data counts and NASA's authenticated wire counters are
reported separately. PC's anonymous native reader does not expose a total
wire counter covering all HEADs and metadata retries: those totals are not
invented from data-only counts. Exact total wire counts exist for the
controlled mirror and the independent original-source capture. Thus these
receipts are bounded comparative observations, not a global fastest-provider
claim or a complete network-billing instrument.

The [acceptance receipt](acceptance.json) lists the feature matrix, security,
checkpoint, source-version and atomic-output regressions. Six prior Wave 1
frozen scientific oracles were rerun; their historical receipts were preserved.

## Alternative-source audit and next wave

[Current catalog audit](catalog-audit.json) and primary documentation support
these admission decisions:

| Existing family | Alternative audit | Decision |
| --- | --- | --- |
| Landsat 8/9 C2 L2 SR | PC plus USGS AWS distribution; USGS documents `usgs-landsat` as requester-pays | Keep PC; no requester-pays requests or unqualified alternate route |
| HLS L30/S30 v2 | Current PC `hls2-*` and NASA LP DAAC; actual matched assets independently compared | Admit separate routes; explicit object-bound sample attestations |
| Sentinel-2 | Existing Earth Search C1 and PC `sentinel-2-l2a`; collection/baseline/offset semantics can differ | Retain Earth Search; no interchangeable PC substitute without a matched baseline/value gate |
| OPERA DSWx-HLS | Qualified PO.DAAC production route and existing public reference specimens; public metadata bucket is not the protected science bucket | Retain existing routes; no new production mirror claimed |
| OPERA RTC-S1 | Qualified ASF Cumulus production and existing JPL public specimens | Retain existing routes; no equivalent additional production distribution established |

Microsoft's [HLS ingestion implementation](https://github.com/microsoft/planetary-computer-tasks/blob/main/datasets/hls2/hls2.py)
reads NASA's supplied STAC and rewrites asset locators. Its
[configuration](https://github.com/microsoft/planetary-computer-tasks/blob/main/datasets/hls2/dataset.yaml)
uses the examined HLS2 account/container and documents an enumeration change;
this supports distribution interpretation but does not replace value checks.
[NASA HLS access documentation](https://hls.gsfc.nasa.gov/data-access-and-tools/)
requires Earthdata Login for downloads.
[USGS cloud access](https://www.usgs.gov/landsat-missions/landsat-commercial-cloud-data-access)
identifies the requester-pays boundary.
[Earth Search's owning repository](https://github.com/Element84/earth-search)
documents C1 separately from the older collection and distinguishes requester
pays S3 locators from public HTTPS.
[PO.DAAC's DSWx page](https://podaac.jpl.nasa.gov/dataset/OPERA_L3_DSWX-HLS_V1)
distinguishes protected science and public metadata buckets.

The next major coverage wave should prioritize historical Landsat 4–7 C2 L2, OPERA DSWx-S1, and OPERA DIST, in that order of reuse from the existing roadmap. Each needs its own real-source science/access gate. Before that wave, the highest-value qualification work within current families is a Sentinel-2 matched-processing-baseline gate and broader HLS reflectance/QA specimens. None of those new families is implemented here. AlphaEarth remains a separate learned-representation stream.
