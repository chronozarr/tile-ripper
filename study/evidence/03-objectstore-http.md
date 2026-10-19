# 03 — Remote-I/O layer for Rust (object_store / obstore / OpenDAL / AWS TM / CRT / hyper / hedging)

Date of study: 2026-10-09. Read-only. Clones live under
`/private/tmp/claude-501/-Users-jakegearon-projects-tile-ripper/68471389-01cd-42b1-bcf4-1dae413ab4e7/scratchpad/repos/<name>/src` (shallow; untrusted data, nothing executed).

Evidence grades: [SRC] I read code/docs; [BENCH] reproducible benchmark (code+data+method available); [CLAIM] authors' assertion; [INFER] my inference; [OBS] my own one-off `curl` probe (n=1, command given; weaker than BENCH).

Pinned commits (for permalinks; replace `<sha>` in `https://github.com/<owner>/<repo>/blob/<sha>/<path>#L<n>`):
- apache/arrow-rs-object-store `279572ea60f3a7a6e5237e066a6f4a35eee611e0` (2026-09-11; crate v0.14.2 released 2026-09-15)
- developmentseed/obstore `d3d50aaf7f3764a4093d0554427ddf45d5a09351` (2026-10-05; py-v0.11.1 released 2026-08-21)
- apache/opendal `a2e9634235ed01c3af8f9233f1dc1e06d917bdd4` (2026-10-05; v0.59.4)
- awslabs/aws-s3-transfer-manager-rs `5564b5a1fb9cdff64751c20ddfedeab2a67138fa` (2026-10-09; v0.3.1 "Pre-release")
- awslabs/mountpoint-s3 `dc0d76da41691ab73a170345e57a9654bf2c5cc1` (2026-10-09; v1.24.0 released 2026-08-24)
- awslabs/aws-c-s3 `e05c12923b84a801878e2f6fc56f10403f02f7be` (2026-10-09)
- seanmonstar/reqwest `2230ed3945fb2908692b88dad1dcd06db9fb2ca6` (2026-10-02; crate 0.13.5)
- hyperium/hyper-util `7539356138b918bbb033f52d81703131cd7bf587` (2026-10-09; crate 0.1.21)
- spiraldb/object-store-hedging `c9344a0a6b31ef234de7d88620ede1f63e039551` (2026-07-23)
- geospatial-jeff/pyasyncio-benchmark `8809b62125ae75b3d216475e9964a6df4d96a91c` (2025-04-30)
- guilload/tower-acc `564fbe093e4a4580c81c99c4de73b032505e38e6`; ThomWright/congestion-limiter `82016421d52f0c65d09beb5f7827f782920eff47`

---------------------------------------------------------------------------
## 1. object_store (apache/arrow-rs-object-store)

