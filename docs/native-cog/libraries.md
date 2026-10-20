# Library assessment

Audited published source packages on 2026-10-19, rather than treating README
capability lists as tested guarantees. Only the selected stack was exercised by
Tile-Ripper's science/fault/real-fixture matrix. Other libraries below were source
inspections, not comparative performance benchmarks.

| Library | Actual useful implementation | Material gaps for this contract | Decision |
|---|---|---|---|
| `async-tiff 0.4.0` | Separate async metadata fetching and tile decoding, typed UInt16 arrays, classic/BigTIFF, GeoKeys, tile range merging, object_store adapter, custom decoder registry. Deflate/LZW/JPEG/ZSTD built in; optional codecs. | `ReqwestReader::make_range_request` checks HTTP error status but accepts a 200 response, does not validate Content-Range/length or bind ETags, and buffers the response. `ImageFileDirectoryReader::read` allocates from unbounded tag count; tag parsing has `expect`/`unwrap` for unknown type/size overflow. Default Deflate `read_to_end` is unbounded. LERC decoder explicitly discards its validity mask. Tile-level API still needs window assembly, calibration and mask semantics. | Strong async architecture, but more adapter hardening than the selected family. Do not use its default transport/decoder against untrusted inputs unchanged. |
| `geotiff-reader 0.9.0` + `tiff-reader`/`geotiff-core` | Typed storage-domain windows and band windows; classic/BigTIFF, both byte orders, tiled/stripped and planar layouts. Explicit parser budgets, bounded encoded/decompressed blocks/output buffers, bounded decoded-block LRU. GeoKey/CRS/tiepoint/transform/nodata handling. HTTP implementation checks 206/length, caches/coalesces ranges and shares in-flight misses; caller can provide headers/client. Native async wrapper sends decode to blocking tasks. | HTTP code does not independently bind/compare ETag on every response; caller must supply snapshot policy. Header chunking can overfetch. Full validity and calibration do not map automatically to Tile-Ripper's contract. `GeoTiffFile::from_tiff` is crate-private, so a custom versioned TiffSource cannot be injected at that layer. Broad upstream profile support is not evidence that every profile meets our science contract. | Best fit: use its underlying `tiff-reader=0.9.0` decoder and `geotiff-core=0.9.0` through a narrow bounded/versioned adapter. No TIFF parser, predictor or compression decoder was implemented here. |
| `cogrs 0.0.4` | RangeReader abstraction, local/HTTP/S3, cached tiles, TIFF/image codecs, proj4rs and CRS definitions. | Original HTTP reader accepts any successful status and buffers the body; does not check exact range, size or ETag. Point/XYZ/reprojection architecture is broader than needed; requires object_store/aws, image and projection dependencies. | Less suitable for scientific acquisition and a small dependency surface. |
| `cogrs-eg 0.2.0` (Earth Genome fork) | Substantial later async/concurrent range work, request coalescing, shared tiles, BigTIFF, version-aware HTTP infrastructure; distinct from the original published crate. | `CogReader::decode_tile`, cache and public `read_tile*` use `Vec<f32>`; sparse tiles use NaN. Every UInt16 integer is exactly representable in f32, but this still changes our native dtype/mask contract and doubles tile payload memory before conversion. CRS tiling/reprojection/overview selection is unnecessary scope here. | More mature transport than the original; not selected for native UInt16 streaming. |

Primary sources:
[async-tiff source](https://docs.rs/crate/async-tiff/0.4.0/source/src/),
[geotiff-reader source](https://docs.rs/crate/geotiff-reader/0.9.0/source/src/),
[tiff-reader source](https://docs.rs/crate/tiff-reader/0.9.0/source/src/),
[geotiff-core source](https://docs.rs/crate/geotiff-core/0.9.0/source/src/),
[cogrs source](https://docs.rs/crate/cogrs/0.0.4/source/src/),
[cogrs-eg source](https://docs.rs/crate/cogrs-eg/0.2.0/source/src/).
The exact downloaded files and hashes are recorded in `library-audit.json`.

## Dependencies and maturity

The selected decoder disables its default `rayon`, JPEG, ZSTD and WebP features.
Its active path uses `flate2` with Rust `zlib-rs`, `weezl` LZW, ndarray, parking_lot,
LRU and Rust GeoTIFF metadata types. Its manifest also pulls pure Rust LERC crates,
although this adapter rejects LERC. `memmap2` is transitive, but this adapter uses
safe FileSource random reads and never calls the unsafe mmap entrypoints.
`roxmltree` parses bounded GDAL calibration XML (DTD disabled by default).
No libtiff, PROJ, GDAL or object_store dependency is introduced by the COG path.

Async-tiff's default features enable object_store and reqwest. Its unconditional
`zstd` dependency uses native zstd bindings; optional JPEG2000/WebP/LERC dependencies
need separate native/build audits. Object_store offers conditional GetOptions, but
Tile-Ripper must actually wire those conditions through each reader; having an
object-store dependency alone does not establish source-version safety.

The original cogrs manifest unconditionally enables object_store HTTP/AWS and
includes image, zstd, proj4rs and CRS definitions. The fork retains a larger,
tiler-oriented stack. Neither a pure-Rust projection claim nor codec inventory
makes their higher-level numeric contract equivalent to Tile-Ripper's.

All candidates are pre-1.0 and actively evolving. The selected package's parser,
cache, filters and budget tests are concrete maintenance evidence; they are not
proof of long-term stability or an independent security audit. Pin versions,
retain the frozen science/fault suite and review upstream changes before updating.
No claim is made that the selected narrow adapter supports everything upstream can
parse. Its stricter exclusions are intentional and enumerated in the report.
