# Matched selection performance

Medians of three separate processes per cell; seconds. Added delay is per HTTP response, including HEAD. Local-file results are separate in `local.json`.

| Delay | Selection | Strategy | Prepare | First values | Complete | CPU | Peak RSS MiB |
|---:|---|---|---:|---:|---:|---:|---:|
| 0 ms | sparse | A full/exact | 0.0230 | 0.0244 | 0.0249 | 0.0172 | 14.84 |
| 0 ms | sparse | B selective/exact | 0.0172 | 0.0184 | 0.0189 | 0.0150 | 14.61 |
| 0 ms | sparse | C full/16 KiB | 0.0100 | 0.0111 | 0.0115 | 0.0110 | 14.67 |
| 0 ms | sparse | C selective/16 KiB | 0.0083 | 0.0096 | 0.0100 | 0.0129 | 14.45 |
| 0 ms | sparse | D h5py/64 KiB | 0.0131 | 0.0153 | 0.0164 | 0.2874 | 57.14 |
| 0 ms | sparse | D h5py/256 KiB | 0.0115 | 0.0129 | 0.0136 | 0.2390 | 57.64 |
| 0 ms | dense | A full/exact | 0.0237 | 0.0254 | 0.0374 | 0.0414 | 18.83 |
| 0 ms | dense | B selective/exact | 0.0183 | 0.0199 | 0.0317 | 0.0392 | 18.39 |
| 0 ms | dense | C full/16 KiB | 0.0116 | 0.0131 | 0.0314 | 0.0348 | 18.44 |
| 0 ms | dense | C selective/16 KiB | 0.0095 | 0.0110 | 0.0252 | 0.0359 | 18.03 |
| 0 ms | dense | D h5py/64 KiB | 0.0145 | 0.0508 | 0.0538 | 0.3197 | 71.48 |
| 0 ms | dense | D h5py/256 KiB | 0.0119 | 0.0439 | 0.0463 | 0.2681 | 72.75 |
| 0 ms | noncontiguous | A full/exact | 0.0259 | 0.0265 | 0.0501 | 0.0378 | 17.45 |
| 0 ms | noncontiguous | B selective/exact | 0.0214 | 0.0220 | 0.0451 | 0.0353 | 17.14 |
| 0 ms | noncontiguous | C full/16 KiB | 0.0113 | 0.0117 | 0.0211 | 0.0282 | 16.95 |
| 0 ms | noncontiguous | C selective/16 KiB | 0.0100 | 0.0105 | 0.0335 | 0.0314 | 16.77 |
| 0 ms | noncontiguous | D h5py/64 KiB | 0.0142 | 0.0556 | 0.0565 | 0.3179 | 61.39 |
| 0 ms | noncontiguous | D h5py/256 KiB | 0.0119 | 0.0455 | 0.0461 | 0.2667 | 62.56 |
| 20 ms | sparse | A full/exact | 1.9263 | 1.9735 | 1.9974 | 0.0347 | 14.78 |
| 20 ms | sparse | B selective/exact | 1.2496 | 1.3014 | 1.3259 | 0.0270 | 14.58 |
| 20 ms | sparse | C full/16 KiB | 0.6746 | 0.7336 | 0.7657 | 0.0328 | 14.75 |
| 20 ms | sparse | C selective/16 KiB | 0.2478 | 0.2952 | 0.3212 | 0.0164 | 14.41 |
| 20 ms | sparse | D h5py/64 KiB | 0.1541 | 0.2011 | 0.2290 | 0.2905 | 56.89 |
| 20 ms | sparse | D h5py/256 KiB | 0.1435 | 0.1726 | 0.2044 | 0.2630 | 57.59 |
| 20 ms | dense | A full/exact | 1.9030 | 1.9545 | 2.1448 | 0.0821 | 18.28 |
| 20 ms | dense | B selective/exact | 1.3342 | 1.3837 | 1.5618 | 0.0761 | 18.05 |
| 20 ms | dense | C full/16 KiB | 0.7009 | 0.7651 | 0.9817 | 0.0854 | 19.34 |
| 20 ms | dense | C selective/16 KiB | 0.2984 | 0.3504 | 0.5355 | 0.0610 | 18.75 |
| 20 ms | dense | D h5py/64 KiB | 0.1539 | 0.8890 | 0.9196 | 0.3610 | 73.77 |
| 20 ms | dense | D h5py/256 KiB | 0.1590 | 0.9240 | 0.9661 | 0.3452 | 75.62 |
| 20 ms | noncontiguous | A full/exact | 2.1633 | 2.1871 | 2.5632 | 0.0792 | 17.09 |
| 20 ms | noncontiguous | B selective/exact | 1.7007 | 1.7258 | 2.0984 | 0.0732 | 16.66 |
| 20 ms | noncontiguous | C full/16 KiB | 0.6538 | 0.6814 | 1.1289 | 0.0661 | 16.81 |
| 20 ms | noncontiguous | C selective/16 KiB | 0.3740 | 0.3994 | 0.7819 | 0.0629 | 16.44 |
| 20 ms | noncontiguous | D h5py/64 KiB | 0.1566 | 1.0755 | 1.1025 | 0.3635 | 61.66 |
| 20 ms | noncontiguous | D h5py/256 KiB | 0.1445 | 1.0284 | 1.0603 | 0.3614 | 63.59 |
| 80 ms | sparse | A full/exact | 6.6384 | 6.8114 | 6.8929 | 0.0341 | 14.72 |
| 80 ms | sparse | B selective/exact | 4.4576 | 4.6372 | 4.7215 | 0.0296 | 14.47 |
| 80 ms | sparse | C full/16 KiB | 2.1488 | 2.3252 | 2.4165 | 0.0338 | 14.69 |
| 80 ms | sparse | C selective/16 KiB | 0.8647 | 1.0399 | 1.1289 | 0.0195 | 14.28 |
| 80 ms | sparse | D h5py/64 KiB | 0.5423 | 0.7248 | 0.8150 | 0.2958 | 57.14 |
| 80 ms | sparse | D h5py/256 KiB | 0.4723 | 0.5698 | 0.6612 | 0.2978 | 58.16 |
| 80 ms | dense | A full/exact | 6.9253 | 7.1005 | 7.7345 | 0.0894 | 18.14 |
| 80 ms | dense | B selective/exact | 4.7991 | 4.9703 | 5.5978 | 0.0800 | 18.17 |
| 80 ms | dense | C full/16 KiB | 2.1185 | 2.2883 | 2.9283 | 0.0929 | 18.64 |
| 80 ms | dense | C selective/16 KiB | 1.0510 | 1.2335 | 1.8683 | 0.0665 | 17.92 |
| 80 ms | dense | D h5py/64 KiB | 0.5427 | 3.1046 | 3.1993 | 0.3452 | 72.25 |
| 80 ms | dense | D h5py/256 KiB | 0.4572 | 2.7305 | 2.8278 | 0.3919 | 73.39 |
| 80 ms | noncontiguous | A full/exact | 7.7936 | 7.8793 | 9.2504 | 0.1624 | 16.89 |
| 80 ms | noncontiguous | B selective/exact | 6.2279 | 6.3162 | 7.7038 | 0.1565 | 17.81 |
| 80 ms | noncontiguous | C full/16 KiB | 2.1583 | 2.2476 | 3.6122 | 0.1125 | 17.25 |
| 80 ms | noncontiguous | C selective/16 KiB | 1.3544 | 1.4422 | 2.8188 | 0.1115 | 17.62 |
| 80 ms | noncontiguous | D h5py/64 KiB | 0.5696 | 3.8430 | 3.9349 | 0.4141 | 61.70 |
| 80 ms | noncontiguous | D h5py/256 KiB | 0.4706 | 3.1701 | 3.2619 | 0.3821 | 63.06 |

`First values` is first provisional native chunk versus completion of the one-shot h5py slice. The latter materializes the whole selection. CPU is whole child-process CPU, including Python startup/imports; h5py API-only CPU and whole-process elapsed time are also retained. Timed acquisition wall clocks exclude interpreter imports and request-file parsing.

Native transport uses five HEADs; the h5py range control uses two. Both pin exact ranges and revalidate ETag/size at completion. Native reservations include its prepared metadata and bounded scheduler; h5py uses a 2 MiB chunk cache and a bounded 2 MiB range-page cache, plus the selected output array. RSS includes the respective runtimes. Native provisional finish, cancellation and whole-index/full versus selected validation scope are not equivalent to returning one h5py array.

The experiment adds delay on loopback without a bandwidth throttle, TLS or WAN variation. It supports neither a general performance-leadership claim nor an optimal-page-size claim outside the tested choices. `mature-io.json` partitions h5py ranges into primary-science payload and metadata/other bytes. Mixed GETs are explicit. Native metadata byte counts are phase-based and include page overfetch, which can contain nearby payload bytes.