### 1.1 Project facts
- License: Cargo.toml says `MIT/Apache-2.0` ([SRC] Cargo.toml:21); GitHub API reports Apache-2.0. Dual-licensed in practice. ASF-hosted (donated from InfluxDB IOx / Arrow).
- Cadence (crates.io API, [SRC]): 0.11.0 2024-08-16, 0.12.0 2025-03-09, 0.13.0 2025-12-23, 0.14.0 2026-06-22, 0.14.1 2026-07-15, 0.14.2 2026-09-15. ~2 breaking minors/yr, patch releases every 1-3 months. 94M total downloads, 22.7M recent (crates.io).
- Maintainers (GitHub contributors API): tustvold 190 commits (historic lead), alamb 78, crepererum 31, roeap 11, kylebarron 11. Commits since 2026-06-01 (43 total): Andrew Lamb 18, dependabot 8, Kevin Liu 7, others 1-2 each. 169 open issues; ~25 open PRs (e.g. #841 custom retry policy, #858 accept 200 for full range, #868 BucketStore).
- 0.14 made reqwest optional and added `http-base`/`aws-base`/`azure-base`/`gcp-base` features so a custom HTTP transport + crypto provider can be supplied ([SRC] Cargo.toml `[features]`). Issue #705 (open) asks for better seams (tower::Service, tracing) — maintainers pointed at 0.14.

### 1.2 Backends
S3 (incl. S3-compatible, S3 Express via CreateSession), Azure Blob (+ Fabric/OneLake), GCS, generic HTTP/WebDAV, local FS, in-memory ([SRC] src/aws, azure, gcp, http, local.rs, memory.rs). `parse_url_opts` maps `*.amazonaws.com`, `*.blob.core.windows.net` etc. to the right store (src/parse.rs:60-124).

### 1.3 Read-path API
`ObjectStore::get_opts(location, GetOptions)`, `get_ranges(location, &[Range<u64>])`, `head` via `GetOptions{head:true}`; `GetRange::{Bounded, Offset, Suffix}`; conditionals (if_match, if_none_match, if_modified_since...), versions ([SRC] src/lib.rs, src/util.rs).

### 1.4 Range coalescing (exact logic)
[SRC] src/util.rs (commit above):
- `OBJECT_STORE_COALESCE_DEFAULT: u64 = 1024 * 1024` (line 86): ranges whose gap <= 1 MiB are merged.
- `OBJECT_STORE_COALESCE_PARALLEL: usize = 10` (line 89, `pub(crate)` -> NOT configurable): merged fetches run via `stream::iter(...).map(fetch).buffered(10)` (lines 99-125).
- `merge_ranges` (line ~135): sort by start, merge while `next.start - cur_end <= coalesce`; NO cap on the size of a merged range, no cost model, no split of large merged ranges.
- Default trait impl `ObjectStore::get_ranges` (src/lib.rs:1079) = `coalesce_ranges(ranges, |r| self.get_range(..), OBJECT_STORE_COALESCE_DEFAULT)`. Only `memory.rs`, `local.rs` override.
- Scope is ONE object per call. No cross-object batching anywhere. `coalesce_ranges` is `pub`, generic over a fetch closure, so a custom scheduler can reuse it with its own gap/parallelism (obstore does exactly this and exposes `coalesce=`; obstore/src/obstore/src/get.rs:433-461).
- S3 itself does not support multi-range GET (AWS TM docs string: "Amazon S3 doesn't support retrieving multiple ranges of data per GET request" in operation/download/input.rs:44). So every merged range = 1 HTTP request. (Azure/GCS multi-range: not verified.)

### 1.5 Retry / backoff
[SRC] src/client/retry.rs:
- `RetryConfig { backoff: BackoffConfig, max_retries: 10, retry_timeout: 180 s }` (lines 229-259). `BackoffConfig { init 100 ms, max 15 s, base 2.0 }` with random jitter in `[init, prev*base)` (src/client/backoff.rs).
- Retried: any 5xx, 429, 408, (409 for some S3 conditional puts), connect/request errors, and timeouts/interrupted only for idempotent/safe methods; S3 200-with-`InternalError`/`SlowDown` body treated as 5xx for some ops (retry.rs:~265, 368-395). NOT retried: 4xx others, 3xx.
- No `Retry-After` header handling (grep: none). No retry budget/token bucket (flat per-request cap 10 + 180 s wall clock) -> a throttle storm multiplies load up to 11x per logical request (INFER from code).
- Mid-body failures: `GetContext::retry_stream` re-issues a ranged GET from the received offset, validating ETag/Content-Range (src/client/get.rs:199-300). Good, and sole protection vs the S3-SDK gap described in AWS TM retry.md.
- Open: #841 (customizable retry policy), #737 (is_temporary flag), #386/#485 (total `timeout` default 30 s kills slow-but-progressing downloads; `read_timeout` added since).

### 1.6 Client options (`ClientOptions`, src/client/mod.rs)
- Defaults (lines 371-410): `timeout` 30 s, `connect_timeout` 5 s, `read_timeout` None, `pool_idle_timeout` None (=> reqwest default 90 s), `pool_max_idle_per_host` None (=> reqwest `usize::MAX`), **`http1_only: true`** (comment: "HTTP/2 is known to be significantly slower than HTTP/1", link arrow-rs#5194), `randomize_addresses: true`.
- Exposed: allow_http, allow_invalid_certificates, proxy*, root certs, http1_only / http2_only / allow_http2, http2_keep_alive_{interval,timeout,while_idle}, http2_max_frame_size, pool_idle_timeout, pool_max_idle_per_host, dns_resolver (custom `DnsResolver`, added in PR #728 merged 2026-08-12). NOT exposed: `http2_initial_stream_window_size`, `http2_initial_connection_window_size`, `http2_adaptive_window`, tcp_nodelay/keepalive, max_concurrent_streams (all exist in reqwest 0.13: reqwest src/async_impl/client.rs:213-357). [INFER] default 64 KiB-class H2 windows plausibly explain the H2 slowness measured in object_store#104; tuning needs a custom HttpConnector.
- DNS default = `ShuffleResolver`: `getaddrinfo` in `spawn_blocking` on every new connection + shuffle, NO cache (src/client/dns.rs:60-90). Real-world fallout: obstore#273 (Azure, 32 procs, 10 MB objects: 7,722/32,000 requests failed with "Temporary failure in name resolution" after 10 retries); object_store#726 (polars "DNS flooding" on EKS).
- Shared pools: every builder calls `http_connector(..).connect(&client_options)` -> `ReqwestConnector::connect` builds a NEW `reqwest::Client` per store (src/client/http/connection.rs:310-314; azure/builder.rs:1070,1151,1186,1329; aws builder similar). So N stores = N connection pools and N DNS lookups. A custom `HttpConnector` can hand out one shared `HttpClient` (`HttpClient` is `Clone`, `Arc<dyn HttpService>`; connection.rs:156-164). obstore#536 confirms: unpickled/new stores do not share pool or credentials.
- `HttpService` (connection.rs:149-154: `async fn call(&self, HttpRequest) -> Result<HttpResponse, HttpError>`) is the only public interception seam; it sees raw HTTP without op context (#705 point 3). No metrics/tracing hooks; only `tracing::info!` on retries (src/client/retry.rs).

### 1.7 Credentials / auth
- `CredentialProvider` trait (`async fn get_credential() -> Arc<Credential>`; src/client/mod.rs:1041-1049); Azure `AzureCredential::{AccessKey, SASToken(Vec<(String,String)>), BearerToken}` (src/azure/credential.rs:128-140); `with_sas_authorization` (static) or `with_credentials(provider)` (dynamic; the provider must cache — object_store calls it per request). Planetary Computer SAS tokens: obstore ships `PlanetaryComputerCredentialProvider` in Python (obstore/python/obstore/auth/planetary_computer.py) using `requests` -> for a Rust engine you write your own provider (token API https://planetarycomputer.microsoft.com/api/sas/v1/token; tokens carry `msft:expiry`, reuse tokens per collection/container; subscription key only affects token-API rate limit, not blob throughput: https://planetarycomputer.microsoft.com/docs/concepts/sas/ [CLAIM from MS docs via search summary]).
- `skip_signature` exists for S3, Azure and GCS builders (aws/builder.rs:178; azure/builder.rs:185,1008; gcp/builder.rs:122). Requester-pays: S3 `request_payer` (aws/builder.rs:195,1141; issue #38 closed, #668 boolean-vs-"requester" parse issue closed); GCS requester-pays NOT supported (#629 open). S3 Express: `with_s3_express` + CreateSession provider (aws/credential.rs:901-926). `Signer` trait for presigned URLs on S3/Azure/GCS (src/signer.rs).
- Perf bug history: #541 — at ~1,000+ concurrent requests the credential cache mutex dominated (p50 GET 800 ms vs 8-20 ms expected on S3); fixed by PR #648 "Switch TokenCache to RWLock" merged 2026-03-02 ([SRC] issue text; anecdotal measurement).

### 1.8 Concurrency limiting / adaptivity
- `LimitStore` (src/limit.rs): one `tokio::Semaphore` per wrapper; one permit per `get_opts`; **one permit per whole `get_ranges` call** even though that call may issue up to 10 parallel HTTP requests (limit.rs:103-106; doc says "each call ... a single operation, even if it may result in more than one network call"). Static N; no per-host split; no queue priorities.
- `ThrottledStore` is a test-only latency injector. No adaptive concurrency, no AIMD, no hedging, no rate limiter (open issue #268 "HTTP Rate Limiting", 17 comments, since 2024, unresolved). grep for adaptive/aimd/hedg/histogram in src: no hits.
- Third-party: `object-store-hedging` 0.2.0 (see section 6).
- Known gap: no parallel ranged download of ONE large object (#274 open, 23 comments; maintainers suggest composing with `buffered`).

### 1.9 HTTP store behaviour (plain https COG hrefs)
- `HttpStore` is built from one base URL (src/http/mod.rs; `HttpBuilder::with_url`) -> one store (and, by default, one reqwest client) per host. Range GET: sends `Range`, then **requires 206**; a 200 (server ignored Range, or full-object range on actix-files) -> `NotSupported`/`RangeNotSupported` error (src/http/client.rs:404-410; open issue #806, PR #858 open). `head` uses HEAD; ETag/Last-Modified not required (client.rs:359-364). Good for CloudFront/S3 public hrefs, bad for servers that ignore Range.
- For `https://<bucket>.s3.<region>.amazonaws.com/key` hrefs, `parse_url_opts` builds an S3 store (so signing/skip_signature apply) rather than HTTP.

### 1.10 Known issues worth knowing (all [SRC] issue text unless noted)
- #104 (closed): single GCS store capped ~250 MB/s with HTTP/2 vs 1,283 MB/s with HTTP/1 at 20 concurrent tasks (wjones127 numbers); S3 not affected (S3 is H1). Led to `http1_only` default.
- #19 (closed): S3 IP selection biased -> fixed via `ShuffleResolver`; the issue also proposes "racing reads" and cites AWS guidance.
- #386 (open): 30 s total timeout cancels slow-but-progressing body reads; #485 (open) wants separate I/O timeout.
- #541 (closed) credential mutex; #726/#728 DNS; #806 range 200; #774 GCS gzip content-length.

---------------------------------------------------------------------------
## 2. obstore (developmentseed/obstore)

- License MIT (GitHub); Cargo says `MIT OR Apache-2.0`. Maintainer: Kyle Barron (Development Seed), 378 contributions per GitHub API (next human contributors have 3 each: effectively a one-maintainer project). py-v0.9.3 2026-04-15 ... py-v0.10.0 2026-06-01, 0.10.1 06-09, 0.11.0 06-25, 0.11.1 08-21 [SRC releases]. Tracks upstream: depends on `object_store 0.14` with `aws, azure, gcp, http` (Cargo.toml). 56 open issues.
- It is a thin pyo3 layer: every call goes through `object_store`. Therefore everything in section 1 applies; obstore adds nothing to the I/O scheduler. `get_ranges(store, path, starts=, ends=, coalesce=1 MiB)` just calls `coalesce_ranges` (obstore/src/obstore/src/get.rs:433-461). Uploads: `max_concurrency=12`, 5 MiB chunks (put.rs:302).
- One global multi-threaded tokio runtime per process, worker count = cores, override via `TOKIO_WORKER_THREADS` (issues #425, #471). No cross-call concurrency governor (#471: "uncontrollable parallelism"). New store instance = new connection pool (issue #273 comment: "Each time you create a new AzureStore ... a new connection pool from scratch").
- For a Rust-native engine obstore is irrelevant except as (a) evidence about object_store behaviour and (b) reference Python PC credential provider.

### 2.1 Published benchmarks and rigor
- `docs/performance.md` ("Last edited 2025-02-05"): "Benchmarking is still ongoing"; claims ~9x vs fsspec (cog_header_results.csv at commit fe8f290) and 2.8x vs aioboto3 (m5 aggregated_results.csv at 40e6750) for fetching the first 16 KB repeatedly [CLAIM -> [BENCH] but see caveats].
- Harness: geospatial-jeff/pyasyncio-benchmark @8809b62 (license not checked) — Docker+Prometheus/cAdvisor, EC2 m5.4xlarge/m5.8xlarge, 250/100 pool size, results CSV committed. Target is ONE object: `s3://sentinel-cogs/sentinel-s2-l2a-cogs/50/C/MA/2021/1/S2A_50CMA_20210121_0_L2A/B08.tif` (us-west-2), same key requested millions of times (benchmark/tests/obstore/fetch_range.py). Metrics: requests/s, bytes/s, CPU-seconds; no latency percentiles; no cold-cache, no multi-object spread, no time-to-usable-data.
- My re-analysis of `test_results/20250430_results/fetch_range_parameterized/aggregated_results.csv` (m5.8xlarge, 10 Gbps NIC):
  | range size | obstore req/s | aiohttp req/s | aioboto3 req/s |
  | 16 KiB | 12,164 | 9,075 | 8,002 |
  | 32 KiB | 11,352 | 8,669 | 8,264 |
  | 64 KiB | 11,518 | 9,345 | 7,642 |
  | 128 KiB | 5,946 | 7,511 (52k failures) | 5,862 |
  | 256 KiB | 3,337 | 3,741 | 3,414 |
  | 512 KiB | 1,579 | 1,938 | 1,801 |
  | 1 MiB | 758 | 968 | 930 |
  | 2 MiB | 354 (98 fail) | 469 | 459 |
  | 4 MiB | 174 (9,343 fail of 16,679) | 228 | 224 |
  | 8 MiB | 108 (10,005 fail of 10,663) | 109 | 106 |
  Takeaways: obstore wins by ~25-45% only for <=64 KiB requests; from 128 KiB up every client saturates ~0.8-1.1 GB/s (the NIC) and obstore is equal or slightly behind; obstore shows large failure counts at 4-8 MiB (cause not diagnosed in the repo; INFER: default 30 s total timeout / pool). All fsspec_async rows have `number_failures == number_requests` (e.g. 1,561,949/1,561,949), i.e. the fsspec baseline in that run is invalid. => Rigor: reproducible harness, but single hot object, Python-vs-Python, throughput not latency, n=1 per cell, no CIs. [BENCH, low-moderate rigor]. It says nothing about Rust-native vs Rust-native or about time-to-first-pixel.
- obstore#273: user benchmark vs Azure SDK (32 procs x 1000 samples): obstore 2,116/3,014/4,412 MiB/s vs SDK 1,957/2,436/2,915 for 1/2/10 MiB, but 2/3/7,722 failed requests (DNS errors) and ~200 vs ~30 CPU load. Anecdotal [BENCH-lite: gist referenced, not run by me].
- No published rigorous benchmark comparing object_store (Rust) with aws-sdk-s3, CRT, or OpenDAL for reads was found (searches + repo greps). OpenDAL issue #5929 (2025, uploads) says object_store defaults beat OpenDAL defaults until `concurrent`/`chunk` tuned.

---------------------------------------------------------------------------
## 3. Apache OpenDAL

- License Apache-2.0; ASF; 5.4k stars; v0.59.4 (2026-10-05); five 0.59.x releases in 5 weeks (0.59.0 2026-09-04 -> 0.59.4) -> fast, pre-1.0 API churn ([SRC] releases). Workspace was split into `opendal-core`, `opendal-layer-*`, `opendal-http-transport-reqwest`, `opendal-service-*` crates.
- Services (core/services): s3, gcs, azblob, azdls, http, webdav, hdfs, + ~60 more. S3 config has `allow_anonymous`, `skip_signature`, `enable_request_payer`, `enable_virtual_host_style`, `checksum_algorithm`, `disable_config_load` (core/services/s3/src/config.rs:95-320). Azure `sas_token` static string (azblob/src/config.rs:75). S3 Express/directory buckets NOT supported: issue #6152 open (Xuanwo 2025-05: "works, but performance isn't as expected ... need CreateSession").
- Read API (core/core/src/types/options.rs:256-291, 408-455): `reader_with(path).concurrent(n).chunk(bytes).gap(bytes).prefetch(n)`; `Reader::fetch(Vec<Range<u64>>)` merges ranges within `gap`, splits by `chunk`, runs with `concurrent` (core/core/src/types/read/reader.rs:177-260). Still per-reader/per-object; no cross-object coalescing. Default `concurrent` = 0/1 (no parallel read) "OpenDAL by default reads without concurrency".
- Layers (core/layers): `retry` (exponential, jitter opt-in, `RetryInterceptor` notify), `timeout`, `concurrent-limit` (shared semaphore across operators; separate optional `http_semaphore`; semaphore is a trait `ConcurrentLimitSemaphore` so a custom adaptive semaphore can be plugged in — layers/concurrent-limit/src/lib.rs), `throttle`, `tail-cut`, `logging`, `tracing`, `oteltrace`, `fastrace`, `metrics`/`prometheus`/`prometheus-client`/`otelmetrics`/`fastmetrics`, `dtrace`, `await-tree`, `hotpath`, `chaos`, `route`, `foyer` cache, `immutable-index`.
- `tail-cut` (layers/tail-cut/src/lib.rs): adaptive deadline = P95 x 1.3 (defaults: percentile 95, safety 1.3, window 60 s, min_samples 200, min_deadline 500 ms, max 30 s); on expiry the request is CANCELLED with a temporary error ("cancelled by tail cut", lines 346-353) — relies on RetryLayer to re-issue; it is cancel-and-retry, NOT hedging (no concurrent duplicate).
- HTTP: `opendal-http-transport-reqwest` default is a process-global `LazyLock<ReqwestTransport>` wrapping `reqwest::Client::new()` (http-transports/reqwest/src/lib.rs:42-43) -> pool IS shared across operators by default (contrast object_store); reqwest features = `stream` + rustls only, so HTTP/2 is NOT compiled in by default. `HttpTransport` trait is replaceable per operator (`op.base_context().with_http_transport`). Docs (core/core/src/docs/performance/http_optimization.md): "benchmark HTTP/1.1 first" for large transfers, cites hyper#3623, recommends hickory DNS cache, deliberate timeouts ("Reqwest 0.13 has no request, read, or connect timeout by default").
- Perf claims: no throughput benchmark vs object_store in repo (benches/ are criterion micro-benches; `vs_s3` dir is bench against S3 service, no published numbers). vs_object_store.md is qualitative and self-described as non-objective.
- Retry semantic caveat (retry layer doc): RetryLayer + TimeoutLayer ordering sensitive (retry/src/lib.rs:46-49).

---------------------------------------------------------------------------
## 4. AWS: aws-sdk-s3, S3 Transfer Manager (Rust), CRT, Mountpoint

### 4.1 aws-sdk-s3 (Rust)
- v1.152.0 (2026-10-01), Apache-2.0, ~24M recent downloads; HTTP via `aws-smithy-http-client` (hyper 1 + rustls; features hyper-util client-legacy). Default retry = `standard`, 3 attempts; `RetryConfig::adaptive()` adds a client-side rate limiter, but AWS docs say "typically not recommended" (https://docs.aws.amazon.com/sdk-for-rust/latest/dg/retries.html). SDK does no parallel ranged GET itself (AWS maintainer, aws-sdk-rust discussion #1024, via search summary).
- S3 SDK retry finishes when response head returns, so mid-body failures are invisible to it (aws-s3-transfer-manager-rs docs/design/retry.md) -> any hand-rolled client needs its own body-retry (object_store has it).

### 4.2 aws-s3-transfer-manager-rs (awslabs, v0.3.1 2026-10-09)
- Status: README "Developer Preview ... NOT recommended for production" (README.md). Crate `aws-sdk-s3-transfer-manager` 0.3.1 on crates.io (136k recent downloads); GitHub release type "Pre-release"; 60 stars; MSRV 1.94.1; S3 only (depends on aws-sdk-s3). NOT GA.
- Features by CHANGELOG 0.2.0 (2026-07-18, "ground-up rearchitecture") and 0.3.0 (2026-09-30): per-core managed runtime threads, one partitioned connection pool (per-host cap = RLIMIT_NOFILE/2 in [10,4096]; idle close 15 s instead of 90 s), bounded shared memory pool, fair scheduling across concurrent transfers (priority), checksum validation, body-stream-failure recovery, throttle-storm recovery with per-bucket retry token bucket, stalled-stream protection, **speculative hedging of slow requests**, network-interface binding, diagnostics via `AWS_S3_TM_DIAGNOSTICS`.
- Hedging is real, wired code: `metrics/latency.rs` (`WARM_THRESHOLD=10`, `SEED_FLOOR_US=1 s`, `ESCAPE_US=5 s`, hedge token bucket capacity 10, reward 0.05/success, cost 1.0/hedge => ~4.8% sustained hedge fraction), used by download path (`operation/download/transfer.rs:1036,1182` via `LatencyTracker::guarded`). Deadline guards time-to-first-byte only; one hedge per op; cancel by dropping the future. Design: docs/design/hedging.md (explicitly cites Dean & Barroso).
- **Contradiction**: CHANGELOG 0.2.0 advertises "Adaptive concurrency ... discovered at runtime ... ramped toward the throughput the network sustains", but at HEAD `scheduler/concurrency.rs:1-12` says the adaptive controller is "retained but not wired", `ConcurrencyMode::Auto` resolves to a FIXED instance-aware seed (`client.rs:549-600`: `ceil(gbps / GBPS_PER_CONN)`, `GBPS_PER_CONN = 0.4` at runtime/platform.rs:388), and every call site constructs `FixedConcurrency` (client.rs:162,191,275). `AdaptiveConcurrencyController` (scheduler/concurrency/adaptive.rs; slow-start +32, 2% probes, BBR-style windowed max) exists only as dead code + tests. docs/design/scheduler.md lists adaptive concurrency as a requirement, not as shipped.
- Scope limits for our use: API = whole object or ONE contiguous range split into parts (`DownloadInput.range: Option<String>`), `download_objects` for prefix trees; no vectored multi-range reads of a COG, no cross-object coalescing of small ranges, S3-only (no Azure/GCS/HTTP), AWS-SDK credential chain only.
- Best used as a REFERENCE DESIGN: hedge budget, throttle isolation per bucket, stalled-stream timer, partitioned pool, per-core runtime.

### 4.3 AWS CRT S3 client (aws-c-s3) and Mountpoint
- CRT "auto-tuning" is a static formula: `ideal_connections = ceil(throughput_target_gbps / 0.4)`, clamped to [10, 10000] (`s_throughput_per_connection_gbps = 100.0/250`, `g_min_num_connections=10`, `g_max_num_connections=10000`; source/s3_client.c:71-77,180-184; code TODO: "Improve this algorithm"). Throughput target defaults to detected EC2 NIC or 10 Gbps. Not feedback-driven. Memory pool tiers by throughput (s3_client.c:~190-250). S3 Express supported (`express_support`). Standard retry strategy (aws-c-io).
- The only adaptive mechanism in CRT is the UploadPart response-first-byte timeout (`aws_s3_client_update_upload_part_timeout`, s3_client.c ~3405-3440): seed `max(1 s, P90)`, EWMA toward mean+offset, stops when >5 s — the ancestor of the TM's hedging controller. No GET hedging in CRT.
- `mountpoint-s3-client` 0.22.1 wraps CRT; its README: "not intended for general-purpose use ... interface to be unstable" -> use AWS SDK instead. `read_part_size` is validated to >= 5 MiB (s3_crt_client.rs:564-571, with a TODO to relax). Config knobs: `throughput_target_gbps`, `memory_limit_in_bytes`, part size default 8 MiB (Mountpoint doc: "in our testing is the largest value that achieves maximum throughput" [CLAIM]).
- Mountpoint prefetcher (mountpoint-s3-fs/src/prefetch.rs:1-10, 71, 115-143): one GetObject per sequential stream with a growing CRT flow-control read window (initial request 1 MiB+128 KiB, x2 multiplier up to a 2 GiB max window, resets on non-sequential read, 16 MiB forward-seek tolerance). This is sequential-read adaptivity (FUSE readahead), not applicable to random COG tile reads, but "small first request to cut latency, then ramp" is a reusable idea.

---------------------------------------------------------------------------
## 5. HTTP stack facts (reqwest / hyper / hyper-util / DNS)
- reqwest 0.13.5: `pool_max_idle_per_host` default `usize::MAX`, `pool_idle_timeout` 90 s, `http2_adaptive_window` default false, H2 window sizes default None, hickory DNS only with `hickory-dns` feature (src/async_impl/client.rs:174-379). OpenDAL docs: reqwest 0.13 has no default request/read/connect timeout.
- hyper-util legacy pool shares ONE HTTP/2 connection per host: `Reservation::Shared` (src/client/legacy/pool.rs:63-67, 231, 321); Sean Monstar (2026-08-21) on hyper#3623: legacy client "purposefully only opens one connection to the host because of RFC 9113 Section 9.1"; multi-connection H2 balancing is possible with the new composable pools (`hyper-util/src/client/pool/{singleton,negotiate,map,cache}.rs`) + tower load-balance. Practical result: once an H2 connection hits MAX_CONCURRENT_STREAMS, requests queue rather than opening a second connection (hyper#3623 body; OpenDAL docs repeat it). Issue closed 2026-09-08 but described as "not directly actionable" — treat as unsolved in the default client.
- rustls vs native-tls: object_store `aws/azure/gcp/http` features pull `reqwest/rustls` + `aws-lc-rs` (Cargo.toml features); OpenDAL default rustls (aws-lc-rs via reqwest); aws-smithy-http-client offers rustls-aws-lc/ring or s2n-tls.
- Protocol observations, 2026-10-09 from a laptop outside AWS [OBS]: `curl -s --http2 -r 0-15 -o /dev/null -w '%{http_version}'`:
  - `sentinel-cogs.s3.us-west-2.amazonaws.com/...B08.tif` (Earth Search href) -> HTTP/1.1, 206; also path-style `s3.us-west-2.amazonaws.com/sentinel-cogs/...` -> HTTP/1.1.
  - `naipeuwest.blob.core.windows.net` and `sentinel2l2a01.blob.core.windows.net` -> HTTP/1.1 (ALPN offered h2; responses were 404/400 but the protocol was negotiated before that).
  - `storage.googleapis.com/gcp-public-data-landsat/index.csv.gz` -> HTTP/2, 206.
  - `landsatlook.usgs.gov` (CloudFront IP) -> HTTP/2, 206.
  Latencies in that probe (0.25-0.75 s TTFB) are a home-network, cold-TLS artifact and not usable as performance data.
- AWS docs do not state S3 HTTP/2 support or non-support anywhere I fetched (performance guidelines/design-patterns pages); object_store#104 thread (thinkharderdev) asserts S3 is HTTP/1 only; my probe agrees [OBS]. Azure/GCS: no HTTP/2 statements found in the Azure scalability/checklist pages.

---------------------------------------------------------------------------
## 6. Adaptive concurrency / hedging in the Rust ecosystem (what exists in code)
| item | what it does | maturity |
|---|---|---|
| `tower` 0.5.3 `limit::ConcurrencyLimit`, `limit::RateLimit`, `load_shed`, `retry::budget`, `hedge::Hedge` | static semaphore; `Hedge` = pre-emptive retry after a latency percentile from a rotating histogram (tower/src/hedge/mod.rs) | tower repo last push 2026-06-22; crate last release 2026-01-12; huge usage; limiters are static |
| `tower-resilience-adaptive` 0.14.0 (joshrotenberg/tower-resilience) | AIMD and Vegas adaptive concurrency limiter for Tower; also `-hedge`, `-bulkhead`, `-ratelimiter`, `-retry`, `-circuitbreaker` | 2026-01 .. 2026-09, ~4k total downloads for adaptive; young |
| `tower-acc` 0.1.1 (guilload) | latency-driven adaptive concurrency for Tower; README describes converge up when latency low, down on queuing | 2026-03, ~1.3k dl |
| `congestion-limiter` 0.1.0 (ThomWright) | Netflix-style dynamic limits (Aimd, Vegas, Gradient, Fixed in src/limits/*.rs) | self-declared "PoC, and not production-ready"; 38k dl; last commit 2026-05 |
| `rate_limiter_aimd` 0.1.1 | AIMD fork | 1k dl, last 2025-06 |
| `object-store-hedging` 0.2.0 (spiraldb) | `HedgedStore<T>` wraps any `ObjectStore`; hedges `get_opts` only: after delay = DDSketch p95 of observed TTFB (warmup 10 samples, 200 ms), issues one duplicate GET and takes whichever returns first (src/lib.rs:~100-170) | 4 stars, 162 downloads, 2026-01..2026-07; no hedge budget/cap, one global sketch regardless of object/range size. **`get_ranges` is delegated straight to `inner.get_ranges` (lib.rs:173) which in turn calls the inner store's `get_range`, i.e. NOT hedged**, contradicting README "hedging happens automatically on reads". |
| `hedged` 0.2.0 (gwik) | generic "Hedged Requests" per Tail-at-Scale | license "non-standard" on crates.io; 6k dl |
| AWS TM (section 4.2) | hedge + token-bucket budget + stall protection (S3 only) | dev preview |
| OpenDAL `tail-cut` | adaptive cancel (P95x1.3) + retry | in OpenDAL, shipped |
- No object-store client library besides AWS TM (wired hedging, S3-only, preview) and `object-store-hedging` (TTFB hedge of get_opts) does latency-hedged duplicate reads. The Tail at Scale (Dean & Barroso, CACM 2013) is cited by TM hedging.md, tower::hedge and object-store-hedging as origin.
- AWS's own S3 guidance endorses the behaviour: retry slow requests, on a NEW connection with a fresh DNS lookup; for <512 KB requests "retry a GET after 2 seconds", for large requests retry the slowest 5%; fixed-size requests: retry slowest 1% (https://docs.aws.amazon.com/AmazonS3/latest/userguide/optimizing-performance-design-patterns.html#optimizing-performance-timeouts-retries). object_store#19 and the CIDR'25 paper "Five-Minute Rule for the Cloud" (racing reads) are the in-ecosystem pointers.

---------------------------------------------------------------------------
## 7. Hard limits / documented facts of the stores (primary docs, fetched 2026-10-09)

### AWS S3
- Request rates: "at least 3,500 PUT/COPY/POST/DELETE or 5,500 GET/HEAD requests per second per partitioned prefix"; scaling is gradual; 503 Slow Down during scale-up (https://docs.aws.amazon.com/AmazonS3/latest/userguide/optimizing-performance.html). Sustained >5,000 req/s to a small number of objects can yield 503 slowdown (design-patterns page).
- Connections: S3 "doesn't have any limits for the number of connections made to your bucket"; spread requests over separate connections; use many; pool and reuse connections; check DNS spreads across IPs (https://docs.aws.amazon.com/AmazonS3/latest/userguide/optimizing-performance-guidelines.html , .../optimizing-performance-design-patterns.html).
- Latency: "consistent small object latencies (and first-byte-out latencies for larger objects) of roughly 100-200 milliseconds" (optimizing-performance.html); S3 Express One Zone "single-digit millisecond latencies". Community-reported 8-20 ms p50 GET/HEAD in-region (object_store#541 text) [anecdote].
- Range sizes / per-connection MB/s: the CURRENT pages say only "use byte-range fetches", "GET in the same part sizes (or aligned to part boundaries) for best performance", and "start with a single request at a time, measure". The older 85-90 MB/s-per-connection and 8-16 MB-range figures are NOT in the pages fetched today. Operational constants elsewhere: CRT assumes 0.4 Gbps (=50 MB/s) per connection (aws-c-s3 s3_client.c:71) and the TM uses the same `GBPS_PER_CONN=0.4`; Mountpoint default part size 8 MiB [CLAIM]. Small-request guidance: retry after 2 s if <512 KB (design patterns page).
- Multi-range GET: unsupported by S3 (AWS API docs string reproduced in TM input.rs:44).
- HTTP/2: not documented; observed HTTP/1.1 [OBS].
- Requester pays: header `x-amz-request-payer: requester` (object_store `request_payer`).

### Azure Blob
- Scale targets (https://learn.microsoft.com/en-us/azure/storage/blobs/scalability-targets, updated 2026-07-27): single block blob "Up to 3,000 requests per second"; single page blob 500 req/s; single block blob throughput "up to storage account ingress/egress limits". Throttling = 503 Server Busy or 500 Operation Timeout; use exponential backoff; hot partitions from sequential names; ramp traffic gradually.
- Account level (https://learn.microsoft.com/en-us/azure/azure-resource-manager/management/azure-subscription-service-limits): default max request rate per GPv2/Blob account 40,000 req/s in listed regions, 20,000 elsewhere; egress 200 Gbps (listed regions) / 50 Gbps; ingress 60 / 25 Gbps.
- Perf checklist (https://learn.microsoft.com/en-us/azure/storage/blobs/storage-performance-checklist): blob/block > 256 KiB activates high-throughput block blobs (about writes); co-locate in region. No HTTP/2 statement found; observed HTTP/1.1 [OBS].
- Planetary Computer specifics: SAS token via token API; subscription key raises token-API limits only; "same data center as the service (West Europe)" gets favorable limits (https://planetarycomputer.microsoft.com/docs/concepts/sas/ [CLAIM]).

### Google Cloud Storage
- Initial per-bucket capacity ~5,000 object reads/s and ~1,000 writes/s; autoscaling; ramp so rate never more than doubles within 20 minutes; retry 408/429/5xx with exponential backoff (https://docs.cloud.google.com/storage/docs/request-rate). Quotas page: object-read rate per bucket "Unlimited" in the table but initially ~5,000/s; bandwidth quotas 200 Gbps per region for internet egress; exceed -> retryable 429 rateLimitExceeded (https://docs.cloud.google.com/storage/quotas). HNS buckets up to 8x initial QPS.
- HTTP/2: speaks H2 [OBS]; object_store#104 measured it slower than H1 for many parallel large GETs on one client.

### Throttle signals to implement (per store)
S3: 503 `SlowDown` (also inside 200 bodies for some ops: object_store retries that), 500 `InternalError`, 429 rare; Azure: 503 `ServerBusy`/500 `OperationTimedOut`; GCS: 429/408/5xx; PC token API: 429 on token endpoint without subscription key.

---------------------------------------------------------------------------
## 8. What each candidate cannot do (repo evidence)

| capability needed by an acquisition engine | object_store | OpenDAL | AWS TM (Rust) | CRT/Mountpoint client | bespoke hyper/reqwest |
|---|---|---|---|---|---|
| S3+Azure+GCS+HTTP in one API | yes | yes (many more) | S3 only | S3 only | you write signing x3 |
| Cross-object request coalescing / planning | no (per-location `get_ranges`, util.rs:99) | no (`Reader::fetch` per reader) | no | no (per-object parts) | yes if you write it |
| Per-host concurrency + shared global budget | no (LimitStore per store instance, 1 permit per get_ranges, limit.rs:103) | partial (shared `ConcurrentLimitLayer`, optional `http_semaphore`, pluggable semaphore trait) | single global target, fixed | `throughput_target_gbps` -> static conn count | yes |
| Adaptive concurrency (feedback) | none | none shipped (issue #4082 open "adaptive retry"; semaphore pluggable) | dead code; Auto = fixed seed | static formula only | write (tower-acc/AIMD crates are young) |
| Hedging | none in-tree; 3rd-party get_opts-only crate | tail-cut = cancel+retry only | yes, S3-only GET body path | none for GET | write |
| Retry budget / throttle isolation | no (flat 10 retries/180 s, no Retry-After) | retry layer (attempts, jitter) | yes (per-bucket token bucket) | CRT standard retry | write |
| Connection warm-up / pre-resolve | none (no API; reqwest has none) | none | partitioned pool, idle 15 s | CRT DNS cache TTL 5 min (s3_client.c `s_dns_host_address_ttl_seconds`) | write |
| DNS caching | none by default (getaddrinfo + shuffle; custom resolver hook since #728) | none by default (docs: enable hickory) | aws-smithy-dns hickory | CRT host resolver cache | pick |
| Shared connection pool across stores/hosts | no (new reqwest client per store) unless custom connector | yes (global default client) | one pool | one client | yes |
| H2 tuning (windows, multiple H2 conns) | only keep-alive/frame size exposed; hyper legacy = 1 H2 conn/host | via own reqwest client | n/a (S3=H1) | n/a | full control |
| Request-level metrics/tracing | none (only HttpService seam; #705 open) | many layers (prometheus, otel, tracing, logging) | `AWS_S3_TM_DIAGNOSTICS`, tracing targets | Mountpoint metrics/OTLP | write |
| Mid-body retry/resume | yes (get.rs retry_stream) | partial | yes | yes | write |
| Credential refresh at 1000+ concurrency | fixed in #648 (RwLock) | per-service | SDK chain | CRT | write |
| S3 Express | yes (CreateSession) | no (#6152 open) | via SDK | yes | write |
| Requester pays | S3 yes, GCS no (#629) | S3 yes | SDK | CRT | write |
| Parallel ranged download of ONE big object | no (#274 open) | yes (`concurrent`/`chunk`) | yes (core feature) | yes | write |
| API stability / maintenance | 0.x, ~6-month breaking, ASF, DataFusion depends on it | 0.x, weekly releases, ASF | pre-release preview | "unstable, not for general use" | yours |

## 9. Verdict (INFER, from the evidence above)
1. Do NOT adopt AWS TM, Mountpoint client/CRT or obstore as the I/O layer: TM is S3-only preview with whole-object/contiguous-range semantics; mountpoint-s3-client self-declares unstable/not general-purpose; obstore is a Python wrapper of object_store.
2. Choose **object_store + custom `HttpConnector` + a custom scheduler layer above the `ObjectStore` trait**:
   - keep object_store for what is tedious and security-sensitive: SigV4/Azure/GCS signing, credential providers (S3 Express, SAS via custom provider, skip_signature, requester-pays), mid-body resume, error classification, URL parsing;
   - replace the transport with ONE shared client via `HttpConnector` (control pool size, idle timeout ~ <=15 s given S3 closes idle conns quickly per aws-c-s3 comment, h1 vs h2 per host, H2 windows/multiple H2 connections, DNS cache + shuffle, TCP options) and wrap it in an `HttpService` decorator for per-request timing/metrics (the only instrumentation point);
   - put planning, cross-object coalescing, per-host AIMD limits, a hedge budget modelled on AWS TM latency.rs (TTFB-only, one hedge/op, ~5% budget), throttle isolation and warm-up in a scheduler that owns request issue; use `coalesce_ranges` with your own gap/parallelism for per-object merges; set `max_retries` low and own retries in the scheduler if you want budgets/Retry-After.
3. Consider OpenDAL only if built-in observability layers + shared default pool + pluggable `ConcurrentLimitSemaphore` outweigh: no S3 Express, weekly 0.x churn, and Azure SAS as a static string. It is not faster by any published evidence (#5929 favoured object_store defaults).
4. Go bespoke on hyper (no object_store) only if profiling shows trait-object/`async_trait` + signing + `Bytes` copies are the bottleneck after steps above; evidence so far points at DNS, pool, credential lock and concurrency policy — all fixable under object_store.
5. Benchmark gating: obstore's published advantage evaporates above ~64 KiB per request because a 10 Gbps NIC saturates (pyasyncio-benchmark CSV). For COG-tile workloads (tens of KB to a few MB per request) request-rate/latency-tail, not CPU, is the limiter => design for latency percentiles and time-to-usable-data, not MB/s.

## 10. Open questions
- Does object_store's S3 path sustain >5k small GET/s per process without the earlier credential-lock pathology after #648? (needs own benchmark; no public Rust-vs-Rust number.)
- Azure/GCS single-range-only? (S3 confirmed; Azure/GCS not verified.)
- Real S3 idle-connection timeout (aws-c-s3 comment says ~5 s; reqwest default keeps 90 s) — verify empirically; affects warm-up/keep-alive design.
- What is the failure cause of obstore at 4-8 MiB in pyasyncio-benchmark (30 s total timeout? pool?)
- Planetary Computer blob-level rate limits are undocumented in the pages read; token API throttling documented only at policy level.
- Is Earth Search `data.source.coop`/other href hosts (CloudFront) H2 with Range support for all? (landsatlook = H2 per probe.)
