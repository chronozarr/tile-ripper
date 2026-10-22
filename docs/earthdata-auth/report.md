# Authenticated live SWOT range acquisition: PASS

Validated 2026-10-22 on `codex/acquisition-planner`, continuing from `85fc657`.
The exact Version D PGD0 product qualified on the earlier mirror now passes
**direct native NASA acquisition**, independent five-layer verification and
successful final source validation. Changes remain local and unreleased.

## Product and scientific result

The canonical product is `SWOT_L2_HR_Raster_100m_UTM19T_N_x_x_x_011_035_115F_20240216T025153_20240216T025214_PGD0_01.nc` in
`SWOT_L2_HR_Raster_D`, 89,242,093 bytes, strong opaque ETag
`"3b93eefb90f0167506435fdca199aa69-1"`. [Canonical identity and method routes](access-contract.json).
The selection starts at `(y,x)=(600,400)`, is 128×128 at unit stride, and uses
the documented good-or-suspect summary policy.

Every stored WSE, UInt8 summary flag, UInt32 detailed flag, Float32 uncertainty,
UInt32 contributing count, source mask, x/y coordinate, derived mask and summary
matches separately executed **h5py and NetCDF-C**. The native executable contacts
NASA itself. Reference readers use only the existing sparse mirror after its
manifest and captured extent hashes are verified. There is no Python download
path in this acceptance run and no full product download.

The policy retains 3,076 cells: WSE mean 275.451613110 m, available per-pixel
uncertainty mean 0.379689272 m, and contributing-pixel sum 21,757. These are the
same qualified raw-value calculations; the uncertainty mean is descriptive.
[Exact live receipt and independent reader hashes](live-exact.json).

## Archive access and credential confinement

Fresh bounded probes reproduced `HEAD: archive → signed CloudFront` and
`GET: archive → URS → archive /login → archive → signed CloudFront`. The first
conditional GET returned eight HDF5 magic bytes with HTTP 206 and the pinned
ETag/length. Earlier independent provider checks also demonstrated 412 for a
wrong If-Match and 403 when a signed route was used with the wrong method.
The sampled HEAD signature advertised 3,600 seconds; GET has its own expiry.
[Safe trace and existing evidence](access-contract.json).

