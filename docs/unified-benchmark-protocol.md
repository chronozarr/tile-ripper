# Unified lifecycle benchmark protocol

Two small/medium workload pairs, three readers and five repetitions give 60
fresh acquisition processes. Reader order is deterministically shuffled within
each repetition. Data are deterministic local files and a loopback STAC/COG
mirror. No repeated public-provider traffic. The old release executable was
captured before rebuilding and is hashed in the results; reproduction from
46dd503 is documented in the workflow guide.

Zarr small is the previous 4×128×160 cube with two variables, irregular dates,
gzip, differing chunk shapes, dot/slash keys, source nodata and one missing
chunk. Its selection returns 29,904 values. Medium is 4×256×320, returning
650,760 values. Both use nonaligned bounds. Independent xarray coordinate
selection and native chunk-grid products define the minimum keys; old slice
visits are independently reconstructed and match its observed counts exactly.
Current planned and observed keys must equal that minimum with one call per
key. xarray DirectoryStore gets and existence probes are separately recorded.

The COG pair retains the earlier STAC fixture's small/large AOIs (called
small/medium here), complete pagination, exact footprint rejection, adjoining
scenes, 10/20 m grids and EPSG:32617/32618. The tuned baseline does real STAC
pagination with an independently known selected item set and tuned GDAL.
Every output from all readers is independently checked.

Reported measurements:

* Process wall and per-child peak RSS include startup/imports and completion.
* STAC discovery time and metadata/planning time are recorded separately;
  explicit Zarr assets have no discovery stage.
* Production read/decode is fused in both mature readers. Separate COG
  transfer-only and memory decode/assembly probes, and Zarr DirectoryStore
  read-only plus Numcodecs gzip decode-only probes, are nonadditive independent
  experiments, not invented per-stage Rust latency. Zarr baseline additionally
  times its data-store reads within xarray's fused load.
* Sink callbacks/writes are measured. Their boundaries differ: xarray's complete
  output call includes metadata; Rust callbacks exclude output initialization
  and final provenance work. Unattributed work remains in wall time.
* COG counters come from the independent HTTP server, including discovery,
  metadata, conditional/ranged GETs and body bytes. Zarr counters cover data
  object gets/encoded bytes, excluding coordinates/metadata/stat checks and
  output traffic. They are local storage calls, not physical disk/network bytes.
* `chunks` counts delivered callback buffers, not source object reads.
  `decoded_bytes` is the legacy count of delivered UInt16 bytes;
  `native_materialized_bytes` additionally counts native chunk buffers,
  including fill for missing objects. Neither is wire traffic or peak memory.

All caches are warm at the OS level. Source/lock/binary hashes and versions are
recorded. This is a single active workstation, not an isolated performance lab.
Report median, range and sample standard deviation; do not infer a general
speed advantage from process ratios. Output chunking/encoding and runtime
startup differ. Buffer admission is not a hard RSS bound. In particular, lower
Rust RSS on these small cubes does not establish a memory scaling advantage.

## Final acquisition measurements

Five repetitions per cell. RSS is the maximum per-process observation.

### ZARR

| Workload | Reader | Wall median [min–max], s | SD, s | Peak RSS, MiB | Data reads | Encoded data/body bytes |
|---|---|---:|---:|---:|---:|---:|
| medium | previous-46dd503 | 3.5395 [3.4345–7.5198] | 1.7594 | 40.1 | 1188 | 13,503,563 |
| medium | unified | 11.2883 [10.6637–14.7482] | 1.6514 | 40.1 | 112 | 879,821 |
| medium | xarray | 0.3371 [0.3289–0.3767] | 0.0191 | 81.8 | 111 | 879,821 |
| small | previous-46dd503 | 0.4152 [0.3790–0.4479] | 0.0260 | 36.8 | 110 | 297,646 |
| small | unified | 0.8497 [0.8328–0.9511] | 0.0483 | 38.0 | 42 | 109,903 |
| small | xarray | 0.3607 [0.3216–0.4496] | 0.0510 | 80.2 | 41 | 109,903 |

### STAC

| Workload | Reader | Wall median [min–max], s | SD, s | Peak RSS, MiB | Data reads | Encoded data/body bytes |
|---|---|---:|---:|---:|---:|---:|
| medium | gdal-stac-tuned | 0.2454 [0.2416–0.2577] | 0.0063 | 76.3 | 8 HEAD / 13 GET | 556,108 |
| medium | previous-46dd503 | 0.1708 [0.1685–0.1737] | 0.0021 | 55.1 | 12 HEAD / 17 GET | 687,180 |
| medium | unified | 0.1721 [0.1687–0.1751] | 0.0027 | 55.8 | 12 HEAD / 17 GET | 687,180 |
| small | gdal-stac-tuned | 0.2385 [0.2357–0.2545] | 0.0075 | 75.8 | 8 HEAD / 12 GET | 327,142 |
| small | previous-46dd503 | 0.1653 [0.1644–0.1932] | 0.0125 | 53.9 | 12 HEAD / 16 GET | 458,214 |
| small | unified | 0.1677 [0.1645–0.1699] | 0.0022 | 54.4 | 12 HEAD / 16 GET | 458,214 |

Zarr xarray counts exclude a missing-object get satisfied by an existence probe;
its complete probed/get key union equals the 42/112-key minimum. COG rows also
made three discovery requests each. Source/body byte counts do not include
output files. The Zarr wall regression is real and dominated by partial output
chunk updates; source read savings do not establish an end-to-end improvement.
