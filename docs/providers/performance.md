# Bounded provider measurements

These are two exact pinned observations on one local network. Times do not establish global speed or service reliability. Wall includes process startup, signing/authentication, metadata/version inspection, native acquisition and durable output. First output is measured from the execution stage, after preparation. Encoded data bytes exclude metadata and access bodies. Full fields and counter scopes are in the JSON receipts.

| Product/provider | Wall s | First output in execution s | CPU s | RSS bytes | Native data bytes | Peak working reservation bytes | Output bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| hls-l30 / planetary_computer | 5.131 | 0.934 | 0.084 | 25493504 | 90612 | 506878 | 223060 |
| hls-l30 / earthdata | 24.243 | 5.096 | 0.157 | 25968640 | 90612 | 2604030 | 227794 |
| hls-s30 / planetary_computer | 4.624 | 1.045 | 0.077 | 25772032 | 108167 | 526272 | 227236 |
| hls-s30 / earthdata | 19.869 | 4.587 | 0.122 | 26214400 | 108167 | 2623424 | 231978 |

Controlled 5-ms/100-Mbit/s mirror wall times were 0.167–0.191s for L30 and 0.168–0.177s for S30 across two interleaved rounds, with identical complete stored values, masks and coordinates. Access overhead is excluded from that mirror comparison. Native dependencies are unchanged in Cargo.lock; Python rasterio 1.4.4/GDAL 3.10.3 provide the independent product oracle.

The first live signer attempt failed HTTP 504 and stopped. A subsequent development failure was the incorrect blob-only token validator, not a provider outage; the observed container read/list grant is now admitted under exact-object request confinement. The final four runs passed. One separate metadata/signing inspection returned a non-JSON response; its precise endpoint/status was not captured, so it is not converted into a service failure rate. No statistical reliability rate is claimed.

[Live receipts](live-performance.json), [controlled receipts](mirror-performance.json), [independent science and access overhead](equivalence.json), [catalog coverage and timing](discovery-validation.json).
