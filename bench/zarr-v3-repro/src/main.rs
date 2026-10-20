#![cfg_attr(feature = "upstream-fixed", allow(deprecated))]
use serde_json::json;
use std::{borrow::Cow, sync::Arc};
#[cfg(not(feature = "upstream-fixed"))]
use zarrs::array::codec::{BytesToBytesCodecTraits, CodecOptions};
#[cfg(feature = "upstream-fixed")]
use zarrs::array::{BytesToBytesCodecTraits, CodecOptions};
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
