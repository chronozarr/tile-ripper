//! Admission and fixed-size gzip decoding around established codec libraries.
//! Never let the gzip codec's growable read_to_end process untrusted data.
use crate::{fail, Budget, Result};
use std::{
    io::{Cursor, Read},
    sync::Arc,
};
use zarrs::{
    array::Array,
    storage::{self as s, ReadableStorageTraits},
};

pub(crate) struct Checked<T> {
    inner: Arc<T>,
    prefix: String,
    expected: usize,
    cap: usize,
    codec: Option<String>,
    element: usize,
    blosc_format: u8,
    bitshuffle: bool,
}
/// Standard v3 ordinary chunks, including the inner chunks selected by zarrs.
/// The existing fixed-output admission is shared with v2; no format decoder is
/// replaced. Only gzip/Zstd are bounded before the bytes codec sees the result.
pub(crate) fn array_v3<T: ReadableStorageTraits + 'static>(
    inner: Arc<T>,
    path: &str,
    metadata: serde_json::Value,
    budget: &Budget,
) -> Result<Array<Checked<T>>> {
    array_v3_profile(inner, path, metadata, budget, false)
}

/// Separate rank-four Int8 admission for the named AlphaEarth prototype.
pub(crate) fn array_v3_alphaearth<T: ReadableStorageTraits + 'static>(
    inner: Arc<T>,
    path: &str,
    metadata: serde_json::Value,
    budget: &Budget,
) -> Result<Array<Checked<T>>> {
    array_v3_profile(inner, path, metadata, budget, true)
}