Earthdata documents private netrc credentials and cookie sessions in its
[official access guide](https://urs.earthdata.nasa.gov/documentation/for_users/data_access/curl_and_wget).
Tile-Ripper adds a narrower policy: private caller-owned regular netrc file,
explicit machine entry, no symlink, 64 KiB file cap and 1,024-token cap.
This credential mechanism requires Unix ownership/permission checks; other
platforms fail closed.
Authorization is reconstructed only for the configured login origin; it is
never forwarded to the archive or signed object. Separate memory-only cookie
jars confine cookies to exact origins, including ports. The signed object gets
neither cookies nor Authorization. Cookie input is capped at 64 KiB/128 headers.

The optional `authenticated-http` feature and explicit preparation API are both
required. Ordinary preparation never reads credentials. The session has no
Debug/Serialize representation. Request JSON and provenance contain public
product identity and policy only. TLS verification stays enabled; redirects
must use HTTPS, approved directed origin edges and exact destination paths.
Referer and environment proxies are disabled for this session. Unexpected
paths, hosts, downgrades, loops and excess hops fail before the next request.

## Exact object/version binding

Canonical provider locator, approved method-specific CloudFront paths for the
same S3 bucket/key, strong ETag, length and access scope form the source binding.
The configured routing adapter is `head-2c0fc4c958b8bc10f4fa177bae3b4543` for HEAD
and `s3-2c0fc4c958b8bc10f4fa177bae3b4543` for GET. Their remaining bucket/key path
must match exactly. A provider route deployment change requires qualification;
there is no hostname wildcard or length-only equivalence rule.

Every GET uses the canonical version's If-Match, exact Range and identity
encoding, and requires 206, exact Content-Range/Content-Length, the same strong
ETag and the exact body length. HEAD checks retain the pinned ETag and length.
Object conditionals reach the canonical and final object endpoints; OAuth
endpoints do not receive product conditionals. Preparation, pre-execution and
post-execution checks remain in place. A terminal authenticated failure poisons
the binding: restoring an earlier server version cannot permit a later finish.

## Method-aware resolution, expiration and bounds

HEAD always resolves independently from the canonical URL. Only GET routes
with a recognized advertised expiry are reused, for at most 60 seconds and
never beyond expiry minus 30 seconds. Unknown signing schemes resolve anew.
401/403 permit one fresh canonical resolution per logical attempt. Rejected
login credentials and 412 fail immediately. Existing transient status/body
retries remain limited to three attempts, with Retry-After at most two seconds.
Wire requests have 30-second timeouts and 10-second connection timeouts; each
chain has at most eight redirects. There is no unvalidated download fallback.

Resolution is serialized on cache misses and active operations are limited to
`min(configured workers/per-host limit,4)` within the caller-owned session.
Cancellation is checked every 20 ms while authenticated headers or bodies are
pending. Stream cancellation joins the producer. Range coalescing, selected
chunk admission, metadata/page caches and existing decoder behavior are retained.
One MiB of conservative transport working reservation is charged to HDF5
admission. Process/runtime/TLS allocator overhead is measured separately as RSS.
Session statistics are cumulative if a caller deliberately reuses the same Arc.

## Actual live request/body accounting

All three metadata strategies passed both independent scientific readers and
final validation. Counts include redirect hops, OAuth and signing requests.
Each successful session performed one login request, nine redirects and no
rejected-route refreshes. Each made five final object HEAD checks.

| Metadata | Object GET / HEAD | Wire GET / HEAD | Redirects | Source body bytes | All response-body bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| exact | 121 / 5 | 125 / 10 | 9 | 2,168,859 | 2,169,233 |
| page4k | 36 / 5 | 40 / 10 | 9 | 2,285,590 | 2,285,964 |
| page16k | 25 / 5 | 29 / 10 | 9 | 2,478,102 | 2,478,476 |

Exact acquisition reads **2,168,859 bytes, 2.430%** of the 89.24 MB object.
The additional 374 bytes are the discarded URS redirect body. The 121 object
GETs comprise 114 metadata/index reads, two coordinate reads and five science
payload reads. The 125 wire GETs include four additional authentication/redirect
requests. Ten wire HEADs comprise five canonical signers and five final checks.

| Exact source component | GETs | Encoded bytes |
| --- | ---: | ---: |
| bitwise_quality | 1 | 268,327 |
| coordinate_chunks | 2 | 1,728 |
| pixel_count | 1 | 133,023 |
| summary_quality | 1 | 334,210 |
| uncertainty | 1 | 727,811 |
| wse | 1 | 668,939 |

The five payloads total 2,132,310 bytes; axes add 1,728 bytes and metadata/index
reads add 34,821 bytes. Headers and TCP/TLS traffic are outside body accounting.

| Metadata | Prepare s | First provisional s | Complete s | Peak RSS bytes | Working reserved bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| exact | 41.173 | 44.962 | 45.702 | 32,768,000 | 28,613,477 |
| page4k | 14.787 | 18.355 | 19.066 | 33,243,136 | 28,238,241 |
| page16k | 12.540 | 16.075 | 17.036 | 33,144,832 | 29,001,041 |

The final exact run admits 28,613,477 working bytes within a 33,554,432-byte
budget and peaks at 32,768,000 RSS bytes. Its resolution/signing header timer is
7.289 s; that timer includes final-object header latency and is not an isolated
causal authentication overhead measurement. The one-login/nine-redirect counts
and 374 extra body bytes identify the directly observed negotiation work.

These are individual live provider/network observations. The earlier sealed
anonymous mirror's zero-delay three-repeat medians were 0.0658 s complete for
exact and 0.0451 s for 16 KiB, with identical source byte/request accounting.
They do not estimate WAN/provider performance. Initial page4k/page16k live
measurements charged 256 KiB of transport reservation; the final exact run
charges 1 MiB. All return the same one-tile subset. RSS differences are therefore
not a controlled strategy comparison. [Machine-readable comparison](performance.json).

One earlier initial-HEAD preparation exhausted three transport retries after
72.34 s, before scientific reads. Its diagnostic build did not retain wire-hop
counts; it is excluded from successful-run totals. No retries or refreshes were
needed in the accepted final exact session. Repeated optimization ran locally.

## Adversarial and cross-format validation

[Transport fixtures](fixtures.json) cover authorized method-specific chains,
expired routes before and after cache reuse, refresh exhaustion, 401/403/412/429,
canonical throttling, cross-origin and path attacks, downgrade, loop/hop bounds,
missing/weak/changed ETags, changed size, ignored/wrong ranges, encoded/corrupt/
truncated/oversized bodies, and cancellation during authentication, range headers
and range bodies. Concurrent expiration reads preserve values; later mutation
permanently invalidates their binding. Trap origins receive no requests, and
signed-resource origins receive neither Authorization nor cookies. Anonymous
and out-of-scope requests cannot acquire session credentials. Synthetic secret
sentinels are absent from diagnostics/provenance. The final redaction audit also
checks actual caller credential material without displaying it.

Regression receipts retain 38 five-layer science cases and 31 quality faults,
64 HDF5 science cases and 33 HDF5 fault/lifecycle cases, COG exact/fault/stream
consumers, Zarr v2/CF, 40 Zarr v3 codec/layout cases and 27 v3 faults, plus durable
publication checks/faults. Rust feature suites and strict Clippy checks pass;
[commands and counts](rust.json); [redaction audit](redaction-audit.json),
[linkage](native-linkage.json) and [reference versions](reference-environment.json). [Format receipts](formats/),
[HDF5 receipts](hdf5-regression/), [quality receipts](quality-regression/).
Repeated request traces are hash-linked to ignored local full receipts; compact
regression files retain assertions, scientific hashes and accounting. The
GDAL-free native acceptance executable links neither libgdal nor libhdf5.

## Decision and next milestone

The live-native Version D gate is **open for this qualified product and policy**.
The reusable contract can be qualified for other protected HDF5 archives that
provide exact approved HTTPS routes, netrc Basic login/cookie access, strong ETags
and conditional ranges. The public API deliberately admits one HDF5 object;
COG/Zarr authentication, bearer-token flows and other signing schemes are not
qualified here. Ordinary anonymous acquisition has no Earthdata dependency.

The next recommended milestone is bounded resumable acquisition/checkpointing
bound to this canonical identity, ETag, selection and quality policy, with final
validation still required before publication. Licensing/human alpha gates remain
separate. No merge, push, publication, tag, deployment or ChronoZarr change occurs.
