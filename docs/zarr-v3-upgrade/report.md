# zarrs 0.23.14 dependency gate — 20 October 2026

The upgrade from f885e50 passes the existing scientific, streaming, native COG,
Zarr v2, failure/publication and memory gates. The deterministic upstream
reproduction passes with the fixed release. All 144 matched output benchmark
cells pass, including the 12 formerly failing medium/large sharded gzip cases.
The optimized uncompressed v2 default and publication ordering are preserved.
V3 input and additional experimental output profiles belong to the next
commits in this milestone.

## Dependency and API changes

- Pin zarrs 0.23.14 and declare Rust 1.91, the dependency's new minimum; the
  exercised compiler is Rust 1.92.0. The package remains Rust 2021.
- Migrate moved `ArraySubset`/`CodecOptions` types, generic array store/retrieve
  calls and datatype factories. No compatibility shim or replacement codec.
- Enable crc32c because this release's sharding builder requires it to compile,
  including a bytes-only index configuration. This does not add index checksums
  to the existing experimental output metadata.
- Keep zarrs_storage 0.4.5, both filesystem 0.3.13 identities and the scoped
  synchronization patch. The storage trait implementations remain compatible.
- Update root and separate consumer lockfiles together. The
  [19-entry registry delta](dependencies.json) records exact versions, checksums,
  licenses and publication dates. All predate the historical October 11 cutoff.
  Conditional WASM Blosc dependencies are recorded without claiming WASM support.
- Correct the writer version in output provenance to zarrs 0.23.14.

The [released upstream fix](https://github.com/zarrs/zarrs/releases/tag/zarrs-v0.23.14)
corrects gzip's encoded-size bound and guards shard writes. The standalone
fixed-release program, with the same zarrs checksum and native zlib-rs backend
as the upgraded application, passes all 98 generated/real input profiles again.
The application benchmark independently exercises that fixed code with the
full production dependency graph.

## Verification

[Verification receipt](verification.json):

| Rust configuration | Passed | Existing ignored |
| --- | ---: | ---: |
| Default GDAL | 65 | 2 |
| GDAL-free native COG | 43 | 3 |
| GDAL-free native + experimental output | 43 | 3 |
| Combined all features | 83 | 4 |

Strict clippy passes for default/native/all features; Zarr-only compilation,
release native build, separate locked consumer build, formatting and diff
checks pass. The remote fixture also runs its ignored version/cancellation
tests; external fixture ignores remain explicitly counted above.

Independent checks were rerun into this directory rather than overwriting
historical receipts:

- [Mixed grids, values, masks, exact timestamps, mutation, interruption and admission](candidates.json),
  including the inherited default checks; [Zarr-Python 2 reopen](interoperability-v2.json).
- [31 syscall/publication fault cases](faults.json).
- [61 native COG faults, 32,391,168 exact real-source samples and seven consumer checks](regressions.json).
- [Remote v2 HTTP checks](remote-zarr-checks.json) and
  [CF Float32 orientation, codecs, missingness and corruption](cf-zarr-checks.json).
- [64-fold logical-volume memory gate](memory.json): large peaks 68.1 MiB raw,
  68.7 gzip, 69.0 Blosc/Zstd and 71.9 experimental sharded gzip, passing the
  192 MiB peak and less-than-64-MiB growth screens. Reservations remain distinct
  from RSS and caller-retained buffers.

## Matched durable-output benchmark

The baseline is the actual preserved f885e50/0d0cc18 optimized binary, SHA-256
`98016f9ef24367c4fd455c98d317f4121fa21187c74ac3c1232647c57231de92`.
It is not the older 82e21b1 writer. Both binaries retain checked per-file
synchronization and publication barriers. Four unchanged frozen Sentinel
observations, the same 256-edge deliveries, two workers/per-host and 16 MiB
source budget are used; every successful output is checked with independent
Zarr-Python/xarray/GDAL references outside write timing.

Large-window medians across three repetitions, seconds:

| Link | Optimized 0.22.10 baseline | Upgraded default v2 | Upgraded experimental sharded gzip |
| --- | ---: | ---: | ---: |
| Warm local HTTP | 0.4302 | 0.3559 | 1.1399 |
| 8 ms / 100 Mbit/s | 3.4583 | 3.4208 | 4.1441 |

This is a preservation result on one warm-cache workstation, not a new public
provider speed claim. Differences from the earlier absolute medians do not
substitute for the matched comparison. The data-layer large cells retain the
same 20 HEAD/24 GET and 25,134,367 response-body bytes. The two explicit macOS
F_FULLFSYNC transaction requests remain in place. They are observed requests,
not measured hardware flushes or power-loss certification.

The last shaped-network group briefly overlapped a mistakenly started memory
check. That check was stopped; all 24 cells in the group were discarded and
rerun in isolation. The final [144-cell receipt](benchmark.json) uses only the
rerun. Rejected measurements and the interruption record remain in ignored
prototype-output; no rejected timing is presented as a successful result.

## Reproduction

Build commands and versions are recorded in the verification receipt and
previous durable-output protocol. `bench/zarrs_upgrade_gate.py` redirects the
existing gates to fresh work/receipt directories:

```sh
prototype-output/durable-output/reader3/bin/python bench/zarrs_upgrade_gate.py checks
prototype-output/zarr-env/bin/python bench/zarrs_upgrade_gate.py reopen-v2
prototype-output/zarr-env/bin/python bench/zarrs_upgrade_gate.py faults
prototype-output/zarr-env/bin/python bench/zarrs_upgrade_gate.py regressions
prototype-output/zarr-env/bin/python bench/zarrs_upgrade_gate.py remote
prototype-output/zarr-env/bin/python bench/zarrs_upgrade_gate.py cf
prototype-output/durable-output/reader3/bin/python bench/zarrs_upgrade_gate.py benchmark
prototype-output/durable-output/reader3/bin/python bench/zarrs_upgrade_gate.py memory
```

Use fresh generated destinations for a new acceptance run. The benchmark helper
preserves/reuses the actual baseline binary and resumes complete cells. Do not
run builds or other validation jobs concurrently with performance measurements.
The three older fixture harnesses now accept `TILE_RIPPER_VALIDATION_DOCS` only
to redirect receipts; their default behavior is unchanged.

No source-format expansion, output-default change, merge, publication, tag or
deployment is included in this dependency gate.
