# Wave 4 measured workflow costs

Fresh native and mature-reader processes use identical frozen guarded source bytes. Five one-shot comparisons cover both inspected NISAR grids, the new GOES layout, and both polar SMAP passes. Process times include startup/imports; native and mature selection/index work differ. These small AOIs do not establish throughput leadership.

| Path | Included centers | Native end-to-end s | Mature end-to-end s | Native CPU s | Native peak RSS MiB | Native GET / HEAD | Source bytes | Physical decode / selected value bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| nisar | 64 | 0.0663 | 0.7185 | 0.0413 | 28.09 | 55 / 6 | 2346700 | 4096.0× |
| goes | 100 | 0.2116 | 0.9265 | 0.1831 | 21.61 | 8 / 6 | 282742 | 1740.0× |
| smap | 9 | 0.6269 | 0.6808 | 0.5670 | 30.81 | 257 / 6 | 1416222 | 666.7× |
| nisar20 | 64 | 0.0696 | 0.6772 | 0.0343 | 25.97 | 34 / 6 | 1526253 | 4096.0× |
| smap-pm | 9 | 0.6402 | 0.6759 | 0.5757 | 30.53 | 256 / 6 | 1419410 | 666.7× |

[Machine-readable measurements](performance.json) retain source request events, physical decoded/selected byte counts, grid inspection and selection time, physical planning, decode/delivery and durable output costs, first-chunk latency, CPU/RSS and working reservations for both workflows. [Live-native checks](live-native-validation.json) separately measure canonical provider access and authentication within preparation. [Catalog runs](catalog-workflows.json) report discovery and demonstrate automatic NISAR preparation from geographic bounds and polygon without supplied observations. Authentication has no separate isolated timer; it is included in live inspection/preparation, and the reports do not claim otherwise.

Capture manifests count total bounded original bytes, including inherited seeded extents; they are not cold-provider performance measurements. Repeated comparison uses frozen replay and refuses uncaptured holes. Native payload reservations are admission bounds, not process-RSS limits. Mature Python imports/runtime are included in its process memory. The independent full-source NISAR coordinate sweep took about 122 s and ~155 MiB RSS; it is correctness evidence, not the mature benchmark algorithm.

The same-grid mature workflows preserve raw values and separate masks and validate source identity before/after output. Native performs full restricted-index validation and richer product/calibration checks; mature HDF5-C may traverse fewer index nodes. Both flush data/directories and use macOS device-flush barriers before/after publication in fresh benchmark namespaces. The native writer retains its no-replace transactional semantics. No paid cloud service or provider-throughput test was used.
