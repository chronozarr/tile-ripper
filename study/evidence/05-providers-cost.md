# 05 - Sentinel-2 L2A providers, formats, limits, costs (probed 2026-10-09)

Evidence grades: [SRC] primary doc/code/response read by me; [BENCH] my own measurement (small, single-run unless noted); [CLAIM] assertion w/o method; [INFER] my inference.
All probes were anonymous public GET/HEAD/Range (<=64 KB per header read). No imagery decoded, no accounts, no auth (PC SAS token = the documented anonymous token endpoint; used only for 64 KB header ranges + HEAD).
Scratch artifacts: scratchpad/stac/*.json (STAC items), scratchpad/stac/hdr/* (header bytes), scratchpad/{tiffhdr,tilesizes,jp2hdr}.py (parsers), scratchpad/stac/workload.py.

Reference scene used everywhere: tile 17SPV, acquisition 2026-08-30 15:58 UTC, S2C, baseline 05.12 (95.5% cloud).
  ES item  S2C_17SPV_20260830_0_L2A (v1 sentinel-2-l2a)   https://earth-search.aws.element84.com/v1/collections/sentinel-2-l2a/items/S2C_17SPV_20260830_0_L2A
  ES C1    S2C_T17SPV_20260830T160924_L2A (v1 sentinel-2-c1-l2a == v2 sentinel-2-l2a)
  PC item  S2C_MSIL2A_20260830T155821_R097_T17SPV_20260830T211114
  CDSE     S2C_MSIL2A_20260830T155821_N0512_R097_T17SPV_20260830T211114
Note: file size varies with valid area (edge-of-swath granule on GCS: 10 m B04 JP2 = 15.8 MB, not ~110 MB).

## 1. Provider facts

### AWS Earth Search / sentinel-cogs (Element84)  [SRC all unless noted]
- STAC: https://earth-search.aws.element84.com/v1 collections: sentinel-2-pre-c1-l2a, sentinel-2-l2a (legacy, all baselines), sentinel-2-c1-l2a (Collection 1, baseline>=05.00), sentinel-2-l1c, ... v2 (beta) https://earth-search.aws.element84.com/v2 : v2 `sentinel-2-l2a` REPLACES v1 `sentinel-2-c1-l2a` (same Item IDs, STAC 1.1.0, only baselines 05.00+ excl 05.09). Doc: https://github.com/Element84/earth-search/blob/main/docs/collections/sentinel-2-l2a.md
- Buckets (AWS registry https://github.com/awslabs/open-data-registry/blob/main/datasets/sentinel-2-l2a-cogs.yaml): `sentinel-cogs` (legacy L2A COGs, us-west-2, RequesterPays False), `e84-earth-search-sentinel-data` (C1 COGs, us-west-2, RequesterPays False).
- Anonymous HTTPS: hrefs https://sentinel-cogs.s3.us-west-2.amazonaws.com/sentinel-s2-l2a-cogs/17/S/PV/2026/8/<item>/B04.tif (HEAD+Range worked unauthenticated). s3:// hrefs only for the JP2 `*-jp2` assets (s3://sentinel-s2-l2a/...) and product_metadata.xml (these are in the eu-central-1 JP2 bucket).
- Storage class INTELLIGENT_TIERING (x-amz-storage-class header), Cache-Control immutable, ETag multipart (-25 parts).
- Legacy COG format [SRC own header parse]: 10 m B04 = uint16 (16 bit), Deflate(8), Predictor 2, tile 1024x1024 base (121 tiles), overviews 5490/2745/1373/687 with 512x512 tiles (4 overviews), first IFD at offset 8, no GDAL ghost header (no IFDS_BEFORE_DATA signature). 20 m (B11): base tile 1024 (36 tiles), 3 overviews. SCL uint8: base tile 512 (121 tiles), overviews tile 256.
- C1 COG format [SRC]: same Deflate+pred 2, 1024/512 blocks, BUT has GDAL ghost header (COG driver layout, IFDS_BEFORE_DATA, first IFD at 192) and STAC `file:size`, `file:checksum`, `storage:*` (platform AWS, region us-west-2, requester_pays False).
- Sizes (reference scene, HEAD Content-Length): B02 203.6 MB, B03 202.9, B04 202.8, B08 201.2; B05 57.0, B11 54.2, B12 53.4; B01 6.8, B09 6.6; SCL 1.8; AOT 0.39; WVP 4.4; TCI 89.0. 7 other low-cloud scenes (6 land covers) B04 whole-file 196-230 MB.
- Base-level compressed bytes (sum of TileByteCounts IFD0) B04: 146.3 (cloudy ref), 160.5 NC forest, 161.9 Mojave, 152.0 NL ag, 146.6 Amazon, 164.8 Sahel, 171.0 Alps => mean 157.6 MB of 241.1 MB raw => 1.53:1 (65%). Mean compressed 1024-block = 1.30 MB (raw 2.10 MB). Overviews add ~33% to file size.
- STAC item fields (legacy v1 item): proj:epsg (item), proj:centroid; asset-level proj:shape, proj:transform, raster:bands{nodata,data_type,spatial_resolution,scale,offset}, eo:bands, gsd; grid:code "MGRS-17SPV" + mgrs:utm_zone/latitude_band/grid_square (NO s2:mgrs_tile); s2:processing_baseline "05.12"; earthsearch:boa_offset_applied; earthsearch:s3_path; NO file:checksum/file:size in legacy. Both COG and JP2 (`*-jp2`) assets are listed.
- STAC rel: Earth Search is "best effort" (README https://github.com/Element84/earth-search/blob/main/README.md). No published rate limits found.
- Counts: ES v1 sentinel-2-l2a ~51.6 M items, c1 ~30.6 M, v2 l2a ~26.2 M (doc above).

### AWS Open Data `sentinel-s2-l2a` (ESA original JP2)  [SRC registry + own probes]
- Registry https://github.com/awslabs/open-data-registry/blob/main/datasets/sentinel-2.yaml: sentinel-s2-l2a, eu-central-1, RequesterPays: False. sentinel-s2-l1c and the *-zips buckets: RequesterPays True in the registry. [BENCH] BUT anonymous HEAD of https://sentinel-s2-l1c.s3.eu-central-1.amazonaws.com/tiles/17/S/PV/2026/8/30/0/B04.jp2 returned 200 on 2026-10-09 (contradicts registry).
- Anonymous HTTPS verified: https://sentinel-s2-l2a.s3.eu-central-1.amazonaws.com/tiles/17/S/PV/2026/8/30/0/R10m/B04.jp2 -> 109,842,415 bytes; CDSE STAC `file:size` for the same band = 109,842,415 (byte-identical size => same ESA/Sinergise JP2).
- JP2 codestream [SRC own parse of first 8 KB]: 10980x10980, 1 component, 15-bit unsigned, tile 1024x1024 (121 tiles), 4 decomposition levels (5 resolutions), code-block 64x64, 1 layer, 5/3 reversible wavelet, progression byte 0 (LRCP), user precincts (Scod=1), TLM and PLT markers present.
- Includes `product_metadata.xml`, `tileinfo` etc. S2 raw DN includes +1000 offset for baseline>=04.00 (ESA original).

### Microsoft Planetary Computer  [SRC]
- STAC https://planetarycomputer.microsoft.com/api/stac/v1 collection `sentinel-2-l2a`; storage account sentinel2l2a01, container sentinel2-l2, msft:region westeurope (collection JSON). Single storage account for all S2 L2A.
- hrefs are unsigned https://sentinel2l2a01.blob.core.windows.net/sentinel2-l2/17/S/PV/2026/08/30/<SAFE>/GRANULE/.../IMG_DATA/R10m/T17SPV_20260830T155821_B04_10m.tif ; must append SAS token. Anonymous blob read disabled (discussion https://github.com/microsoft/PlanetaryComputer/discussions/378 as quoted in issue 457).
- Own re-processing with Sen2Cor to COG ("converted to cloud-optimized GeoTIFF" in collection description). Assets: AOT(10 m!), B01(60), B02-B04(10), B05-B07(20), B08(10), B09(60), B8A(20), B11/B12(20), SCL(20), WVP(10 m!), visual TCI(10). No B10.
- COG format [SRC own header parse]: 10 m B04: BitsPerSample 15 (uint16), Deflate, Predictor 1 (NONE), tile 512x512 everywhere (484 tiles at base), 4 overviews (5490..687), GDAL COG ghost header (IFDS_BEFORE_DATA, BLOCK_LEADER/TRAILER) , first IFD at 192.
- Sizes (ref scene, SAS HEAD): B02 277.1 MB, B03 277.7, B04 278.1, B08 273.7; B05 73.4, B11 72.2; SCL 1.57; AOT 3.6; TCI 106.5. Blob type BlockBlob, tier Hot.
- Base-level compressed B04 (same-date scenes as ES samples): 204.8 (ref), 209.6 Alps, 186.3 Amazon, 205.0 Mojave, 186.2 NL, 190.4 Sahel => mean 197.1 MB => 1.22:1 (82%); mean 512-block 0.407 MB (raw 0.524). PC is 1.2-1.27x bigger than ES for the SAME scene because no predictor.
- STAC item fields: proj:epsg (item), s2:mgrs_tile "17SPV", s2:processing_baseline, s2:granule_id; assets: proj:bbox, proj:shape, proj:transform, gsd. NO raster:bands (no scale/offset anywhere), NO file:checksum/file:size, extension list only eo/sat/projection.
- Offset: raw ESA DN (baseline>=04.00 has +1000), nothing in STAC. Open since 2022: https://github.com/microsoft/PlanetaryComputer/issues/134 ("workaround is for users to apply the offset themselves", maintainer). Offset is only in product XML (MTD_MSIL2A.xml, asset `product-metadata`) per https://github.com/sentinel-hub/titiler-openeo/pull/427 (checked live 2026-10-08 per its author) [CLAIM-third-party].
- Completeness [BENCH]: tile 17SPV Aug-Sep 2026: PC 25 items vs ES 30 (missing e.g. 2026-08-25, 09-04, 09-09, 09-14). Point in 17SPV Aug 2026: PC 6, ES 7, CDSE 7.
- SAS: https://github.com/microsoft/PlanetaryComputerDataCatalog/blob/main/docs/concepts/sas.md ; GET https://planetarycomputer.microsoft.com/api/sas/v1/token/sentinel-2-l2a (container-scoped, anonymous). [BENCH] anonymous token fetched from a macOS workstation (non-Azure) 2026-10-09 19:44:38Z: msft:expiry 20:29:38Z => 45 min; sv=2025-07-05; user-delegation key window 7 days. Maintainer: "By default, SAS tokens will be valid for 45 minutes" (issue 457 https://github.com/microsoft/PlanetaryComputer/issues/457). User reports 8-24 h from some locations (same issue; issue 415) - not reproduced.
- Rate limits: not published. Docs: tier depends on (a) request origin inside West Europe, (b) optional API key. 429 body: {"statusCode":429,"message":"Rate limit is exceeded. Try again in 23 seconds."} (issue 457). Issue 506 (2026-09-25, closed 2026-10-02): 429 on /sas/v1/sign after ~10-12 calls even with 1 s sleep; maintainer: use token endpoint once per container + cache (planetary_computer pkg does). 2025-02 incident: 100x signing traffic -> 429 (issue 415). STAC search timeouts ~70% 2026-05-02, open (issue 476). Azure Blob returns 503 ServerBusy/500 at partition limits (docs below).
- Azure storage account targets West Europe GPv2: 40,000 req/s, 200 Gbps egress, 60 Gbps ingress (default) https://learn.microsoft.com/en-us/azure/storage/common/scalability-targets-standard-account (updated 2026-06-16).
- Local repo note: /Users/jakegearon/projects/chronozarr/examples/sentinel2_pc/catalog.py documents "SAS tokens expire in about an hour... signing at search time broke every month after the first hour of a full-archive run with HTTP 403".

### Copernicus Data Space Ecosystem (CDSE)  [SRC]
- STAC https://stac.dataspace.copernicus.eu/v1 (collection description: Level-2A "Collection 1 product"; items include current baselines, e.g. N0512 for 2026-08). Format: JPEG2000 inside SAFE; per-resolution assets B02_10m, B02_20m, B02_60m ... (every band at every resolution: ESA-resampled) + `Product` zip.
- hrefs: s3://eodata/Sentinel-2/MSI/L2A/2026/08/30/<SAFE>/GRANULE/.../IMG_DATA/R10m/T17SPV_20260830T155821_B02_10m.jp2 (auth:refs s3); alternate https://download.dataspace.copernicus.eu/odata/v1/Products(<uuid>)/Nodes(...)/$value (auth: OIDC bearer).
- S3: endpoint https://eodata.dataspace.copernicus.eu (GSLB CloudFerro/OTC; OTC-pinned https://eodata.ams.dataspace.copernicus.eu), bucket `eodata`, region name `default`; keys generated in the CDSE S3 keys manager (account required; "Add Credentials" with user-chosen expiration date; secret shown once) https://documentation.dataspace.copernicus.eu/APIs/S3.html . STAC storage:schemes: cdse-s3 requester_pays false; creodias-s3 (eodata.cloudferro.com, commercial, no quota) requester_pays true.
- STAC item fields present: file:size, file:checksum (multihash), proj:shape (asset), proj:transform, proj:bbox, proj:code (EPSG:32617; projection v2 so no proj:epsg), raster:scale 0.0001 + raster:offset -0.1 (raster v2, per asset; truthful: raw ESA DN), data_type, grid:code MGRS-17SPV, processing:version "05.12" (= baseline), eopf:* , statistics, _private.product_size 915.4 MB (zip). No s2:* keys.
- Sizes (ref scene, STAC file:size, JP2): B02_10m 112.4 MB, B03 111.4, B04 109.8, B08 115.3; 20 m bands 30-33 MB (B01_20m 22.1); 60 m 3.7 MB; SCL_20m 1.39 MB. (Same bytes as AWS sentinel-s2-l2a.) JP2 contains its own resolution pyramid (no extra 33%): 2.2:1 vs raw 241 MB.
- Quotas [SRC https://documentation.dataspace.copernicus.eu/Quotas.html; source https://github.com/eu-cdse/documentation/blob/main/Quotas.qmd last change 2026-01-09]: S3/OData/STAC general user: 4 concurrent connections, 20 MB/s per connection, 12 TB per rolling 30 d (checked hourly; over quota -> 1 MB/s and 1 connection), S3 2,000 requests/min ("only applies to S3"), 100 active sessions, access token active 10 min, refresh possible within 60 min, "no limits" on products per token. Docs say each file in a recursive S3 download counts as a request. No 429/503 semantics documented. "Direct HTTP access to COGs": 50,000 req/month (not S2 L2A JP2).
- => CDSE S3 theoretical ceiling for one user = 4 x 20 MB/s = 80 MB/s (~0.64 Gbps) and 33 req/s.

### Google Cloud public bucket  [SRC own probes]
- gs://gcp-public-data-sentinel-2 : location EU (multi-region), STANDARD, anonymous JSON-API/HTTPS (https://storage.googleapis.com/storage/v1/b/gcp-public-data-sentinel-2 ...). `L2/tiles/<UTM>/<lat>/<sq>/<SAFE>/` = L2A SAFE with JP2 per band per resolution; `tiles/...` = L1C. L2A present through 2026-09 for 17SPV. index.csv.gz last-modified 2024-08-28 (stale). Docs page: https://docs.cloud.google.com/storage/docs/public-datasets/sentinel-2 ("available for users to access for free"). Bucket owner pays unless requester-pays (https://docs.cloud.google.com/storage/docs/requester-pays). No STAC (use CDSE/ES; or the index). GCS egress list price: [CLAIM, not verified: ~$0.12/GB first TiB].

### ESA EOPF Sentinel Zarr Samples Service  [SRC]
- Site https://zarr.eopf.copernicus.eu/data-and-tools/ ; STAC https://stac.core.eopf.eodc.eu/ (stac-fastapi-pgstac; collections incl. sentinel-2-l2a, sentinel-2-l2a-zarr3 "Zarr3 staging", sentinel-2-l1c(-zarr3)). FAQ (updated 2025-12-16) https://zarr.eopf.copernicus.eu/wp-content/uploads/2025/12/EOPF-Sentinel-Zarr-Samples_FAQ.pdf : no credentials needed; CEPH object storage by EODC in Vienna; fair-use, no credit system; one Zarr store per SAFE product (not a datacube); "Operational generation of Zarr products is expected by late 2026 or early 2027"; "currently only a rolling one-year archive"; Europe L2A, each tile <900 MB; Zarr v2 currently, v3 migration planned; sharding "via ZEP2 in future GDAL releases". Direct download not recommended.
- Probes [BENCH]: Bavarian point (11.1E,48.1N): items Sep 2026 EOPF-zarr2 17 / zarr3 18 / ES 18 / CDSE 18; Mar 2026: 17/0/17/17; Jun 2025: 1/0/16/16; Jun 2023: 0/0/12/12. CONUS bbox 2026: 0 items (zarr2 and zarr3). One NC item exists (2025-04-17). => Europe-only, ~7-12-month rolling window in practice.
- Zarr v2 array (default collection, cpm_v270, 2026-09-29 item) https://data.eodc.eu/collections/EOPF_ZARR/products/cpm_v270/S02MSIL2A/2026/09/29/<id>.zarr/measurements/reflectance/r10m/b04/.zarray : shape 10980x10980, chunks 2048x2048, dtype <u2, Blosc(zstd, clevel 3, shuffle=2 bitshuffle), fill 0; .zattrs add_offset -0.1, scale_factor 0.0001 (CF), .zmetadata consolidated. Chunk 0.0 = 5,097,989 B (raw 8.39 MB => 0.61). Older item (cpm_v256, objectstore.eodc.eu:2222) STAC also anonymous. FAQ says 1830/915/305 chunks - outdated vs observed 2048.
- Zarr v3 staging (cpm_v300, 2026-10-06 item): chunk_grid regular 11264x11264 (= one shard per band per tile) with sharding_indexed inner chunk 1024x1024, inner codecs bytes + blosc(zstd, clevel 3, shuffle), index_codecs bytes+crc32c, index_location end. add_offset -0.1 stored in attributes. => one object per band per scene, inner-chunk reads via range GET.
- Asset list: SR_10m/20m/60m groups, per-band B02_10m etc., SCL_20m, TCI_10m, AOT/WVP, zipped_product, .zmetadata. STAC item has proj:code/proj:bbox, no file:checksum.
- GitHub PR (EOPF-Explorer/data-pipeline #415) says staging items expire 2026-11-01 - from a web-search summary only [CLAIM, unverified].

## 2. Processing-baseline offset (BOA_ADD_OFFSET = -1000 DN from baseline 04.00, 2022-01-25)
Semantics: for baseline>=04.00 ESA writes DN = 10000*rho + 1000; reflectance = 0.0001*DN - 0.1. "Harmonized" = subtract 1000 (clip at 1; mind uint16 wrap: PC issue 134; S2Mosaic PR https://github.com/DPIRD-DMA/S2Mosaic/pull/9 uses DN 0 stays 0, 1-1000 -> 1).

| Source | Pixel values | Where offset lives | Reliability |
|---|---|---|---|
| PC sentinel-2-l2a | raw ESA DN (+1000 for >=04.00) | nowhere in STAC; only product XML; s2:processing_baseline >= 4.0 is the proxy (as in local catalog.py boa_offset) | known gap, open since 2022 |
| ES v1 sentinel-2-l2a (legacy) | mixed: harmonized (offset removed) when earthsearch:boa_offset_applied==true, raw when false/absent | raster:bands offset is -0.1 on ALL baseline>=04 items even where already applied (issue https://github.com/Element84/earth-search/issues/9, maintainer: flag is the truth); flag itself wrong for some 2025 items (issues #66 https://github.com/Element84/earth-search/issues/66 and #71 https://github.com/Element84/earth-search/issues/71, both OPEN, #71 last activity 2026-08-21) | UNRELIABLE; COG headers carry no scale/offset (maintainer, issue 9) |
| ES v1 sentinel-2-c1-l2a / v2 sentinel-2-l2a | raw ESA DN | raster:scale 0.0001 + raster:offset -0.1 truthful; always apply both (maintainer tylanderson, issue https://github.com/Element84/earth-search/issues/41) | OK; no boa_offset_applied property |
| CDSE JP2 / AWS sentinel-s2-l2a / GCS | raw ESA DN | CDSE STAC raster:offset -0.1; product XML | OK |
| EOPF Zarr | raw ESA DN | add_offset -0.1 in .zattrs (CF-decodable by xarray) | OK |
- 7 of 7 sampled 2026 legacy ES items: boa_offset_applied=true, raster offset -0.1 (S2B_17SPV_20260914, S2C_11SNV_20260924, S2C_31UFT_20260814, S2B_20MQB_20260901, S2C_31PCQ_20260825, S2C_32TNS_20260927, S2C_17SPV_20260830). I did NOT decode pixels (rule) - the benchmark must verify per provider with a dark-water/dark-forest min-DN check or compare first tile vs CDSE JP2.
- Cross-catalogue table (checked live 2026-10-08 by its author; third-party): https://github.com/sentinel-hub/titiler-openeo/pull/427 .
- S2Mosaic PR: PC pixel = raw (+1000), ES = harmonized (blue median 1942 vs 926 for same dates).
- ES COG for legacy items: sizes of legacy vs C1 B04 same scene 202.81 vs 202.67 MB; base-level tile sums 146.3 vs 146.1 MB. Predictor 2 means a constant shift barely changes compressed size so size cannot tell them apart.

## 3. Rate limits / auth summary
| Provider | Auth | Token life | Limits (documented) | Notes |
|---|---|---|---|---|
| ES sentinel-cogs / e84 bucket | none (anonymous S3/HTTPS) | n/a | S3: >=5,500 GET/HEAD/s per prefix, 3,500 write, 503 SlowDown on scale-up https://docs.aws.amazon.com/AmazonS3/latest/userguide/optimizing-performance.html ; no per-IP limit documented; STAC "best effort" | EC2 multi-flow via IGW: <32 vCPU capped 5 Gbps; >32 vCPU 50% of instance bw; single flow 5 Gbps https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/ec2-instance-network-bandwidth.html |
| AWS sentinel-s2-l2a JP2 | none | n/a | same S3 | eu-central-1 |
| PC | anonymous SAS from /sas/v1/token/{collection} (container scope) | 45 min observed (anon, non-Azure) | unpublished; 429 w/ Retry hint on signing API; better inside West Europe / with key | refresh well before expiry; refresh per container; do not call /sign per asset |
| CDSE | account + S3 keys (user-set expiry) or OIDC bearer (10 min, refresh <=60 min) | S3 key: until chosen expiry | 4 conn, 20 MB/s/conn, 2,000 S3 req/min, 12 TB/30d | HTTP 429/503 behaviour undocumented |
| EODC EOPF | none | n/a | fair use, unspecified | APISIX gateway; CORS * ; HTTP/2 |
| GCS | none | n/a | none documented | EU multi-region |

## 4. Prices (list)  [SRC AWS bulk pricing JSON pulled 2026-10-09: https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonS3/current/us-west-2/index.json (pub 2026-09-28) and .../AWSDataTransfer/current/us-west-2/index.json (pub 2026-09-16); eu-central-1 same paths]
- S3 GET/SELECT: us-west-2 $0.0004 per 1,000 ($0.004/10k); eu-central-1 $0.00043 per 1,000. PUT/COPY/POST/LIST: $0.005 / $0.0054 per 1,000.
- Internet egress (both regions): $0.09/GB first 10 TB, $0.085 next 40 TB, $0.07 next 100 TB, $0.05 >150 TB; first 100 GB/month free aggregated across services (S3 pricing page https://aws.amazon.com/s3/pricing/ text). Inter-region $0.02/GB (sender pays). S3 -> EC2/AWS services same region: no charge (S3 pricing page). Intra-region cross-AZ $0.01/GB each way (bulk JSON "IntraRegion").
- Open Data Sponsorship covers storage AND data transfer for sponsored datasets: handbook https://assets.opendata.aws/aws-onboarding-handbook-for-data-providers.pdf ("covers the cost of storage and data transfer"; "our program terms only cover S3 storage / egress costs"). => anonymous requester of non-requester-pays sentinel-cogs pays $0 for requests and egress. Requester-pays buckets: requester pays request + transfer.
- Azure (retail prices API, westeurope): internet egress $0.087/GB (100 GB - ~10.3 TB), $0.083, $0.07, $0.05; first 100 GB free; Routing-Preference Internet tier $0.08/0.065/0.06/0.04; inter-region $0.02; Blob hot read ops $0.0043 per 10k ($0.00043/1k). PC downloads: "data hosted by the Planetary Computer is free for anyone to use" (sas.md) - user pays nothing for egress/ops; only throttled.
- GCS: public bucket owner pays; [CLAIM] list egress ~$0.12/GB.
- CDSE: free within quotas; CREODIAS S3 variant requester pays (commercial).
- NAT gateway data processing (~$0.045/GB) applies if EC2 reaches S3 via NAT instead of a gateway VPC endpoint [CLAIM from general knowledge, not verified here].

Cost formulas (B = GB transferred incl. over-read, R = GET count, T = wall hours, p = VM $/h)
- Workstation <- sponsored public data (ES sentinel-cogs, e84 bucket, AWS JP2, PC, EODC, GCS): marginal cloud cost = $0; real cost = ISP + T x workstation.
- Same-region VM (EC2 us-west-2 <- sentinel-cogs): C = T*p (+ NAT 0.045*B if no gateway endpoint). S3 GET/egress $0 to you (sponsor pays). If you read YOUR OWN us-west-2 copy: C = T*p + R*0.0000004; transfer 0.
- Cross-region VM (e.g. us-east-1 <- sentinel-cogs): sender-billed transfer; sponsor-paid for sponsored non-requester-pays => $0 to you [INFER]; if requester-pays: B*0.02 + R*0.0000004.
- Requester-pays bucket from internet: C = R*0.0004/1000 + B*0.09 (<=10 TB tier, minus 100 GB free) .
- Azure VM in West Europe <- PC: C = T*p; egress/ops $0 (PC pays). Own Azure copy: R*0.00043/1000 + same-region transfer 0.
- Illustrative at list price (hypothetical, if requester paid) for workload A: 20.1 GB*$0.09 = $1.81 + 17k GET*0.0000004 = $0.007.

## 5. VM NIC ceilings
- AWS c7gn: 16xlarge 200 Gbps, 12xl 150, 8xl 100, 4xl 50, 2xl 25 (burst 50), xlarge 12.5 (burst 40); c6in: 32xl 200, 24xl 150, 16xl 100, 12xl 75, 8xl 50, 4xl 25(burst 50); c5n.18xlarge 100, 9xl 50 https://docs.aws.amazon.com/ec2/latest/instancetypes/co.html . Caveat (EC2 bandwidth doc above): traffic through an internet gateway: instances <32 vCPU limited to 5 Gbps multi-flow, >32 vCPU limited to 50% of instance bandwidth; single flow 5 Gbps (10 in cluster PG, ENA Express 25). S3 gateway VPC endpoint traffic is "in-Region" so should not be IGW-capped [INFER - not stated in doc].
- Azure Ddsv5: D32 16,000 Mbps, D48 24,000, D64 30,000, D96 35,000 https://learn.microsoft.com/en-us/azure/virtual-machines/sizes/general-purpose/ddsv5-series ; Dsv6/Ddsv6 D96 41,000 Mbps; Dndsv6 (preview) D96nds 150,000 Mbps (web-search summary of learn.microsoft.com pages) . Azure limit = aggregated across NICs, not guaranteed.

## 6. Workload sizing  (all [INFER] unless a number is a measured input)
Measured inputs: raw 10 m band = 10980^2*2 B = 241.12 MB; SCL 20 m uint8 = 30.14 MB. ES base compressed/band 157.6 MB (1.53:1; 1024-block 1.302 MB); PC 197.1 MB (1.22:1; 512-block 0.407 MB); CDSE/AWS JP2 ~110 MB/band whole file. AOI window block counts from STAC geometry via pyproj (workload.py) for 6 aois.yaml AOIs, 2025-10..2026-09 on ES.
Items per MGRS tile per month (all clouds, ES, 12 mo): Ucayali 8.6 (2 tiles, 17.2 items, 8.5 dates/mo); Rondonia 8.5; Lake Mead 7.4/tile (4 tiles); Mekong 7.5/tile; Nile 17.5/tile; NL(52N) 17.9/tile; Yukon(60N) 11.7; point inside single-orbit area 7/mo. Platforms present: S2A, S2B, S2C all three. Duplicates (sequence _1/_3 reprocessings) 1-2% tropics, ~18% same-tile-same-date S2A+S2B at high overlap. => 7-18 items/tile/month; AOI straddling a tile edge reads 2 tiles; 9.8 km tile overlap.
A. Owner reference: 25 km AOI (Ucayali, 2765x2759 px, 7.63 Mpx), B02/B03/B04/B08 uint16 + SCL.
   per date decoded = 4*7.63e6*2 + 1.9e6 = 62.9 MB; 12 GB / 62.9 MB = 191 dates; at 8.5 dates/mo = 22.4 months (~1.9 yr); items = 22.4*17.2 = 385 (two tiles/date; naive per-item reads fetch 1.35x union pixels: 2*5.17/7.63).
   blocks/band-item: 10.0 (1024), 31.5 (512). GETs: ES 385*4*10.0 = 15.4k (+~2k header/SCL); PC 385*4*31.5 = 48.5k (+~2k). Coalescing row-major block runs would cut GETs ~3x [INFER].
   compressed read: ES 15.4k*1.302 = 20.1 GB; PC 48.5k*0.407 = 19.8 GB => 1.65-1.7x useful bytes (block rounding + tile overlap).
   output if monthly median composites: 22.4*4*7.63e6*2 = 1.37 GB.
B. Large AOI x few scenes: 1 full tile, 4 bands + SCL: per scene 4*241.1 + 30.1 = 994.6 MB; 12.06 scenes = ~12 scenes (11.9 GB) = ~1.4-1.7 months of one single-orbit tile.
   read: ES 12*4*157.6 = 7.6 GB (0.63x); PC 12*4*197.1 = 9.5 GB; JP2 12*4*110 = 5.3 GB.
   GETs (1 per block): ES 12*(4*121+121) = 7.3k; PC 12*(4*484+121) = 24.7k; whole-object ranged GETs (16 MB) ~ 13 per ES band.
C. Small AOI x many scenes -> must be many sites: 500 chips 512x512 px x 4 bands x 12 monthly dates: 500*12*4*0.524 MB = 12.6 GB.
   blocks/chip-band random alignment (1+511/1024)^2 = 2.25 (ES), (1+511/512)^2 = 3.99 (PC). GETs ES 54k, PC 96k (+24k header GETs if 6,000 distinct scene-bands).
   read ES 54k*1.302 = 70.2 GB (5.6x useful), PC 96k*0.407 = 39.0 GB (3.1x). At 256 px chips: ES 15.5x, PC 7.0x. (Single 5 km AOI x 10 yr would give only ~3 GB with 10 bands: per date 4*0.25e6*2 + 6*0.0625e6*2 = 2.75 MB; ~1000 dates.)
D. Overlap-heavy: 24x28 km AOI at 52N (NL, 2 tiles, 35.8 items/mo, 14.7 dates/mo), 4 bands: per item useful 4*3.84e6*2 = 30.7 MB; 12 GB = 391 items = 10.9 months. blocks/band-item 7.5 (1024) / 24.0 (512). GETs ES 11.7k, PC 37.4k; read ~15.2 GB both.
Time floors (read GB / bandwidth): 20.1 GB -> 32 s @5 Gbps, 6.4 s @25 Gbps, 251 s @CDSE 80 MB/s (+ 17k+ requests = 8.5+ min @2,000/min). 70.2 GB -> 112 s @5 Gbps; 878 s @80 MB/s (+27 min @2,000 req/min if 54k requests).

## 7. Latency from this workstation [BENCH, n=3 cold connections each, 64 KB range GET, noisy]
ES us-west-2: TTFB 0.32-0.56 s; PC westeurope (SAS): 0.69-1.02 s; AWS eu-central-1 JP2: 0.49-0.73 s; EODC zarr: 0.47-0.93 s. (connect+TLS 0.2-0.8 s dominates => connection reuse and HTTP/2/keepalive matter.)

## 8. Licensing  [SRC]
- Legal notice (EC) https://sentinels.copernicus.eu/documents/247904/690755/Sentinel_Data_Legal_Notice : free, full and open under Reg (EU) 377/2014 and Delegated Reg 1159/2013; rights to reproduce, distribute, communicate to the public, adapt/modify/combine; no warranty; attribution: 'Copernicus Sentinel data [Year]' ; modified: 'Contains modified Copernicus Sentinel data [Year]'. Bulk redistribution of derived products OK with attribution (Art. 8).
- ES collection license "proprietary" with link to the same notice; PC STAC license "proprietary" + same link; AWS registry same. ES/PC infrastructure is best-effort; PC docs say data "free for anyone to use" but throttled.

## 9. Contradictions / open items
- ES docs (collection page) say apply offset if raster:bands offset != 0; maintainers/issues say offset field is wrong on legacy items. Use C1/v2 or pixel-check.
- AWS registry: L1C requester-pays; my anonymous HEAD returned 200.
- PC docs: West Europe/API key => longer tokens; I saw 45 min from non-Azure workstation; issue reporters saw 8 h local vs 45 min Azure Batch.
- EOPF FAQ chunking (1830) and "Zarr v2 only" vs observed 2048 chunks and Zarr3-sharded staging collection; FAQ "rolling 1-year" vs STAC extents 2017->.
- User's "1024x1024 blocks" premise holds only for Earth Search/C1 COG base level.
- Not verified: pixel offsets, CDSE 429/503 behaviour, PC numeric limits, GCS egress price, EOPF zarr3 expiry.
