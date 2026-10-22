# SWOT subset performance and physical reads

Three fresh reader processes per cell. Delays apply to each loopback HEAD/GET; there is no bandwidth throttle or provider traffic. CPU includes child process startup; mature API CPU is retained separately. Native first-result time is first provisional chunk; mature first-result time is a complete slice. Native pre/post identity checks use more HEADs. Native emits chunk coordinates/metadata; controls return array hashes and summaries. Working reservations exclude runtime and caller retention and are not RSS caps.

## loopback_conditional_ranges; added delay 0 ms

| Selection | Reader / policy | Prep ms | First ms | Total ms | CPU ms | Peak RSS MiB | GET/HEAD | Bytes | Reservation MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| window | native full/exact | 17.61 | 24.43 | 24.90 | 21.62 | 21.88 | 73/5 | 706984 | 21.41 |
| window | native full/page4k | 9.59 | 16.49 | 16.87 | 17.31 | 21.59 | 26/5 | 803193 | 22.08 |
| window | native full/page16k | 10.06 | 17.60 | 18.08 | 16.70 | 21.72 | 17/5 | 946553 | 23.17 |
| window | native selective/exact | 20.61 | 28.27 | 28.83 | 20.06 | 21.92 | 73/5 | 706984 | 21.41 |
| window | native selective/page4k | 10.49 | 17.90 | 18.44 | 17.35 | 21.64 | 26/5 | 803193 | 22.08 |
| window | native selective/page16k | 10.89 | 18.41 | 18.89 | 16.68 | 21.67 | 17/5 | 946553 | 23.17 |
| window | h5py 0 | 36.29 | 45.19 | 45.97 | 265.16 | 63.88 | 54/2 | 717300 | — |
| window | h5py 16384 | 16.86 | 25.39 | 26.14 | 261.04 | 65.53 | 15/2 | 944825 | — |
| boundary | native full/exact | 17.76 | 24.75 | 44.51 | 39.31 | 23.73 | 76/5 | 3214940 | 22.43 |
| boundary | native full/page4k | 9.22 | 16.00 | 38.36 | 36.87 | 25.31 | 29/5 | 3311149 | 23.11 |
| boundary | native full/page16k | 10.13 | 17.47 | 37.86 | 35.55 | 25.62 | 20/5 | 3454509 | 24.19 |
| boundary | native selective/exact | 20.19 | 27.33 | 47.55 | 39.64 | 26.59 | 76/5 | 3214940 | 22.43 |
| boundary | native selective/page4k | 11.59 | 18.44 | 37.82 | 35.76 | 24.09 | 29/5 | 3311149 | 23.11 |
| boundary | native selective/page16k | 9.54 | 16.40 | 36.87 | 34.69 | 24.28 | 20/5 | 3454509 | 24.19 |
| boundary | h5py 0 | 36.83 | 71.61 | 72.33 | 285.83 | 72.23 | 57/2 | 3225256 | — |
| boundary | h5py 16384 | 18.80 | 52.21 | 52.94 | 276.76 | 74.66 | 18/2 | 3452781 | — |
| strided | native full/exact | 19.29 | 26.35 | 46.14 | 41.26 | 26.52 | 76/5 | 3214940 | 22.43 |
| strided | native full/page4k | 11.05 | 18.11 | 38.72 | 35.69 | 23.53 | 29/5 | 3311149 | 23.11 |
| strided | native full/page16k | 9.21 | 16.44 | 36.54 | 35.88 | 23.73 | 20/5 | 3454509 | 24.19 |
| strided | native selective/exact | 16.90 | 23.92 | 44.17 | 38.78 | 23.83 | 76/5 | 3214940 | 22.43 |
| strided | native selective/page4k | 10.84 | 18.06 | 39.15 | 35.47 | 25.00 | 29/5 | 3311149 | 23.11 |
| strided | native selective/page16k | 9.66 | 16.63 | 36.84 | 37.06 | 24.42 | 20/5 | 3454509 | 24.19 |
| strided | h5py 0 | 36.88 | 70.79 | 71.49 | 296.63 | 71.17 | 57/2 | 3225256 | — |
| strided | h5py 16384 | 17.33 | 50.89 | 51.55 | 277.60 | 73.45 | 18/2 | 3452781 | — |

## loopback_conditional_ranges; added delay 20 ms

