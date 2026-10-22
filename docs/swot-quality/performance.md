# Performance and physical acquisition

Medians of three fresh processes per cell; RSS columns use the maximum peak. Zero/20 ms delays apply per loopback HEAD/GET, with no bandwidth throttle. No provider traffic occurs. CPU includes native child startup; mature API and process CPU are retained separately. Native validates more metadata and emits provisional tiles; mature first results are complete slices. NetCDF-C local kernel I/O is not instrumented. All file/cache reuse is stated in workflow.md.

## conditional_loopback, 0 ms added response delay

| Selection | Reader / operation / policy | Prep ms | First ms | Total API ms | Process CPU ms | API CPU ms | RSS MiB | GET/HEAD | Bytes | Reservation MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| window | native / wse-only / exact | 23.89 | 31.95 | 32.55 | 22.19 | - | 22.06 | 73/5 | 693244 | 21.40 |
| window | native / multi-raw / exact | 36.39 | 44.79 | 64.79 | 47.58 | - | 25.62 | 121/5 | 2168859 | 22.16 |
| window | native / multi-aligned / exact | 36.62 | 64.91 | 65.78 | 47.98 | - | 26.69 | 121/5 | 2168859 | 26.29 |
| window | native / wse-only / page16k | 11.26 | 19.30 | 19.84 | 18.45 | - | 21.97 | 17/5 | 916427 | 23.03 |
| window | native / multi-raw / page16k | 15.67 | 24.16 | 44.12 | 41.81 | - | 25.45 | 25/5 | 2478102 | 24.23 |
| window | native / multi-aligned / page16k | 16.24 | 44.30 | 45.09 | 41.61 | - | 26.41 | 25/5 | 2478102 | 27.41 |
| window | h5py / multi-quality / 0 | 102.65 | complete slice | 149.19 | 313.86 | 128.10 | 65.67 | 55/2 | 2172081 | - |
| window | h5py / multi-quality / 16384 | 88.87 | complete slice | 130.70 | 303.79 | 117.62 | 66.16 | 25/2 | 2509142 | - |
| dense | native / wse-only / exact | 22.89 | 31.16 | 31.81 | 22.49 | - | 21.98 | 73/5 | 693244 | 21.41 |
| dense | native / multi-raw / exact | 36.17 | 44.83 | 65.58 | 50.31 | - | 28.05 | 121/5 | 2168859 | 22.18 |
| dense | native / multi-aligned / exact | 35.87 | 63.75 | 85.89 | 70.35 | - | 28.66 | 121/5 | 2168859 | 26.87 |
| dense | native / wse-only / page16k | 10.24 | 18.03 | 18.55 | 18.13 | - | 21.97 | 17/5 | 916427 | 23.04 |
| dense | native / multi-raw / page16k | 15.11 | 23.31 | 42.74 | 41.66 | - | 25.59 | 25/5 | 2478102 | 24.25 |
| dense | native / multi-aligned / page16k | 15.14 | 43.40 | 86.90 | 85.99 | - | 28.23 | 25/5 | 2478102 | 27.83 |
| dense | h5py / multi-quality / 0 | 101.34 | complete slice | 148.28 | 315.48 | 126.50 | 65.55 | 55/2 | 2172081 | - |
| dense | h5py / multi-quality / 16384 | 85.43 | complete slice | 125.49 | 305.08 | 116.89 | 67.81 | 25/2 | 2509142 | - |

## conditional_loopback, 20 ms added response delay

