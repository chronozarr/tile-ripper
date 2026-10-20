# Experimental Zarr v3 diagnosis — 20 October 2026

The 12 medium/large failures are caused by an **upstream gzip encoded-size
underestimate in zarrs 0.22.10**, consumed by its bounded shard encoder. The
released **zarrs 0.23.14 fixes this defect**. Our independent in-memory program
reproduces the same error on both deterministic generated data and the captured
Sentinel B08 values; the fixed release passes every corresponding probe.

Keep the validated uncompressed v2 default and the existing publication
protocol. Further v3 work is reasonable in a separate milestone using the
upstream fix. The present pinned gzip/sharding candidate remains unsuitable
for general use. This investigation changes no runtime behavior, dependencies,
input support, streaming API or scientific output contract. It corrects one
misleading synchronization comment and adds research artifacts.

## Failure and exact operation

The original receipt contains 12 errors: 512-square and 2048-square windows,
warm local and shaped 8 ms / 100 Mbit/s HTTP, three repetitions each. Every
failure returns:

```text
CodecError(Other("Sharding did not allocate a large enough buffer"))
```

The application calls `values.store_array_subset_elements_opt` for each source
delivery. With experimental partial encoding disabled, zarrs reads/modifies
and re-encodes the enclosing shard. Its `ShardingCodec::encode` chooses
`encode_bounded` from the inner codec's declared `BoundedSize`. The guard at
[zarrs 0.22.10 source, lines 650–656](https://github.com/zarrs/zarrs/blob/0563c6421781eba488413f3b2e34047162087e3e/zarrs/src/array/codec/array_to_bytes/sharding/sharding_codec.rs#L650-L656)
rejects a chunk whose encoded end exceeds that reservation. This is a library
size-bound error, not an allocator failure or a Tile-Ripper memory-budget error.

The pinned gzip bound for an input of N bytes is:

```text
N + 18 + 5 * ceil(N / 32768)
```

That assumed DEFLATE block/encoding overhead is not a valid worst-case bound
for the encoders used by flate2. Gzip output can exceed it. Encoding into a
growable vector succeeds; treating the estimate as a shard allocation limit
fails. This also explains why the unsharded compressed v2 writer passes.

The actual native dependency graph matters: `tiff-reader 0.9.0` enables
`flate2/zlib-rs`, which selects zlib-rs over the otherwise enabled miniz_oxide
backend through Cargo feature unification. The standalone default miniz_oxide
build passes the real B08 windows, but fails generated noise; the standalone
zlib-rs build reproduces the real-window errors. This is an upstream bound
defect exposed by a valid dependency configuration, not a reason to change the
validated COG decoder's compression backend.

Exact relevant versions (full package checksums and graphs are in the two
standalone lockfiles and [receipt](results.json)):

| Dependency | Pinned reproduction | Fixed-release control |
| --- | --- | --- |
| zarrs | 0.22.10 | 0.23.14 |
| flate2 | 1.1.10 | 1.1.10 |
| zlib-rs | 0.6.8 | 0.6.8 |
| miniz_oxide | 0.9.1 | 0.9.1 |
| rayon | 1.12.0 | 1.12.0 |
| zarrs_storage | 0.4.5 | 0.4.5 |
| serde_json | 1.0.145 | 1.0.145 |
| Compiler | Rust 1.92.0, ded5c06cf | same |

The original metadata is valid standard v3: values `uint16`, masks `uint8`,
little-endian bytes then gzip level 1 inside `sharding_indexed`; inner chunks
`[1,256,256]`, outer shards `[1,512,512]`; regular grid, v2-style dot-separated
keys, fill 0, end index encoded as little-endian bytes without CRC32C. Array
shapes are `[1,512,512]` or `[1,2048,2048]`. Coordinates/time/scalar metadata
are unsharded bytes. The sink sets codec concurrency to 1 and leaves
experimental partial encoding false. [Application metadata construction](../../src/scientific_output.rs).

## Connection to the captured failures

The two windows start at source column 6144, row 1024. Their first 512-square
region is identical. Independent GDAL reads of the frozen original ranges
supply the standalone program; source ETags and decoded-input SHA-256 values
are recorded in `results.json`. No raw imagery is committed.

Each 256-square UInt16 inner chunk has 131,072 decoded bytes; zarrs declares
131,110 encoded bytes. Four chunks plus a 64-byte index reserve **524,504 bytes**.
The actual gzip level-1 lengths with the native zlib-rs backend are:

| Observation | Inner chunk encoded lengths, C order | Sum before index | Incremental result |
| --- | --- | ---: | --- |
| 2025-01-02 B04 | 126330, 124710, 125902, 126182 | 503124 | passes |
| 2025-01-02 B08 | 133038, 133033, 133485, 133285 | 532841 | fourth delivery fails |
| 2025-01-12 B04 | 130331, 129224, 128695, 129106 | 517356 | passes |
| 2025-01-12 B08 | 133209, 133019, 133375, 133248 | 532851 | fourth delivery fails |

Initially unwritten inner chunks are fill and omitted. Three B08 chunks fit
within the allocation reserved for four. Once the fourth is populated, even
the chunk payloads alone exceed the entire allocation, triggering the exact
recorded guard. Both B08 dates reproduce this without Tile-Ripper, filesystem
I/O, HTTP, a scheduler or an application memory budget. The original 12 stderr
records do not name which B08 observation reached that condition first;
concurrent delivery can change that order. The failure cause does not depend
on that attribution.

Fresh application replays reproduce the same medium/large error with both
**16 MiB and 64 MiB** budgets. **8 MiB** is independently rejected by the
existing admission check before staging, because the declared writer
reservation is 9.375 MiB. Raising the budget cannot repair a wrong codec size
bound. These six replays publish nothing and clean their owned staging.

## Deterministic reproducer and affected profiles

The [standalone crate](../../bench/zarr-v3-repro/Cargo.toml) depends on zarrs,
flate2 and serde_json, uses `MemoryStore`, and contains no Tile-Ripper code or
replacement encoder. Its [small main program](../../bench/zarr-v3-repro/src/main.rs)
generates a fixed SplitMix64 seed-1 UInt16 array, measures gzip's declared and
actual size, and writes one shard. No source data is required.

From the repository root (the first command is expected to exit 1):

```sh
cargo run --locked --manifest-path bench/zarr-v3-repro/Cargo.toml --features native-backend --bin zarr-v3-repro
cargo run --locked --manifest-path bench/zarr-v3-repro/fixed/Cargo.toml --features native-backend --bin zarr-v3-repro
```

Use `--target-dir target/zarr-v3-repro` and `target/zarr-v3-repro-fixed` to keep
build products in the root ignored target directory. The fixed manifest reuses
the same source with only API import compatibility selected. Its `crc32c`
feature is required to compile that release's sharding builder even though our
explicit metadata still uses a bytes-only index. Root dependencies are unchanged.

[Matrix source](../../bench/zarr-v3-repro/src/bin/matrix.rs) exercises:

- UInt16 inner edges 16, 32, 64, 96, 128, 192, 256 and 512, sharded/unsharded,
  compressed/uncompressed v3, plus UInt8 mask/noise controls.
- Equal-size 1D, 2D, 3D and rectangular chunks; codec concurrency 1, 2 and 4;
  whole writes and successive inner-chunk updates.
- Constant, structured and deterministic noisy inputs; gzip levels 0, 1, 5
  and 9; both index locations; the existing partial-encoding option.
- With the optional local mirror, all four real windows under both backends,
  uncompressed/sharded, gzip/sharded and gzip/unsharded, at concurrency 1 and 4.

| Version/backend | Exact round trips | Codec errors | Caught index-write panics | Total |
| --- | ---: | ---: | ---: | ---: |
| 0.22.10 / miniz_oxide | 62 | 30 | 6 | 98 |
| 0.22.10 / zlib-rs | 56 | 41 | 1 | 98 |
| 0.23.14 / miniz_oxide | 98 | 0 | 0 | 98 |
| 0.23.14 / zlib-rs | 98 | 0 | 0 | 98 |

All 24 uncompressed cases per profile pass, including sharded v3. All
unsharded gzip cases pass. For the noisy edge sweep with zlib-rs, all eight
sharded gzip sizes fail. With miniz_oxide, edges through 128 pass while 192,
256 and 512 fail. Equivalent-shape noisy writes fail at all tested
concurrencies in the pinned version. Serial execution is sufficient; there
is no evidence of a concurrency race. Constant/structured level-1 controls
pass, explaining the earlier successful structured large-memory tests.

The trailing-index variant is a second manifestation of the same defect:
two 32,768-byte level-0 chunks encode to 32,796 bytes each, but the old bound
is 32,791. With a bytes-only end index, the index write needs 65,624 bytes
against 65,614 reserved and panics. The matrix catches and reports it. The
original 12 failures are the returned-error variant. Neither is safe to ignore.

Existing experimental partial encoding passes the two noisy probes because
it avoids that whole-shard preallocation path. It remains an experimental
option, not an adopted workaround. These probes do not establish its
performance, durability, independent interoperability or memory suitability.
The sweeps identify affected tested profiles, not an exhaustive safe-size or
safe-compression-level boundary.

## Upstream status and engineering decision

[Issue #444](https://github.com/zarrs/zarrs/issues/444) already documents this
size-bound failure and both manifestations. [PR #446](https://github.com/zarrs/zarrs/pull/446),
merged as `4e80aee5209445701caccb5bb6db5ff9df4fcdfb`, corrects gzip's bound
and adds defensive shard bounds checks. The fix is in the published
[0.23.14 release](https://github.com/zarrs/zarrs/releases/tag/zarrs-v0.23.14),
tag commit `f2c77ea616507cd5837f32eafb4f4bf317db8896`. Its bound is
`N + ceil(N/8) + ceil(N/64) + 5 + 18`, giving 149,527 bytes for a 131,072-byte
input. Our fixed-release controls confirm the repair on the same inputs.

An **upstream dependency fix is appropriate**, with a deliberate upgrade and
integration validation in a separate milestone. There is no current metadata,
chunk divisibility or application-budget correction that fixes this defect.
Changing shape, compression level, backend or partial encoding may dodge some
cases but does not fix the invalid bound. Do not implement a replacement codec.
A [local issue/confirmation draft](upstream-issue-draft.md) includes source and
exact dependencies; do not file a duplicate of the resolved upstream issue.
Nothing was sent upstream.

V3 output is reasonable to prototype further **after using the upstream fix**,
under the existing experimental gate. This is not production qualification:
the upgrade changes library APIs/dependencies and allocation bounds; renew
independent scientific/reader, failure/publication, memory and performance
validation, including shard rewrite amplification and any checksum decision.
The standalone fixed-release tests alone do not validate Tile-Ripper with that
upgrade. No v3 input or release/default change is included here.

## Preservation and durability audit

The original production source digest, all three measured binary hashes,
eight independent scientific/fault/memory/regression receipts and 144 benchmark
rows were checked against the committed receipts. Large-window medians remain
4.845936 → 0.526623 s (**9.2019×**) locally and 7.565564 → 3.293668 s
(**2.2970×**) on the shaped link. Four transaction unit tests pass again.

The only source edit corrects a comment claiming Rust's `sync_all` silently
falls back to weaker synchronization: the tested Rust 1.92 implementation
already propagates `F_FULLFSYNC` errors. All executable source and the root
manifest/lockfile are otherwise identical to `0d0cc18`; the reproduction
harness checks that exact comment-only difference.

The [synchronization audit](synchronization-audit.md) separates atomic
application publication, conditional filesystem crash consistency and observed
syscalls. **No physical hardware flush or power-loss survival was measured.**
The v2 implementation and publication ordering remain adopted without further
speed changes.

Run the stdlib-only matrix directly with `cargo run ... --bin matrix`. For the
complete local receipt, including retained binary/source checks, original-range
extraction and six budget replays:

```sh
prototype-output/durable-output/reader3/bin/python bench/zarr_v3_diagnosis.py --frozen
```

Without `--frozen`, the wrapper runs 74 generated-data profiles per build; it
still checks the retained local v2 benchmark binaries/receipts. Output goes to
ignored `prototype-output/zarr-v3-diagnosis/`. The versioned `results.json`
contains compact copies of the 392 probe outcomes and source/lock hashes.
