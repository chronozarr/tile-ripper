# Native Sentinel request profile

The excess GET count is understood. It combines repeated metadata paging,
one request per selected encoded tile, and conservative reader admission. TCP
connections are reused. This investigation changes no production transport or
buffer reservation; adjacent-range coalescing remains a measured opportunity,
not an installed optimization.

## Evidence and timing scope

[`request-profile.json`](request-profile.json) records 24 fresh-process diagnostic
replays: three preserved readers, four frozen workloads, and unshaped loopback or
8 ms / shared 100 Mbit/s. Every replay passes the existing independent rasterio
science/coverage oracle and applicable RSS/HEAD gates. Native and baseline chunk
records agree, including ordering. The mirror index and baseline binary digest
are checked before replay. Each request is matched against physical TIFF
metadata and every image IFD; raw ranges and socket receipts stay in the ignored
`prototype-output/native-sentinel/request-profile-v2/` directory. There is no
provider capture or new scientific source material.

The existing [72-trial benchmark](../native-cog/benchmark.json), its
[protocol](../native-cog/protocol.md), and the independent
[local read/decode trials](../native-cog/local-decode.json) remain the performance
authority. The new single-repeat measurements diagnose requests and concurrency;
they are not a replacement performance comparison. Server timers include the
artificial delay, mirror reads and shared-link writes. They exclude TLS and
public-provider latency. Sums of concurrent worker/request durations are not wall
time. Native read/decode minus synchronous range time estimates decode, copying
and allocation together; it is not an isolated codec timer.

## Where the GETs go

Shaped accepted-matrix counts, confirmed by the new physical traces:

| Workload | Reader | Metadata/header GETs | Payload-bearing GETs | Total GETs |
| --- | --- | ---: | ---: | ---: |
| many-small | Native | 64 | 16 | 80 |
| many-small | Tuned GDAL | 16 | 16 | 32 |
| several-large | Native | 24 | 24 | 48 |
| several-large | Tuned GDAL | 6 | 24 | 30 |
| overlap-a | Native | 24 | 24 | 48 |
| overlap-a | Tuned GDAL | 6 | 24 | 30 |
| overlap-b | Native | 24 | 24 | 48 |
| overlap-b | Tuned GDAL | 6 | 12 | 18 |

Native opens each source once for layout inspection and again for the selected
reader. Each opening requests pages `[0,4096)` and `[4096,8192)`. For these
sources the actual TIFF metadata intervals are `[0,8)` and `[192,4208)`: just
112 bytes spill into the second page. Thus six sources need
`6 sources × 2 opens × 2 pages = 24` metadata GETs; sixteen sources need 64.
The selected readers then fetch four tiles/source or one tile/source.

These are *metadata-phase requests*, not exclusively metadata bytes. The native
metadata pages touch 47,764 bytes of unselected image payload in each six-source
workload and 127,388 bytes in many-small. Those bytes, exact metadata, framing,
padding and all duplicate intervals remain in the physical byte accounts. Native
data GETs include the COG four-byte leader and trailer around each compressed
tile. Selected tile payload is not fetched twice in these workloads.

For several-large, the entire 18-GET difference from tuned GDAL is metadata
requests. Overlap-b adds a second difference: GDAL combines two adjacent tiles
in each of its 12 payload GETs. Its selected byte count is therefore close to
native's, whereas several-large GDAL transfers substantial unselected imagery.
The frozen shaped totals remain 37.747 MB native versus 56.820 MB tuned GDAL for
several-large, and 37.747 versus 38.044 MB for overlap-b.

## Adjacency and bounded coalescing

All three four-tile workloads select the same two-by-two native tiles. Within a
row, compressed payloads have an eight-byte gap. Their current leader/trailer
wrapped ranges are exactly adjacent. Across rows, the intervening gap is
13,134,891–14,275,449 bytes and contains unselected tiles.

An exact, selection-aware horizontal merge could reduce 24 tile GETs to 12
without increasing current encoded-body traffic. The largest merged wrapped
range would be 3,285,284 bytes, below the transport's 8 MiB single-read ceiling.
The payload-only interval calculation reports 96 gap bytes across twelve pairs;
those framing bytes are already present in the current wrapped requests. There
is no adjacency gain for the one-tile many-small selections. Cross-row merging
would be substantial overfetch and is not justified.

This does not make blind range read-ahead safe. `TiffSource` receives one tile
request at a time; the transport has no selected-neighbor information. Retaining
a merged encoded buffer while returning an owned individual tile adds memory
which the existing one-encoded-tile reservation does not cover. For these six
sources, adding a retained row buffer to the current cache/decode/output
reservation would require approximately 17.43–17.86 MB per active reader,
exceeding the declared 16 MiB (16.78 MB) budget. A correct optimization needs
selection hints, explicit encoded-buffer accounting, a budget-based fallback,
exact response splitting and the full source-version/fault matrix. None is
silently added here.

An 8 KiB metadata prefix would halve metadata GETs with identical metadata-phase
bytes on these fixtures: native totals would become 48→36 for the six-source
cases and 80→48 for many-small. This remains a counterfactual. Uniformly widening
pages reads extra bytes for profiles whose metadata fits in 4 KiB and consumes
the fixed metadata-cache budget faster for scattered metadata. The current
512 KiB cache, 8 MiB read limit, exact byte accounting, conditional requests and
version checks remain unchanged.

## Connections, decoding and scheduling

The socket trace rules out one new connection per native GET. In the diagnostic
six-source runs, 48 GETs use three GET-bearing TCP connections; 45 reuse a
connection that already served a GET. HEAD validation uses additional sockets,
bringing the total to nine. The profile retains each accepted connection's
request counts. This is local HTTP/1.1 evidence, not a claim about TLS connection
behavior at a public object store.

Native reserves an 8 MiB decoded cache for a four-tile selection, plus encoded,
decode and output working space. The 16 MiB budget admits one active reader;
the baseline GDAL scheduler admits two. Independently tuned GDAL uses four or
eight worker slots, with actual concurrent payload requests limited by source
count. The new trace observes one native payload GET at a time for all three
six-source workloads, versus multiple simultaneous GDAL payload GETs. The
many-small native selections reserve a smaller cache and admit two readers.
Increasing concurrency without changing the memory model would violate the
current working-buffer contract.

The frozen local read/decode medians are 0.220 s native versus 0.164 s current
GDAL for several-large, and 0.202 versus 0.160 s for overlap-b. Native's shaped
read/decode residual after measured range time is 0.323 and 0.241 s respectively.
Decode/assembly is a real cost; most shaped native read/decode time is still
spent in synchronous range retrieval. Eighteen extra GET delays at 8 ms amount
to 0.144 s of aggregate request service before overlap, not an explanation by
themselves for the entire 1.33 s overlap-b wall-time loss. Request count,
serialized readers, body-transfer scheduling and decoding all contribute;
the diagnostic does not assign an unsupported causal percentage to each.

The accepted shaped completion medians remain 4.830 s native / 4.845 s tuned
GDAL for several-large, and 4.713 / 3.383 s for overlap-b. Understanding the GET
count does not remove the overlap-b regression or justify changing defaults.

## Reproduction

```sh
prototype-output/zarr-env/bin/python bench/native_request_profile.py \
  --label a-new-request-profile
```

Use `--native-binary /absolute/path/to/a/new/consumer` to diagnose a separately
built candidate; preserve the frozen native and baseline binaries. `--output`
selects a different compact report path. The output directory label must be new.
The harness never changes source bytes, binaries or production configuration.
