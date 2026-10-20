//! Isolate codec, shape, write pattern and concurrency; no application budgets.
#![cfg_attr(feature = "upstream-fixed", allow(deprecated))]
use serde_json::{json, Value};
use std::{
    borrow::Cow,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
#[cfg(not(feature = "upstream-fixed"))]
use zarrs::array::codec::{BytesToBytesCodecTraits, CodecOptions};
#[cfg(feature = "upstream-fixed")]
use zarrs::array::{ArraySubset, BytesToBytesCodecTraits, CodecOptions};
#[cfg(not(feature = "upstream-fixed"))]
use zarrs::array_subset::ArraySubset;
use zarrs::{
    array::{codec::GzipCodec, Array, ArrayBytes, BytesRepresentation},
    storage::store::MemoryStore,
};

type Error = Box<dyn std::error::Error>;

fn noise(count: usize) -> Vec<u16> {
    let mut state = 1u64;
    (0..count)
        .map(|_| {
            state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            ((z ^ (z >> 31)) >> 48) as u16
        })
        .collect()
}

fn probe(
    shape: Vec<u64>,
    inner: Vec<u64>,
    data: Vec<u8>,
    dtype: &str,
    sharded: bool,
    level: Option<u32>,
    concurrency: usize,
    incremental: bool,
    partial: bool,
    index: &str,
    pattern: &str,
) -> Result<Value, Error> {
    let bytes = json!({"name":"bytes","configuration":{"endian":"little"}});
    let mut codecs = vec![bytes.clone()];
    if let Some(level) = level {
        codecs.push(json!({"name":"gzip","configuration":{"level":level}}));
    }
    let outer = if sharded {
        shape.clone()
    } else {
        inner.clone()
    };
    let metadata = json!({
        "zarr_format":3,"node_type":"array","shape":shape,"data_type":dtype,"fill_value":0,
        "chunk_grid":{"name":"regular","configuration":{"chunk_shape":outer}},
        "chunk_key_encoding":{"name":"v2","configuration":{"separator":"."}},
        "codecs":if sharded {json!([{"name":"sharding_indexed","configuration":{
            "chunk_shape":inner,"codecs":codecs,"index_codecs":[bytes],"index_location":index}}])} else {json!(codecs)}
    });
    let array = Array::new_with_metadata(
        Arc::new(MemoryStore::new()),
        "/a",
        serde_json::from_value(metadata.clone())?,
    )?;
    let mut options = CodecOptions::default();
    options.set_concurrent_target(concurrency);
    options.set_experimental_partial_encoding(partial);
    let full = ArrayBytes::new_flen(data.clone());
    let counts: Vec<_> = shape.iter().zip(&inner).map(|(s, c)| s / c).collect();
    let mut subsets = Vec::new();
    let mut encoded_sizes = Vec::new();
    for linear in 0..counts.iter().product::<u64>() {
        let mut rem = linear;
        let mut start = vec![0; shape.len()];
        for d in (0..shape.len()).rev() {
            start[d] = (rem % counts[d]) * inner[d];
            rem /= counts[d];
        }
        let subset = ArraySubset::new_with_start_shape(start, inner.clone())?;
        let chunk = full
            .extract_array_subset(&subset, &shape, array.data_type())?
            .into_fixed()?;
        if let Some(level) = level {
            let gzip = GzipCodec::new(level)?;
            let encoded = gzip.encode(Cow::Borrowed(&chunk), &options)?;
            encoded_sizes.push(json!({"input_bytes":chunk.len(),"bound":gzip.encoded_representation(&BytesRepresentation::FixedSize(chunk.len() as u64)).size(),"actual":encoded.len()}));
        }
        subsets.push((subset, chunk.into_owned()));
    }
    let mut completed = 0;
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<(), Error> {
        if incremental {
            for (subset, chunk) in &subsets {
                array.store_array_subset_opt(
                    subset,
                    ArrayBytes::new_flen(chunk.as_slice()),
                    &options,
                )?;
                completed += 1;
            }
        } else {
            array.store_array_subset_opt(&array.subset_all(), full, &options)?;
            completed = 1;
        }
        let decoded: ArrayBytes = array.retrieve_array_subset_opt(&array.subset_all(), &options)?;
        assert_eq!(decoded.into_fixed()?.as_ref(), data);
        Ok(())
    }));
    let result = match outcome {
        Ok(Ok(())) => "roundtrip_ok".to_string(),
        Ok(Err(e)) => format!("error: {e:?}"),
        Err(e) => format!(
            "panic: {}",
            e.downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| e.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic")
        ),
    };
    Ok(
        json!({"metadata":metadata,"pattern":pattern,"concurrency":concurrency,"incremental":incremental,
        "experimental_partial_encoding":partial,"completed_writes":completed,"gzip_chunks":encoded_sizes,"result":result}),
    )
}

