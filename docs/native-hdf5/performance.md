# Matched HDF5 acquisition measurements

Frozen synthetic source: 1,610,157 bytes, UInt16, shape 768 × 1024, 64 × 64 gzip chunks. Sparse selection: 16 samples across 16 chunks. Dense selection: 196,608 samples across 48 chunks. Every native value, validity bit and coordinate matches h5py and netCDF4. Independent h5py workers read the same selected axes and preserve scientific attributes.

Medians of three alternating native-policy repetitions. One native worker and one independent h5py process; 40 ms is controlled per-request latency, with unconstrained loopback bandwidth. Times include initial metadata/coordinates, payloads and source validation. Native metadata timing includes index discovery; h5py defers some index work until the value read. Process startup/imports are outside the internal acquisition timer. The native timed verification consumer dumps per-chunk bytes/masks; Python returns an array and hashes it. Low-latency and CPU comparisons include this consumer-work difference; native policy comparisons keep that work fixed. Retrieval/decode stages are separately retained. CPU/RSS scopes differ explicitly in raw receipts.

## 0 ms added request latency

| Selection | Reader | Completion (s) | GET / HEAD | Response bytes | Max RSS (MiB) |
| --- | --- | ---: | ---: | ---: | ---: |
| sparse | individual | 0.0209 | 63 / 5 | 142,391 | 14.4 |
| sparse | adjacent | 0.0299 | 63 / 5 | 142,391 | 14.5 |
| sparse | bounded | 0.0263 | 63 / 5 | 142,391 | 14.5 |
| sparse | h5py-0 | 0.0357 | 47 / 2 | 151,967 | 56.7 |
| sparse | h5py-4096 | 0.0293 | 26 / 2 | 188,519 | 56.9 |
| sparse | h5py-65536 | 0.0197 | 14 / 2 | 1,114,112 | 57.7 |
| sparse | full-file | 0.0154 | 1 / 2 | 1,610,157 | 60.0 |
| dense | individual | 0.0330 | 89 / 5 | 411,875 | 17.0 |
| dense | adjacent | 0.0341 | 47 / 5 | 411,875 | 16.7 |
| dense | bounded | 0.0331 | 47 / 5 | 411,875 | 16.7 |
| dense | h5py-0 | 0.0530 | 73 / 2 | 421,451 | 57.7 |
| dense | h5py-4096 | 0.0437 | 57 / 2 | 454,995 | 57.8 |
| dense | h5py-65536 | 0.0244 | 19 / 2 | 1,638,400 | 59.8 |
| dense | full-file | 0.0142 | 1 / 2 | 1,610,157 | 61.7 |

## 40 ms added request latency

| Selection | Reader | Completion (s) | GET / HEAD | Response bytes | Max RSS (MiB) |
| --- | --- | ---: | ---: | ---: | ---: |
| sparse | individual | 3.0252 | 63 / 5 | 142,391 | 14.6 |
| sparse | adjacent | 3.0150 | 63 / 5 | 142,391 | 14.6 |
| sparse | bounded | 3.0115 | 63 / 5 | 142,391 | 14.5 |
| sparse | h5py-0 | 2.2358 | 47 / 2 | 151,967 | 56.7 |
| sparse | h5py-4096 | 1.2877 | 26 / 2 | 188,519 | 56.5 |
| sparse | h5py-65536 | 0.7479 | 14 / 2 | 1,114,112 | 57.5 |
| sparse | full-file | 0.1451 | 1 / 2 | 1,610,157 | 59.7 |
| dense | individual | 4.1669 | 89 / 5 | 411,875 | 16.9 |
| dense | adjacent | 2.3015 | 47 / 5 | 411,875 | 17.0 |
| dense | bounded | 2.3261 | 47 / 5 | 411,875 | 17.1 |
| dense | h5py-0 | 3.4276 | 73 / 2 | 421,451 | 57.7 |
| dense | h5py-4096 | 2.6758 | 57 / 2 | 454,995 | 58.6 |
| dense | h5py-65536 | 0.9720 | 19 / 2 | 1,638,400 | 59.9 |
| dense | full-file | 0.1475 | 1 / 2 | 1,610,157 | 61.3 |

Adjacent coalescing improves the dense 40 ms case by 1.81× over native individual reads, with zero payload gap overfetch. Exact native payload/index range repeats are absent; small overlapping header-prefix reads remain visible in byte-union coverage. The bounded-gap control in resources.json reduces eight payload GETs to one with 7,680 overfetch bytes (11.71% of selected compressed bytes).

Tuned h5py remains faster in the high-latency cases. Its larger windows trade extra bytes for fewer requests; the 64 KiB strategy can cover most or more than one source-file length after overlapping reads. The explicit full-file control is fastest on this small source and fast link. That control is a useful alternative when transfer cost, memory and file size permit; it is never a native fallback.

Native index discovery scans all v1 B-tree leaves. This metadata cost, small signature/header reads, and repeated parsing of cached index bytes limit latency and dense decode efficiency. The priority is established-reader index pruning and metadata locality. These measurements support a useful selective payload profile and do not establish general throughput or memory leadership.

summary.json retains metadata/first-value/CPU/RSS/reservation medians and byte-union accounting; benchmark.json retains all cells and transport phases. resources.json retains bounded-gap, memory, corruption and index-discovery controls.
