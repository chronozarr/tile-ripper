# Draft: zarrs 0.22.10 confirmation of resolved gzip/sharding issue #444

**Local draft only. Already fixed upstream; do not file a duplicate issue.**
This can be supplied as older-version/backport evidence if needed. No issue or
comment has been submitted.

zarrs 0.22.10 has the same invalid gzip bound documented in
[#444](https://github.com/zarrs/zarrs/issues/444). A standalone MemoryStore write
returns `CodecError(Other("Sharding did not allocate a large enough buffer"))`.
The same program passes with released zarrs 0.23.14, including full readback.

Exact dependencies: zarrs 0.22.10, flate2 1.1.10, zlib-rs 0.6.8,
miniz_oxide 0.9.1, serde_json 1.0.145, Rust 1.92.0 (ded5c06cf), macOS 26.6.2
arm64. [Full lockfile](../../bench/zarr-v3-repro/Cargo.lock).

Minimal manifest (the attached lockfile pins transitive dependencies):

```toml
[package]
name = "zarr-v3-repro"
version = "0.1.0"
edition = "2021"

[dependencies]
zarrs = { version = "=0.22.10", default-features = false, features = ["gzip", "sharding"] }
flate2 = { version = "=1.1.10", features = ["zlib-rs"] }
serde_json = "=1.0.145"
```

Source `src/main.rs` (for this version, omit the fixed-release compatibility
imports/attribute from the shared source):

```rust
use serde_json::json;
use std::{borrow::Cow, sync::Arc};
use zarrs::array::codec::{BytesToBytesCodecTraits, CodecOptions};
use zarrs::{
    array::{codec::GzipCodec, Array, BytesRepresentation},
    storage::store::MemoryStore,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let edge: usize = std::env::args().nth(1).unwrap_or("256".into()).parse()?;
    let mut state = 1u64;
    let mut values: Vec<u16> = (0..4 * edge * edge)
        .map(|_| {
            state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            ((z ^ (z >> 31)) >> 48) as u16
        })
        .collect();
    if let Some(path) = std::env::args().nth(2) {
        let input = std::fs::read(path)?;
        assert_eq!(input.len(), 8 * edge * edge);
        values = input
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
    }
    let bytes: Vec<u8> = values[..edge * edge]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let mut options = CodecOptions::default();
    options.set_concurrent_target(1);
    let gzip = GzipCodec::new(1)?;
    let bound = gzip.encoded_representation(&BytesRepresentation::FixedSize(bytes.len() as u64));
    let encoded = gzip.encode(Cow::Borrowed(&bytes), &options)?;
    println!(
        "{}",
        json!({"operation":"gzip encode", "input_bytes":bytes.len(), "declared_bound":bound.size(), "actual_bytes":encoded.len()})
    );
    let metadata = json!({
        "zarr_format":3,"node_type":"array","shape":[1,2*edge,2*edge],"data_type":"uint16","fill_value":0,
        "chunk_grid":{"name":"regular","configuration":{"chunk_shape":[1,2*edge,2*edge]}},
        "chunk_key_encoding":{"name":"v2","configuration":{"separator":"."}},
        "codecs":[{"name":"sharding_indexed","configuration":{
            "chunk_shape":[1,edge,edge],
            "codecs":[{"name":"bytes","configuration":{"endian":"little"}},{"name":"gzip","configuration":{"level":1}}],
            "index_codecs":[{"name":"bytes","configuration":{"endian":"little"}}],"index_location":"end"
        }}]
    });
    let array = Array::new_with_metadata(
        Arc::new(MemoryStore::new()),
        "/a",
        serde_json::from_value(metadata)?,
    )?;
    let result = array.store_chunk_elements_opt(&[0, 0, 0], &values, &options);
    println!(
        "{}",
        json!({"operation":"store_chunk_elements_opt", "edge":edge, "result":format!("{result:?}")})
    );
    result?;
    assert_eq!(array.retrieve_chunk_elements::<u16>(&[0, 0, 0])?, values);
    Ok(())
}
```

Run `cargo run --locked`. Expected: a complete lossless round trip. Actual:
the logged gzip size exceeds its declared bound, and the shard write returns
the error above. No filesystem, source acquisition, application scheduler or
memory-budget logic participates. With miniz_oxide alone the deterministic
noise also fails; which real datasets trigger the error depends on the backend.

Real UInt16 B08 data reproduced this at gzip level 1 with inner `[1,256,256]`,
outer `[1,512,512]`, a bytes-only 64-byte end index and concurrency 1. The bound
reserves 524,504 bytes total; actual compressed chunk payloads alone total
532,841 bytes. Raw shards and unsharded gzip pass. Changing concurrency or
raising the application budget does not fix the bound.

The appropriate resolution is the existing [#446 fix](https://github.com/zarrs/zarrs/pull/446)
and [0.23.14 release](https://github.com/zarrs/zarrs/releases/tag/zarrs-v0.23.14).
[Fixed manifest/lock](../../bench/zarr-v3-repro/fixed/Cargo.toml) and the
[392-case receipt](results.json) provide the controlled comparison. The
fixed manifest enables crc32c to compile that release's sharding builder;
explicit array metadata still has a bytes-only index.