| Selection | Reader / policy | Prep ms | First ms | Total ms | CPU ms | Peak RSS MiB | GET/HEAD | Bytes | Reservation MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| window | native full/exact | 2198.37 | 2281.46 | 2311.03 | 73.68 | 22.22 | 73/5 | 706984 | 21.41 |
| window | native full/page4k | 888.52 | 973.45 | 1005.40 | 56.08 | 21.73 | 26/5 | 803193 | 22.08 |
| window | native full/page16k | 600.64 | 683.03 | 724.47 | 70.80 | 21.98 | 17/5 | 946553 | 23.17 |
| window | native selective/exact | 2285.23 | 2360.42 | 2385.03 | 93.86 | 22.31 | 73/5 | 706984 | 21.41 |
| window | native selective/page4k | 864.82 | 954.95 | 987.78 | 67.76 | 21.97 | 26/5 | 803193 | 22.08 |
| window | native selective/page16k | 987.44 | 1052.95 | 1084.35 | 68.63 | 22.62 | 17/5 | 946553 | 23.17 |
| window | h5py 0 | 2329.24 | 2415.30 | 2445.73 | 1108.71 | 64.34 | 54/2 | 717300 | — |
| window | h5py 16384 | 558.91 | 620.70 | 651.52 | 979.73 | 66.14 | 15/2 | 944825 | — |
| boundary | native full/exact | 2694.77 | 2761.05 | 2900.56 | 135.00 | 26.47 | 76/5 | 3214940 | 22.43 |
| boundary | native full/page4k | 815.60 | 888.03 | 1030.82 | 80.68 | 26.30 | 29/5 | 3311149 | 23.11 |
| boundary | native full/page16k | 594.29 | 668.40 | 812.39 | 82.58 | 25.94 | 20/5 | 3454509 | 24.19 |
| boundary | native selective/exact | 2338.70 | 2433.28 | 2659.48 | 121.71 | 25.53 | 76/5 | 3214940 | 22.43 |
| boundary | native selective/page4k | 938.08 | 1063.90 | 1264.34 | 109.06 | 26.19 | 29/5 | 3311149 | 23.11 |
| boundary | native selective/page16k | 619.01 | 700.56 | 853.91 | 108.21 | 24.62 | 20/5 | 3454509 | 24.19 |
| boundary | h5py 0 | 1808.65 | 2028.38 | 2055.09 | 1058.63 | 73.08 | 57/2 | 3225256 | — |
| boundary | h5py 16384 | 530.53 | 715.09 | 746.81 | 1020.46 | 77.16 | 18/2 | 3452781 | — |
| strided | native full/exact | 2270.55 | 2373.66 | 3211.85 | 119.23 | 25.70 | 76/5 | 3214940 | 22.43 |
| strided | native full/page4k | 901.33 | 974.33 | 1139.08 | 115.33 | 25.38 | 29/5 | 3311149 | 23.11 |
| strided | native full/page16k | 586.98 | 663.74 | 836.45 | 103.52 | 28.23 | 20/5 | 3454509 | 24.19 |
| strided | native selective/exact | 2241.61 | 2316.28 | 2469.75 | 138.69 | 25.81 | 76/5 | 3214940 | 22.43 |
| strided | native selective/page4k | 881.04 | 961.70 | 1161.53 | 87.44 | 26.64 | 29/5 | 3311149 | 23.11 |
| strided | native selective/page16k | 646.24 | 710.92 | 980.13 | 109.35 | 27.20 | 20/5 | 3454509 | 24.19 |
| strided | h5py 0 | 1907.18 | 2197.01 | 2234.54 | 1087.95 | 75.70 | 57/2 | 3225256 | — |
| strided | h5py 16384 | 760.53 | 1485.47 | 1516.98 | 678.70 | 79.06 | 18/2 | 3452781 | — |

## local_sparse_file; added delay 0 ms

| Selection | Reader / policy | Prep ms | First ms | Total ms | CPU ms | Peak RSS MiB | GET/HEAD | Bytes | Reservation MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| window | native full/exact | 2.35 | 8.85 | 9.08 | 15.85 | 18.69 | — | not instrumented | 21.41 |
| window | h5py 0 | 2.48 | 9.95 | 10.15 | 270.85 | 61.86 | — | not instrumented | — |
| window | netcdf4 0 | 6.48 | 14.10 | 14.28 | 111.92 | 58.02 | — | not instrumented | — |
| boundary | native full/exact | 2.04 | 7.79 | 23.46 | 28.95 | 19.19 | — | not instrumented | 22.43 |
| boundary | h5py 0 | 2.13 | 28.87 | 28.99 | 252.08 | 68.44 | — | not instrumented | — |
| boundary | netcdf4 0 | 5.74 | 32.85 | 32.93 | 116.87 | 66.78 | — | not instrumented | — |
| strided | native full/exact | 2.10 | 7.92 | 24.59 | 30.50 | 19.31 | — | not instrumented | 22.43 |
| strided | h5py 0 | 2.19 | 29.29 | 29.41 | 266.77 | 68.39 | — | not instrumented | — |
| strided | netcdf4 0 | 5.73 | 33.19 | 33.26 | 117.94 | 66.86 | — | not instrumented | — |

## Byte accounting

Native phase bytes separate explicit coordinate chunk reads, WSE payload reads and metadata/index/page reads. Metadata pages can overlap payloads. Duplicate-byte counts include all physical overlap, even nonidentical ranges. Extra unique bytes compare a paged policy with the same reader's exact union; they do not equate native and mature validation scopes. All source coordinate chunks in this file hold 1580 samples, so 25,280 bytes of axes must be decoded even for a few requested coordinates. Each WSE chunk decodes 2,496,400 bytes. See acquisition.json and performance-summary.json for per-cell overfetch and range counts.

netCDF4/NetCDF-C is a local sparse-file reference. Its kernel read ranges are not instrumented, so no remote byte or latency equivalence is claimed. h5py exact/16 KiB controls use bounded conditional mirror ranges with pre/post ETag/length checks and a bounded 2 MiB reference page cache. Native uses full index traversal by default; experimental selective traversal cannot validate unread subtrees. The three relevant indexes here are flat, so both strategies visit all three nodes.