fn array_v3_profile<T: ReadableStorageTraits + 'static>(
    inner: Arc<T>,
    path: &str,
    mut metadata: serde_json::Value,
    budget: &Budget,
    alphaearth: bool,
) -> Result<Array<Checked<T>>> {
    let element = match metadata["data_type"].as_str() {
        Some("int8") if alphaearth => 1,
        Some("uint16") => 2,
        Some("float32") => 4,
        Some("float64" | "int64") => 8,
        _ => return fail("v3 admitted dtypes are UInt16/Float32, Float64/Int64 coordinates"),
    };
    let chunks = metadata["chunk_grid"]["configuration"]["chunk_shape"]
        .as_array()
        .ok_or("missing regular chunk shape")?;
    if (alphaearth && (chunks.len() != 4 || metadata["data_type"] != "int8"))
        || (!alphaearth && (chunks.is_empty() || chunks.len() > 3))
    {
        return fail("unsupported v3 chunk rank");
    }
    let expected = chunks.iter().try_fold(element, |n: usize, c| {
        let size = c
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or("invalid v3 chunk dimension")?;
        n.checked_mul(size).ok_or("v3 chunk size overflow")
    })?;
    if expected > budget.buffer_bytes / 2 {
        return fail("decoded v3 inner chunk exceeds half buffer_bytes");
    }
    if metadata["chunk_grid"]["name"] != "regular"
        || metadata
            .get("storage_transformers")
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return fail("v3 requires a regular grid without storage transformers");
    }
    let codecs = metadata["codecs"].as_array().ok_or("missing v3 codecs")?;
    if codecs.is_empty()
        || codecs.len() > 2
        || codecs[0]["name"] != "bytes"
        || codecs[0]["configuration"]["endian"] != "little"
    {
        return fail("v3 requires little-endian bytes and at most one admitted lossless codec");
    }
    let codec = codecs
        .get(1)
        .map(|v| v["name"].as_str().ok_or("invalid codec name"))
        .transpose()?
        .map(str::to_owned);
    let config = codecs
        .get(1)
        .map(|v| v["configuration"].clone())
        .unwrap_or_default();
    match codec.as_deref() {
        None => {}
        Some("gzip") if config["level"].as_u64().is_some_and(|v| v <= 9) => {}
        Some("zstd") => {}
        Some("blosc")
            if matches!(config["cname"].as_str(), Some("lz4" | "zstd"))
                && matches!(
                    config["shuffle"].as_str(),
                    Some("noshuffle" | "shuffle" | "bitshuffle")
                )
                && config["typesize"].as_u64() == Some(element as u64)
                && config["clevel"].as_u64().is_some_and(|v| v <= 9)
                && config["blocksize"].as_u64() == Some(0) => {}
        _ => return fail("unsupported v3 codec profile: bytes, gzip, Zstd or Blosc LZ4/Zstd"),
    }
    // Instantiate the original chain as well, so the library validates every
    // codec field before removing a codec whose output is checked here.
    let _ = Array::new_with_metadata(
        inner.clone(),
        &format!("/{path}"),
        serde_json::from_value(metadata.clone())?,
    )?;
    if matches!(codec.as_deref(), Some("gzip" | "zstd")) {
        metadata["codecs"].as_array_mut().unwrap().truncate(1);
    }
    let store = Arc::new(Checked {
        inner,
        prefix: format!("{path}/"),
        expected,
        cap: budget.buffer_bytes / 2,
        codec,
        element,
        blosc_format: if config["cname"] == "zstd" { 4 } else { 1 },
        bitshuffle: config["shuffle"] == "bitshuffle",
    });
    Ok(Array::new_with_metadata(
        store,
        &format!("/{path}"),
        serde_json::from_value(metadata)?,
    )?)
}
pub(crate) fn array<T: ReadableStorageTraits + 'static>(
    inner: Arc<T>,
    path: &str,
    mut metadata: serde_json::Value,
    attrs: serde_json::Value,
    budget: &Budget,
) -> Result<Array<Checked<T>>> {
    let element = match metadata["dtype"].as_str() {
        Some("<u2") => 2,
        Some("<f4") => 4,
        Some("<f8" | "<i8") => 8,
        _ => return fail("unsupported native little-endian dtype"),
    };
    let chunks = metadata["chunks"].as_array().ok_or("missing chunks")?;
    if chunks.is_empty() || chunks.len() > 3 {
        return fail("unsupported chunk rank");
    }
    let expected = chunks.iter().try_fold(element, |n: usize, c| {
        let c = c
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or("invalid chunk dimension")?;
        n.checked_mul(c).ok_or("decoded chunk size overflow")
    })?;
    if expected > budget.buffer_bytes / 2 {
        return fail("decoded chunk exceeds half buffer_bytes");
    }
    let codec = metadata["compressor"]["id"].as_str().map(str::to_owned);
    if codec.as_deref() == Some("gzip") {
        metadata["compressor"] = serde_json::Value::Null;
    }
    let checked = Arc::new(Checked {
        inner,
        prefix: format!("{path}/"),
        expected,
        cap: budget.buffer_bytes / 2,
        codec,
        element,
        blosc_format: if metadata["compressor"]["cname"] == "zstd" {
            4
        } else {
            1
        },
        bitshuffle: metadata["compressor"]["shuffle"] == 2,
    });
    let mut ar = Array::new_with_metadata(
        checked,
        &format!("/{path}"),
        serde_json::from_value(metadata)?,
    )?;
    *ar.attributes_mut() = serde_json::from_value(attrs)?;
    Ok(ar)
}
fn gzip(bytes: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut decoder = flate2::bufread::GzDecoder::new(Cursor::new(bytes));
    let mut out = vec![0u8; expected];
    decoder
        .read_exact(&mut out)
        .map_err(|_| "truncated or corrupt gzip chunk")?;
    let mut extra = [0u8; 1];
    if decoder
        .read(&mut extra)
        .map_err(|_| "corrupt gzip checksum/trailer")?
        != 0
    {
        return fail("gzip decoded size exceeds validated chunk size");
    }
    if decoder.into_inner().position() != bytes.len() as u64 {
        return fail("trailing gzip bytes or concatenated members unsupported");
    }
    Ok(out)
}
fn zstd_chunk(bytes: &[u8], expected: usize) -> Result<Vec<u8>> {
    let size =
        zstd::zstd_safe::find_frame_compressed_size(bytes).map_err(|_| "invalid Zstd frame")?;
    if size != bytes.len() {
        return fail("trailing or concatenated Zstd frames unsupported");
    }
    if zstd::zstd_safe::get_frame_content_size(bytes)
        .map_err(|_| "invalid Zstd content size")?
        .is_some_and(|size| size != expected as u64)
    {
        return fail("Zstd frame content size differs from validated chunk size");
    }
    let decoded = zstd::bulk::decompress(bytes, expected)?;
    if decoded.len() != expected {
        return fail("Zstd decoded size differs from validated chunk size");
    }
    Ok(decoded)
}
impl<T: ReadableStorageTraits> ReadableStorageTraits for Checked<T> {
    fn get(&self, key: &s::StoreKey) -> std::result::Result<s::MaybeBytes, s::StorageError> {
        let result = (|| -> Result<s::MaybeBytes> {
            // Local size admission as well as HTTP download admission, before get.
            if self
                .inner
                .size_key(key)?
                .is_some_and(|v| v > self.cap as u64)
            {
                return fail("encoded object exceeds half buffer_bytes");
            }
            let Some(bytes) = self.inner.get(key)? else {
                if [
                    "time/",
                    "x/",
                    "y/",
                    "latitude/",
                    "longitude/",
                    "lat/",
                    "lon/",
                ]
                .contains(&self.prefix.as_str())
                {
                    return fail("required coordinate chunk is missing");
                }
                return Ok(None);
            };
            if bytes.len() > self.cap {
                return fail("encoded object exceeds half buffer_bytes");
            }
            if !key.as_str().starts_with(&self.prefix) {
                return fail("unexpected array object");
            }
            match self.codec.as_deref() {
                Some("gzip") => Ok(Some(gzip(&bytes, self.expected)?.into())),
                Some("zstd") => Ok(Some(zstd_chunk(&bytes, self.expected)?.into())),
                Some("blosc") => {
                    if bytes.len() < 16 {
                        return fail("truncated Blosc header");
                    }
                    let field = |i| {
                        u32::from_le_bytes(bytes[i..i + 4].try_into().expect("checked header"))
                            as usize
                    };
                    // c-blosc's published header: high three flag bits are
                    // compressor format (1 = LZ4), bit 2 is bitshuffle. Plain
                    // memcpy blocks use no compressor. Reject hidden codecs.
                    let flags = bytes[2];
                    if bytes[0] != 2
                        || (flags & 2 == 0
                            && ((flags >> 5) != self.blosc_format
                                || bytes[1] != 1
                                || (flags & 4 != 0) != self.bitshuffle))
                    {
                        return fail("unsupported Blosc stream version/codec/filter");
                    }
                    if field(4) != self.expected
                        || field(12) != bytes.len()
                        || field(8) == 0
                        || field(8) > self.expected
                        || bytes[3] as usize != self.element
                    {
                        return fail("Blosc header disagrees with validated chunk size/type");
                    }
                    Ok(Some(bytes)) // established zarrs/c-blosc validates and decodes
                }
                None => {
                    if bytes.len() != self.expected {
                        return fail("raw chunk size mismatch");
                    }
                    Ok(Some(bytes))
                }
                _ => fail("unsupported codec"),
            }
        })();
        result.map_err(|e| s::StorageError::Other(e.to_string()))
    }
    fn get_partial_many<'a>(
        &'a self,
        key: &s::StoreKey,
        ranges: s::byte_range::ByteRangeIterator<'a>,
    ) -> std::result::Result<s::MaybeBytesIterator<'a>, s::StorageError> {
        let Some(bytes) = self.get(key)? else {
            return Ok(None);
        };
        let mut out = Vec::new();
        for r in ranges {
            let start = r.start(bytes.len() as u64) as usize;
            let end = r.end(bytes.len() as u64) as usize;
            if start > end || end > bytes.len() {
                return Err(s::StorageError::Other("invalid range".into()));
            }
            out.push(Ok(bytes.slice(start..end)));
        }
        Ok(Some(Box::new(out.into_iter())))
    }
    fn size_key(&self, key: &s::StoreKey) -> std::result::Result<Option<u64>, s::StorageError> {
        self.inner.size_key(key)
    }
    fn supports_get_partial(&self) -> bool {
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn encoded(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }
    #[test]
    fn zstd_fixed_output_rejects_size_bombs_crc_and_trailing_frames() {
        let valid = zstd::bulk::compress(&[7; 32], 1).unwrap();
        assert_eq!(zstd_chunk(&valid, 32).unwrap(), vec![7; 32]);
        assert!(zstd_chunk(&valid[..valid.len() - 1], 32).is_err());
        assert!(zstd_chunk(&valid, 31).is_err());
        let mut two = valid.clone();
        two.extend_from_slice(&valid);
        assert!(zstd_chunk(&two, 32).is_err());
        let bomb = zstd::bulk::compress(&vec![0; 16 * 1024 * 1024], 1).unwrap();
        assert!(zstd_chunk(&bomb, 32).is_err());
    }
    #[test]
    fn fixed_allocation_rejects_bombs_truncation_crc_and_trailing_members() {
        let valid = encoded(&[7; 32]);
        assert_eq!(gzip(&valid, 32).unwrap(), vec![7; 32]);
        assert!(gzip(&valid[..valid.len() - 3], 32).is_err());
        let mut bad = valid.clone();
        bad[12] ^= 0xff;
        assert!(gzip(&bad, 32).is_err());
        assert!(gzip(&encoded(&vec![0; 16 * 1024 * 1024]), 32).is_err());
        let mut bad = valid.clone();
        bad.extend_from_slice(&valid);
        assert!(gzip(&bad, 32).is_err());
        assert!(gzip(&valid, 64).is_err());
    }
}