| Selection | Reader / operation / policy | Prep ms | First ms | Total API ms | Process CPU ms | API CPU ms | RSS MiB | GET/HEAD | Bytes | Reservation MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| window | native / wse-only / exact | 2060.63 | 2134.58 | 2168.22 | 54.33 | - | 22.33 | 73/5 | 693244 | 21.40 |
| window | native / multi-raw / exact | 3383.07 | 3451.79 | 3638.68 | 140.67 | - | 28.16 | 121/5 | 2168859 | 22.16 |
| window | native / multi-aligned / exact | 3368.54 | 3605.31 | 3637.39 | 156.57 | - | 28.91 | 121/5 | 2168859 | 26.29 |
| window | native / wse-only / page16k | 574.83 | 651.51 | 675.28 | 54.60 | - | 22.30 | 17/5 | 916427 | 23.03 |
| window | native / multi-raw / page16k | 689.52 | 760.90 | 944.68 | 91.60 | - | 27.52 | 25/5 | 2478102 | 24.23 |
| window | native / multi-aligned / page16k | 652.23 | 877.97 | 910.75 | 87.49 | - | 28.36 | 25/5 | 2478102 | 27.41 |
| window | h5py / multi-quality / 0 | 1327.27 | complete slice | 1892.47 | 423.20 | 230.21 | 64.94 | 55/2 | 2172081 | - |
| window | h5py / multi-quality / 16384 | 579.19 | complete slice | 941.63 | 365.29 | 160.96 | 69.47 | 25/2 | 2509142 | - |
| dense | native / wse-only / exact | 2110.67 | 2181.07 | 2210.50 | 72.84 | - | 22.89 | 73/5 | 693244 | 21.41 |
| dense | native / multi-raw / exact | 3371.36 | 3442.31 | 3627.13 | 148.38 | - | 27.42 | 121/5 | 2168859 | 22.18 |
| dense | native / multi-aligned / exact | 3371.01 | 3581.10 | 3634.44 | 153.79 | - | 30.47 | 121/5 | 2168859 | 26.87 |
| dense | native / wse-only / page16k | 534.62 | 606.59 | 632.11 | 32.75 | - | 22.73 | 17/5 | 916427 | 23.04 |
| dense | native / multi-raw / page16k | 646.25 | 723.36 | 891.76 | 95.71 | - | 28.23 | 25/5 | 2478102 | 24.25 |
| dense | native / multi-aligned / page16k | 687.44 | 929.87 | 1017.87 | 157.03 | - | 30.39 | 25/5 | 2478102 | 27.83 |
| dense | h5py / multi-quality / 0 | 1299.33 | complete slice | 1901.02 | 476.20 | 269.96 | 67.52 | 55/2 | 2172081 | - |
| dense | h5py / multi-quality / 16384 | 615.55 | complete slice | 976.14 | 394.44 | 194.18 | 70.28 | 25/2 | 2509142 | - |

## local_sparse_reference, 0 ms added response delay

| Selection | Reader / operation / policy | Prep ms | First ms | Total API ms | Process CPU ms | API CPU ms | RSS MiB | GET/HEAD | Bytes | Reservation MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| window | h5py / multi-quality / 0 | 1.55 | complete slice | 29.69 | 199.90 | 29.70 | 53.31 | - | not instrumented | - |
| window | netcdf4 / multi-quality / 0 | 5.62 | complete slice | 34.18 | 125.62 | 34.18 | 68.16 | - | not instrumented | - |
| window | native / multi-aligned / 0 | 5.81 | 27.93 | 28.30 | 34.95 | - | 22.72 | 0/0 | not instrumented | 26.29 |
| dense | h5py / multi-quality / 0 | 1.50 | complete slice | 30.06 | 203.61 | 30.06 | 53.14 | - | not instrumented | - |
| dense | netcdf4 / multi-quality / 0 | 5.71 | complete slice | 35.33 | 124.85 | 35.33 | 69.86 | - | not instrumented | - |
| dense | native / multi-aligned / 0 | 5.66 | 27.32 | 49.69 | 57.19 | - | 24.25 | 0/0 | not instrumented | 26.87 |

## Default exact traffic for the 128 x 128 window

| Operation | GET/HEAD | Total bytes | Duplicate bytes | Coordinates bytes / GET | Metadata and indexes bytes / GET |
|---|---:|---:|---:|---:|---:|
| wse-only | 73/5 | 693244 | 418 | 1728 / 2 | 22577 / 70 |
| multi-raw | 121/5 | 2168859 | 588 | 1728 / 2 | 34821 / 114 |
| multi-aligned | 121/5 | 2168859 | 588 | 1728 / 2 | 34821 / 114 |

Metadata/index phases include their actual physical bytes. Paged metadata can overlap payload ranges, so phase totals, unique bytes and duplicate bytes have different meanings. Coordinate reads occur once per axis across five variables. Cache eviction may require repeat payload reads; encoded cache hits and repeated decompression are reported separately. Working reservations are not RSS caps. Mature reader validation scopes differ; these results do not establish a universal performance lead.
