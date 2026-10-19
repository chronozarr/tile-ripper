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
                if ["time/", "x/", "y/", "latitude/", "longitude/"].contains(&self.prefix.as_str())
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
