# Wave 3 measurements

These serialized, fresh-process runs use identical guarded original-source bytes on macOS ARM64. The native path and independent h5py/HDF5-C → PROJ → Zarr-Python path validate source identity before and after reading, retain original arrays and masks, write complete outputs, flush files/directories, and perform two macOS device-flush barriers around publication. Python imports are included. These small correctness selections do not establish throughput leadership.

| Family | Native wall / CPU (s) | Mature wall / CPU (s) | Native / mature RSS (MiB) | First chunk from preparation (s) |
| --- | ---: | ---: | ---: | ---: |
| goes-c13 | 0.6165 / 0.0405 | 0.4721 / 0.4471 | 21.45 / 90.06 | 0.013575 |
| smap-am | 0.1196 / 0.0609 | 0.4474 / 0.4149 | 30.27 / 75.38 | 0.063590 |
| nisar | 0.0625 / 0.0315 | 0.3944 / 0.3683 | 27.03 / 78.38 | 0.031483 |

Wall time includes process startup and durable completion. First-chunk timing starts at preparation and is a separate instrumented Rust execution measurement. It is not time from process launch. The native path validates a full legacy chunk index; the mature reader may traverse less index metadata. Output metadata/provenance size and index work therefore differ.

| Family | Inspection / selection / physical plan (s) | GET / HEAD | All response bytes / science payload bytes | Full decoded chunk bytes / selected-value amplification |
| --- | ---: | ---: | ---: | ---: |
| goes-c13 | 0.013454 / 0.000001 / 0.003084 | 8 / 6 | 249,813 / 139,756 | 520,000 / 169.27× |
| smap-am | 0.066435 / 0.000001 / 0.004178 | 215 / 6 | 1,184,073 / 29,001 | 370,176 / 482.00× |
| nisar | 0.028621 / 0.000000 / 0.001004 | 55 / 6 | 2,346,700 / 659,148 | 1,048,576 / 4096.00× |

Decoded amplification uses entire source science/quality chunks divided by selected science/quality bytes, before masks/coordinates. Metadata is included in all-response bytes and excluded from science-payload bytes. The table counts mirror resource requests; live authentication includes extra redirects/validation and has a different request scope.

| Family | Requested budget (MiB) | Peak reader reservation + output reservation (MiB) | Native objects / bytes | Mature objects / bytes |
| --- | ---: | ---: | ---: | ---: |
| goes-c13 | 32 | 3.60 + 8 | 58 / 327,008 | 57 / 39,055 |
| smap-am | 64 | 11.25 + 8 | 61 / 1,278,839 | 60 / 20,992 |
| nisar | 64 | 24.57 + 8 | 40 / 219,727 | 39 / 9,093 |

Reservations bound reader/output working state; process/runtime overhead and caller retention are outside that budget. Process RSS is measured separately above. Native output includes the full acquisition manifest and source metadata; output byte counts are not equal-work compression scores.

## Catalog, geographic selection and live authentication

- GOES bounded live catalog discovery: 0.223117 s; 1 returned observation(s), `noaa` route selected before physical preparation.
- SMAP bounded live catalog discovery: 0.197357 s; 1 returned observation(s), `earthdata` route selected before physical preparation.
- NISAR bounded live catalog discovery: 0.239949 s; 1 returned observation(s), `earthdata` route selected before physical preparation.
- GOES C13 geographic center selection: 0.251644 s, measured separately from inspection.
- GOES C01 geographic center selection: 0.955395 s, measured separately from inspection.
- SMAP am geographic center selection: 0.028485 s, measured separately from inspection.
- SMAP pm geographic center selection: 0.029250 s, measured separately from inspection.
- NISAR geographic AOI translation is not admitted; native-index selection takes 0 s in the benchmark selection timer. Inverse navigation for selected centers is independently checked.

| Live route | Signing/authentication (s) | Wire GET / HEAD | Response body bytes |
| --- | ---: | ---: | ---: |
| SMAP am | 8.742179 | 219 / 12 | 1,184,395 |
| SMAP pm | 7.550619 | 228 / 12 | 1,238,702 |
| NISAR HHHH | 8.209920 | 59 / 12 | 2,347,132 |

SMAP live values/coordinates/masks were rechecked against independent source bytes when recovering the summary from completed native manifests. Original outer-process CPU/RSS/wall measurements were not retained; mirror process measurements above remain available. NISAR live complete fetch took 25.043691 s (CPU 0.359077 s; peak RSS 31.03 MiB). Authentication totals are measured inside the transport and should not be added again to fetch wall time.

## Bounded original-source capture

| Source | Logical object bytes | Successful captured body bytes | GETs |
| --- | ---: | ---: | ---: |
| goes-c13 | 4,042,217 | 487,844 | 222 |
| smap-am | 594,638,674 | 3,481,600 | 587 |
| nisar | 7,017,070,592 | 4,374,528 | 267 |
| GOES C01 | 14,394,371 | 576,491 | 229 |

Capture costs precede replay/benchmark work and are not included in mirror timing. An earlier broad SMAP inspection reached its 8 MiB cap and stopped; the successful 3,481,600-byte sealed capture is not the total exploratory provider traffic. The additional SMAP date used 564,083 original bytes. No whole source object was downloaded. Captures cover selected original extents, not complete object hashes.

See [performance.json](performance.json) for exact measurements, source phases, mature-reader request counts and binary identity; [acceptance.json](acceptance.json) for commands, platform checks and log hashes.