fn main() -> Result<(), Error> {
    // Expected panics are captured as results, without noisy default stderr hooks.
    std::panic::set_hook(Box::new(|_| {}));
    let mut rows = Vec::new();
    for edge in [16, 32, 64, 96, 128, 192, 256, 512] {
        let shape = vec![1, 2 * edge, 2 * edge];
        let inner = vec![1, edge, edge];
        let data: Vec<_> = noise((4 * edge * edge) as usize)
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        for (sharded, level) in [
            (true, None),
            (true, Some(1)),
            (false, None),
            (false, Some(1)),
        ] {
            rows.push(probe(
                shape.clone(),
                inner.clone(),
                data.clone(),
                "uint16",
                sharded,
                level,
                1,
                false,
                false,
                "end",
                "splitmix64 seed 1",
            )?);
        }
    }
    for inner in [
        vec![65536],
        vec![256, 256],
        vec![1, 256, 256],
        vec![1, 128, 512],
    ] {
        let mut shape = inner.clone();
        let n = shape.len();
        if n == 1 {
            shape[0] *= 4;
        } else {
            shape[n - 1] *= 2;
            shape[n - 2] *= 2;
        }
        let data: Vec<_> = noise(262144).iter().flat_map(|v| v.to_le_bytes()).collect();
        for concurrency in [1, 2, 4] {
            for incremental in [false, true] {
                rows.push(probe(
                    shape.clone(),
                    inner.clone(),
                    data.clone(),
                    "uint16",
                    true,
                    Some(1),
                    concurrency,
                    incremental,
                    false,
                    "end",
                    "splitmix64 seed 1",
                )?);
            }
        }
    }
    let shape = vec![1, 512, 512];
    let inner = vec![1, 256, 256];
    for pattern in ["constant 1", "structured", "splitmix64 seed 1"] {
        let values = match pattern {
            "constant 1" => vec![1u16; 262144],
            "structured" => (0..262144)
                .map(|i| ((i % 512 * 17 + i / 512 * 31) % 65535 + 1) as u16)
                .collect(),
            _ => noise(262144),
        };
        let data: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        for level in [0, 1, 5, 9] {
            rows.push(probe(
                shape.clone(),
                inner.clone(),
                data.clone(),
                "uint16",
                true,
                Some(level),
                1,
                false,
                false,
                "end",
                pattern,
            )?);
        }
    }
    // Both masks (uint8) are exercised independently of the value dtype.
    for data in [
        vec![1u8; 262144],
        noise(262144).iter().map(|v| *v as u8).collect(),
    ] {
        let pattern = if data.iter().all(|v| *v == 1) {
            "mask constant 1"
        } else {
            "uint8 noise"
        };
        rows.push(probe(
            shape.clone(),
            inner.clone(),
            data,
            "uint8",
            true,
            Some(1),
            1,
            true,
            false,
            "end",
            pattern,
        )?);
    }
    for index in ["start", "end"] {
        let data: Vec<_> = noise(262144).iter().flat_map(|v| v.to_le_bytes()).collect();
        rows.push(probe(
            shape.clone(),
            inner.clone(),
            data.clone(),
            "uint16",
            true,
            Some(1),
            1,
            true,
            true,
            index,
            "splitmix64 seed 1",
        )?);
        // Small level-0 case exercises the distinct trailing-index panic.
        rows.push(probe(
            vec![65536],
            vec![32768],
            vec![1; 65536],
            "uint8",
            true,
            Some(0),
            1,
            false,
            false,
            index,
            "stored blocks constant 1",
        )?);
    }
    if let Some(dir) = std::env::args().nth(1) {
        for i in 0..4 {
            let data = std::fs::read(format!("{dir}/sentinel-{i}.u16"))?;
            for (sharded, level) in [(true, None), (true, Some(1)), (false, Some(1))] {
                for concurrency in [1, 4] {
                    rows.push(probe(
                        shape.clone(),
                        inner.clone(),
                        data.clone(),
                        "uint16",
                        sharded,
                        level,
                        concurrency,
                        true,
                        false,
                        "end",
                        &format!("sentinel-{i}"),
                    )?);
                }
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"backend":if cfg!(feature="native-backend"){"zlib-rs"}else{"miniz_oxide"},"rows":rows})
        )?
    );
    Ok(())
}
