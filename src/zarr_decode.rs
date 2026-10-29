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
    #[cfg_attr(not(feature = "icechunk"), allow(dead_code))]
    grib: Option<GribConfig>,
}
#[cfg_attr(not(feature = "icechunk"), allow(dead_code))]
struct GribConfig {
    shape: [usize; 2],
    north_up: bool,
    adjust_longitude_range: bool,
    scale: f64,
    offset: f64,
}
pub(crate) fn is_grib(metadata: &serde_json::Value) -> bool {
    metadata["codecs"]
        .as_array()
        .is_some_and(|codecs| codecs.iter().any(|codec| codec["name"] == "gribberish"))
}
fn grib_config(metadata: &serde_json::Value) -> Result<Option<GribConfig>> {
    if !is_grib(metadata) {
        return Ok(None);
    }
    if !cfg!(feature = "icechunk") {
        return fail("build with icechunk to decode virtual GRIB chunks");
    }
    let codecs = metadata["codecs"]
        .as_array()
        .ok_or("missing GRIB codec chain")?;
    if !(codecs.len() == 1 || (codecs.len() == 2 && codecs[0]["name"] == "scale_offset"))
        || codecs.last().unwrap()["name"] != "gribberish"
        || metadata["data_type"] != "float64"
    {
        return fail(
            "virtual GRIB admits Float64 gribberish with optional preceding scale_offset only",
        );
    }
    let config = &codecs.last().unwrap()["configuration"];
    if config.as_object().is_none_or(|values| {
        values
            .keys()
            .any(|key| !matches!(key.as_str(), "var" | "north_up" | "adjust_longitude_range"))
    }) || config.get("var").is_some_and(|value| {
        value
            .as_str()
            .is_none_or(|v| matches!(v, "latitude" | "longitude"))
    }) {
        return fail("unsupported GRIB configuration or coordinate-mode codec");
    }
    let flag = |name: &str| -> Result<bool> {
        match config.get(name) {
            None => Ok(false),
            Some(value) => value
                .as_bool()
                .ok_or_else(|| "GRIB adjustment flags must be boolean".into()),
        }
    };
    let mut scale = 1.;
    let mut offset = 0.;
    if codecs.len() == 2 {
        let config = &codecs[0]["configuration"];
        if config.as_object().is_none_or(|values| {
            values
                .keys()
                .any(|key| !matches!(key.as_str(), "scale" | "offset"))
        }) {
            return fail("unsupported scale_offset configuration");
        }
        scale = config
            .get("scale")
            .map_or(Some(1.), |v| v.as_f64())
            .ok_or("invalid storage scale")?;
        offset = config
            .get("offset")
            .map_or(Some(0.), |v| v.as_f64())
            .ok_or("invalid storage offset")?;
        if !scale.is_finite() || scale == 0. || !offset.is_finite() {
            return fail("storage scale_offset must be finite and invertible");
        }
    }
    let dims = metadata["chunk_grid"]["configuration"]["chunk_shape"]
        .as_array()
        .ok_or("missing GRIB shape")?;
    if dims.len() != 3 || dims[0] != 1 {
        return fail("GRIB chunks require one complete time/grid slice");
    }
    let shape = [
        dims[1]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("invalid GRIB height")?,
        dims[2]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("invalid GRIB width")?,
    ];
    Ok(Some(GribConfig {
        shape,
        scale,
        offset,
        north_up: flag("north_up")?,
        adjust_longitude_range: flag("adjust_longitude_range")?,
    }))
}
#[cfg(feature = "icechunk")]
fn packed_u32(bytes: &[u8], start: usize, width: usize) -> Result<u32> {
    if width > 32
        || start
            .checked_add(width)
            .is_none_or(|end| end > bytes.len().saturating_mul(8))
    {
        return fail("GRIB descriptor exceeds admitted packed data");
    }
    let mut value = 0u32;
    for position in start..start + width {
        value = (value << 1) | ((bytes[position / 8] >> (7 - position % 8)) & 1) as u32;
    }
    Ok(value)
}
#[cfg(feature = "icechunk")]
fn descriptor_span(groups: usize, width: usize) -> Result<usize> {
    let bits = groups
        .checked_mul(width)
        .ok_or("GRIB descriptor size overflow")?;
    // The maintained complex decoder aligns descriptors through f32 arithmetic.
    // Bound the products to exactly represented integers before checking offsets.
    if bits > 1 << 24 {
        return fail("GRIB descriptor exceeds exactly represented decoder offsets");
    }
    bits.div_ceil(8)
        .checked_mul(8)
        .ok_or_else(|| "GRIB descriptor alignment overflow".into())
}
#[cfg(feature = "icechunk")]
fn admit_complex_packing(section: &[u8], packed: &[u8], count: usize, spatial: bool) -> Result<()> {
    let groups = u32::from_be_bytes(section[31..35].try_into().unwrap()) as usize;
    if groups == 0 {
        return Ok(());
    } // maintained decoder allocates exactly section-5 count
    if groups > count {
        return fail("GRIB group count exceeds admitted points");
    }
    let reference_bits = section[19] as usize;
    let width_bits = section[36] as usize;
    let length_bits = section[46] as usize;
    if reference_bits > 31
        || width_bits > 32
        || length_bits > 32
        || section[21] != 1
        || section[22] > 2
    {
        return fail("unsupported GRIB complex descriptor width or splitting/missing mode");
    }
    let prefix = if spatial {
        if !matches!(section[47], 1 | 2) || !(1..=4).contains(&section[48]) {
            return fail("unsupported GRIB spatial differencing descriptors");
        }
        (section[47] as usize + 1) * section[48] as usize * 8
    } else {
        0
    };
    let widths_start = prefix
        .checked_add(descriptor_span(groups, reference_bits)?)
        .ok_or("GRIB descriptor overflow")?;
    let lengths_start = widths_start
        .checked_add(descriptor_span(groups, width_bits)?)
        .ok_or("GRIB descriptor overflow")?;
    let mut values_start = lengths_start
        .checked_add(descriptor_span(groups, length_bits)?)
        .ok_or("GRIB descriptor overflow")?;
    if values_start > packed.len().saturating_mul(8) {
        return fail("truncated GRIB complex descriptors");
    }
    let width_reference = section[35] as u32;
    let length_reference = u32::from_be_bytes(section[37..41].try_into().unwrap());
    let length_increment = section[41] as u32;
    let last_length = u32::from_be_bytes(section[42..46].try_into().unwrap());
    let mut decoded = 0usize;
    for group in 0..groups {
        let width = packed_u32(packed, widths_start + group * width_bits, width_bits)?
            .checked_add(width_reference)
            .ok_or("GRIB group width overflow")?;
        if width > 31 {
            return fail("GRIB group width exceeds the admitted integer decoder");
        }
        let length = if group == groups - 1 {
            last_length
        } else if length_bits == 0 {
            length_reference
        } else {
            packed_u32(packed, lengths_start + group * length_bits, length_bits)?
                .checked_mul(length_increment)
                .and_then(|n| n.checked_add(length_reference))
                .ok_or("GRIB group length overflow")?
        };
        decoded = decoded
            .checked_add(length as usize)
            .ok_or("GRIB decoded group count overflow")?;
        if decoded > count {
            return fail("GRIB group lengths exceed admitted points");
        }
        // The upstream decoder multiplies these u32 fields before indexing.
        let bits = width
            .checked_mul(length)
            .ok_or("GRIB group bit span overflow")? as usize;
        values_start = values_start
            .checked_add(bits)
            .ok_or("GRIB packed value offset overflow")?;
        if values_start > packed.len().saturating_mul(8) {
            return fail("truncated GRIB packed group values");
        }
    }
    if decoded != count {
        return fail("GRIB group lengths do not match declared points");
    }
    Ok(())
}
#[cfg(feature = "icechunk")]
fn admit_png_packing(packed: &[u8], count: usize, bits: usize) -> Result<()> {
    if packed.len() < 45
        || &packed[..8] != b"\x89PNG\r\n\x1a\n"
        || &packed[12..16] != b"IHDR"
        || u32::from_be_bytes(packed[8..12].try_into().unwrap()) != 13
    {
        return fail("GRIB PNG requires a complete leading IHDR");
    }
    let width = u32::from_be_bytes(packed[16..20].try_into().unwrap()) as usize;
    let height = u32::from_be_bytes(packed[20..24].try_into().unwrap()) as usize;
    if width == 0 || height == 0 || width.checked_mul(height) != Some(count) {
        return fail("GRIB PNG dimensions exceed admitted points");
    }
    let depth = packed[24] as usize;
    let sample_bytes = match (packed[25], depth) {
        (0, 1 | 2 | 4 | 8) => 1,
        (0, 16) => 2,
        (2, 8) => 3,
        (6, 8) => 4,
        _ => return fail("unsupported GRIB PNG sample layout"),
    };
    if bits == 0 || bits > sample_bytes * 8 || packed[26] != 0 || packed[27] != 0 || packed[28] != 0
    {
        return fail("unsupported GRIB PNG packing, compression or interlace");
    }
    let mut position = 8usize;
    let mut idat = false;
    let mut ended = false;
    while position < packed.len() {
        if position
            .checked_add(12)
            .is_none_or(|end| end > packed.len())
        {
            return fail("truncated GRIB PNG chunk");
        }
        let length =
            u32::from_be_bytes(packed[position..position + 4].try_into().unwrap()) as usize;
        let end = position
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .filter(|end| *end <= packed.len())
            .ok_or("GRIB PNG chunk exceeds packed payload")?;
        match &packed[position + 4..position + 8] {
            b"IHDR" if position == 8 && length == 13 => {}
            b"IDAT" if !ended => idat = true,
            b"IEND" if idat && length == 0 && end == packed.len() => ended = true,
            _ => {
                return fail(
                    "GRIB PNG admits IHDR/IDAT/IEND only; ancillary/APNG chunks are unsupported",
                )
            }
        }
        position = end;
    }
    if !ended {
        return fail("GRIB PNG requires a final IEND");
    }
    Ok(())
}
#[cfg(feature = "icechunk")]
fn admit_grib_packing(section: &[u8], bitmap: &[u8], data: &[u8], grid_count: usize) -> Result<()> {
    if section.len() < 11 || bitmap.len() < 6 || data.len() < 5 {
        return fail("truncated GRIB value sections");
    }
    let count = u32::from_be_bytes(section[5..9].try_into().unwrap()) as usize;
    if count > grid_count {
        return fail("GRIB packed points exceed admitted grid");
    }
    match bitmap[5] {
        0 => {
            if bitmap.len() != 6 + grid_count.div_ceil(8) {
                return fail("GRIB bitmap length differs from admitted grid");
            }
            if !grid_count.is_multiple_of(8)
                && bitmap.last().unwrap() & ((1u8 << (8 - grid_count % 8)) - 1) != 0
            {
                return fail("nonzero GRIB bitmap padding");
            }
            if bitmap[6..]
                .iter()
                .map(|byte| byte.count_ones() as usize)
                .sum::<usize>()
                != count
            {
                return fail("GRIB bitmap defined points disagree with section5");
            }
        }
        255 if bitmap.len() == 6 && count == grid_count => {}
        _ => return fail("unsupported GRIB bitmap indicator or point count"),
    }
    let template = u16::from_be_bytes(section[9..11].try_into().unwrap());
    let expected = match template {
        0 | 41 => 21,
        2 => 47,
        3 => 49,
        42 => 25,
        _ => return fail("unsupported GRIB packing template"),
    };
    if section.len() != expected {
        return fail("GRIB packing descriptor length differs from admitted template");
    }
    let bits = section[19] as usize;
    if bits > 32 {
        return fail("GRIB bit depth exceeds admitted decoder");
    }
    let packed = &data[5..];
    match template {
        0 => {
            if bits != 0
                && packed.len()
                    != count
                        .checked_mul(bits)
                        .ok_or("GRIB simple packing size overflow")?
                        .div_ceil(8)
            {
                return fail("GRIB simple packed bytes disagree with admitted points");
            }
        }
        2 | 3 => admit_complex_packing(section, packed, count, template == 3)?,
        41 => admit_png_packing(packed, count, bits)?,
        42 => {
            let block = section[22] as usize;
            let interval = u16::from_be_bytes(section[23..25].try_into().unwrap()) as usize;
            if bits != 0
                && (!(1..=32).contains(&bits)
                    || !matches!(block, 8 | 16 | 32 | 64)
                    || interval == 0
                    || block
                        .checked_mul(interval)
                        .is_none_or(|scratch| scratch > grid_count))
            {
                return fail("GRIB CCSDS scratch or block layout exceeds admitted grid");
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
#[cfg(feature = "icechunk")]
fn decode_grib(bytes: &[u8], config: &GribConfig) -> Result<Vec<u8>> {
    std::panic::catch_unwind(|| decode_grib_checked(bytes, config))
        .map_err(|_| "GRIB decoder rejected malformed source bytes")?
}
#[cfg(feature = "icechunk")]
fn decode_grib_checked(bytes: &[u8], config: &GribConfig) -> Result<Vec<u8>> {
    if bytes.len() < 20
        || &bytes[..4] != b"GRIB"
        || bytes[7] != 2
        || u64::from_be_bytes(bytes[8..16].try_into().unwrap()) != bytes.len() as u64
        || &bytes[bytes.len() - 4..] != b"7777"
    {
        return fail("GRIB payload must contain exactly one GRIB2 message");
    }
    // Validate section framing before entering the maintained parser, whose
    // section accessors assume complete headers. This is admission, not decoding.
    let mut position = 16;
    let mut sections: [Option<&[u8]>; 8] = [None; 8];
    while position < bytes.len() - 4 {
        if position + 5 > bytes.len() - 4 {
            return fail("truncated GRIB section header");
        }
        let length = u32::from_be_bytes(bytes[position..position + 4].try_into().unwrap()) as usize;
        let section = bytes[position + 4];
        let minimum = match section {
            1 => 21,
            2 => 5,
            3 => 14,
            4 => 9,
            5 => 11,
            6 => 6,
            7 => 5,
            _ => return fail("unsupported GRIB section"),
        };
        if length < minimum
            || position
                .checked_add(length)
                .is_none_or(|end| end > bytes.len() - 4)
        {
            return fail("invalid GRIB section length");
        }
        if sections[section as usize]
            .replace(&bytes[position..position + length])
            .is_some()
        {
            return fail("repeated GRIB sections are outside the single-field profile");
        }
        if section == 3 {
            let count =
                u32::from_be_bytes(bytes[position + 6..position + 10].try_into().unwrap()) as usize;
            if count
                != config.shape[0]
                    .checked_mul(config.shape[1])
                    .ok_or("GRIB grid overflow")?
            {
                return fail("GRIB declared grid count exceeds admitted shape");
            }
        }
        if section == 5 {
            let count =
                u32::from_be_bytes(bytes[position + 5..position + 9].try_into().unwrap()) as usize;
            if count
                > config.shape[0]
                    .checked_mul(config.shape[1])
                    .ok_or("GRIB grid overflow")?
            {
                return fail("GRIB encoded point count exceeds admitted shape");
            }
        }
        position += length;
    }
    if position != bytes.len() - 4 {
        return fail("GRIB sections do not end at the message trailer");
    }
    admit_grib_packing(
        sections[5].ok_or("missing GRIB representation section")?,
        sections[6].ok_or("missing GRIB bitmap section")?,
        sections[7].ok_or("missing GRIB data section")?,
        config.shape[0]
            .checked_mul(config.shape[1])
            .ok_or("GRIB grid overflow")?,
    )?;
    let message =
        gribberish::message::Message::from_data(bytes, 0).ok_or("invalid GRIB2 message")?;
    if message.grid_dimensions()? != (config.shape[0], config.shape[1])
        || !message.is_regular_grid()?
        || message.data_point_count()?
            > config.shape[0]
                .checked_mul(config.shape[1])
                .ok_or("GRIB grid overflow")?
    {
        return fail("GRIB grid does not match admitted Zarr chunk shape");
    }
    if !matches!(message.data_template_number()?, 0 | 2 | 3 | 41 | 42) {
        return fail("unsupported GRIB2 packing template (admitted:0,2,3,41,42)");
    }
    let mut values = message.data()?;
    if values.len()
        != config.shape[0]
            .checked_mul(config.shape[1])
            .ok_or("GRIB grid overflow")?
    {
        return fail("GRIB decoded value count differs from admitted grid");
    }
    if config.adjust_longitude_range || config.north_up {
        values = message.latlng_projector()?.adjust_data(
            values,
            config.adjust_longitude_range,
            config.north_up,
        );
    }
    Ok(values
        .into_iter()
        .flat_map(|value| (value / config.scale + config.offset).to_le_bytes())
        .collect())
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
    let grib = grib_config(&metadata)?;
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
    if grib.is_some() {
        metadata["codecs"] =
            serde_json::json!([{"name":"bytes","configuration":{"endian":"little"}}]);
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
        grib,
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
        grib: None,
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
            #[cfg(feature = "icechunk")]
            if let Some(config) = &self.grib {
                return Ok(Some(decode_grib(&bytes, config)?.into()));
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
    #[cfg(feature = "icechunk")]
    #[test]
    fn grib_rejects_declared_allocation_bombs_and_bad_framing_before_decode() {
        let config = GribConfig {
            shape: [2, 2],
            north_up: false,
            adjust_longitude_range: false,
            scale: 1.,
            offset: 0.,
        };
        for section in [3u8, 5] {
            let section_size = if section == 3 { 14usize } else { 11usize };
            let mut bytes = vec![0u8; 16 + section_size + 4];
            bytes[..4].copy_from_slice(b"GRIB");
            bytes[7] = 2;
            let length = bytes.len() as u64;
            bytes[8..16].copy_from_slice(&length.to_be_bytes());
            bytes[16..20].copy_from_slice(&(section_size as u32).to_be_bytes());
            bytes[20] = section;
            let offset = if section == 3 { 22 } else { 21 };
            bytes[offset..offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
            let end = bytes.len();
            bytes[end - 4..].copy_from_slice(b"7777");
            assert!(decode_grib(&bytes, &config)
                .unwrap_err()
                .to_string()
                .contains("count exceeds admitted shape"));
            bytes[16..20].copy_from_slice(&1u32.to_be_bytes());
            assert!(decode_grib(&bytes, &config).is_err());
        }
        assert!(decode_grib(b"GRIB", &config).is_err());
    }
    #[cfg(feature = "icechunk")]
    #[test]
    fn grib_complex_lengths_and_descriptors_are_bounded_before_library_decode() {
        for template in [2u16, 3] {
            let mut section = vec![0u8; if template == 2 { 47 } else { 49 }];
            section[5..9].copy_from_slice(&4u32.to_be_bytes());
            section[9..11].copy_from_slice(&template.to_be_bytes());
            section[21] = 1;
            section[31..35].copy_from_slice(&1u32.to_be_bytes());
            section[42..46].copy_from_slice(&4u32.to_be_bytes());
            if template == 3 {
                section[47] = 1;
                section[48] = 1;
            }
            let data = vec![0u8; if template == 2 { 5 } else { 7 }];
            let bitmap = [0, 0, 0, 6, 6, 255];
            admit_grib_packing(&section, &bitmap, &data, 4).unwrap();
            section[42..46].copy_from_slice(&u32::MAX.to_be_bytes());
            assert!(admit_grib_packing(&section, &bitmap, &data, 4)
                .unwrap_err()
                .to_string()
                .contains("group lengths exceed"));
            section[42..46].copy_from_slice(&4u32.to_be_bytes());
            section[31..35].copy_from_slice(&u32::MAX.to_be_bytes());
            assert!(admit_grib_packing(&section, &bitmap, &data, 4).is_err());
            section[31..35].copy_from_slice(&1u32.to_be_bytes());
            section[35] = 32;
            assert!(admit_grib_packing(&section, &bitmap, &data, 4).is_err());
            section[35] = 1;
            assert!(admit_grib_packing(&section, &bitmap, &data, 4).is_err()); // no packed group bits
        }
    }
    #[cfg(feature = "icechunk")]
    #[test]
    fn grib_bitmap_simple_and_ccsds_amplification_is_rejected_before_decode() {
        let mut section = vec![0u8; 21];
        section[5..9].copy_from_slice(&4u32.to_be_bytes());
        section[19] = 1;
        let bitmap = [0, 0, 0, 6, 6, 255];
        admit_grib_packing(&section, &bitmap, &[0; 6], 4).unwrap();
        assert!(admit_grib_packing(&section, &bitmap, &[0; 7], 4).is_err());
        assert!(admit_grib_packing(&section, &[0, 0, 0, 8, 6, 0, 0xf0, 0], &[0; 6], 4).is_err());
        assert!(admit_grib_packing(&section, &[0, 0, 0, 7, 6, 0, 0xff], &[0; 6], 4).is_err());
        assert!(admit_grib_packing(&section, &[0, 0, 0, 6, 6, 254], &[0; 6], 4).is_err());
        let mut section = vec![0u8; 25];
        section[5..9].copy_from_slice(&4u32.to_be_bytes());
        section[9..11].copy_from_slice(&42u16.to_be_bytes());
        section[19] = 8;
        section[22] = 64;
        section[23..25].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(admit_grib_packing(&section, &bitmap, &[0; 5], 4)
            .unwrap_err()
            .to_string()
            .contains("scratch"));
    }
    #[cfg(feature = "icechunk")]
    #[test]
    fn grib_png_dimensions_and_ancillary_chunks_cannot_expand_allocation() {
        fn chunk(bytes: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend_from_slice(name);
            bytes.extend_from_slice(data);
            bytes.extend_from_slice(&[0; 4]);
        }
        let mut header = vec![0u8; 13];
        header[..4].copy_from_slice(&2u32.to_be_bytes());
        header[4..8].copy_from_slice(&2u32.to_be_bytes());
        header[8] = 8;
        let png = |extra: bool| {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            chunk(&mut bytes, b"IHDR", &header);
            if extra {
                chunk(&mut bytes, b"zTXt", &[0; 4]);
            }
            chunk(&mut bytes, b"IDAT", &[0]);
            chunk(&mut bytes, b"IEND", &[]);
            bytes
        };
        let mut valid = png(false);
        admit_png_packing(&valid, 4, 8).unwrap();
        valid[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(admit_png_packing(&valid, 4, 8)
            .unwrap_err()
            .to_string()
            .contains("dimensions"));
        assert!(admit_png_packing(&png(true), 4, 8).is_err());
        let mut invalid = png(false);
        invalid[25] = 3;
        assert!(admit_png_packing(&invalid, 4, 8).is_err());
        let mut invalid = png(false);
        invalid[28] = 1;
        assert!(admit_png_packing(&invalid, 4, 8).is_err());
    }
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
