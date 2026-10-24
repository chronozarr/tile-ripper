//! Narrow storage-domain COG adapter. TIFF parsing/codecs are upstream.
use crate::{
    fail, Asset, Chunk, ExecutionReport, Layout, Node, Operation, Read, Result, Selected, Tile,
    Variable,
};
use geotiff_core::{CrsInfo, GeoKeyDirectory, GeoTransform, RasterType};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use tiff_reader::{Ifd, OpenOptions, ParseBudgets, TagValue, TiffFile};
#[path = "cog_range.rs"]
mod range;
use range::RangeSource;
const MAX_CACHE: usize = 8 * 1024 * 1024;
const MAX_TILE: usize = 2 * 1024 * 1024;
const CODEC_WORKING: usize = 128 * 1024;
const MAX_RETAINED_PHYSICAL_METADATA: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleType {
    UInt8,
    UInt16,
    Int16,
    Float32,
}
impl SampleType {
    fn from_ifd(i: &Ifd) -> Result<Self> {
        match (
            i.bits_per_sample()?.as_slice(),
            i.sample_format()?.as_slice(),
        ) {
            ([8], [1]) => Ok(Self::UInt8),
            ([16], [1]) => Ok(Self::UInt16),
            ([16], [2]) => Ok(Self::Int16),
            ([32], [3]) => Ok(Self::Float32),
            _ => fail("native COG supports Byte, UInt16, Int16, and Float32 storage samples only"),
        }
    }
    fn bytes(self) -> usize {
        match self {
            Self::UInt8 => 1,
            Self::UInt16 | Self::Int16 => 2,
            Self::Float32 => 4,
        }
    }
    fn dtype(self) -> &'static str {
        match self {
            Self::UInt8 => "Byte",
            Self::UInt16 => "UInt16",
            Self::Int16 => "Int16",
            Self::Float32 => "Float32",
        }
    }
    fn predictor(self, value: u16) -> bool {
        match self {
            Self::Float32 => matches!(value, 1 | 3),
            _ => matches!(value, 1 | 2),
        }
    }
    fn nodata(self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        match self {
            Self::UInt8 if value.fract() == 0. && (0. ..=u8::MAX as f64).contains(&value) => {
                Some(value)
            }
            Self::UInt16 if value.fract() == 0. && (0. ..=u16::MAX as f64).contains(&value) => {
                Some(value)
            }
            Self::Int16
                if value.fract() == 0. && (i16::MIN as f64..=i16::MAX as f64).contains(&value) =>
            {
                Some(value)
            }
            // GDAL_NODATA is decimal text. Normalize it to the exact stored
            // Float32 value used by masking and validity comparisons.
            Self::Float32 if (value as f32).is_finite() => Some((value as f32) as f64),
            _ => None,
        }
    }
}

fn sample_type(l: &Layout) -> Result<SampleType> {
    match l.dtype.as_str() {
        "Byte" => Ok(SampleType::UInt8),
        "UInt16" => Ok(SampleType::UInt16),
        "Int16" => Ok(SampleType::Int16),
        "Float32" => Ok(SampleType::Float32),
        _ => fail("unsupported native COG sample type"),
    }
}

struct PhysicalMetadata {
    geokey_directory: Vec<u16>,
    geo_double_params: Vec<f64>,
    geo_ascii_params: String,
    model_pixel_scale: Vec<f64>,
    model_tiepoint: Vec<f64>,
    raster_type: &'static str,
    gdal_metadata_xml: Option<String>,
    gdal_nodata_text: Option<String>,
    normalized_crs: String,
    normalization: Option<&'static str>,
}
impl PhysicalMetadata {
    fn bytes(&self) -> usize {
        self.geokey_directory.capacity() * 2
            + self.geo_double_params.capacity() * 8
            + self.geo_ascii_params.capacity()
            + self.model_pixel_scale.capacity() * 8
            + self.model_tiepoint.capacity() * 8
            + self.gdal_metadata_xml.as_ref().map_or(0, String::capacity)
            + self.gdal_nodata_text.as_ref().map_or(0, String::capacity)
            + self.normalized_crs.capacity()
    }
    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "geokey_directory":self.geokey_directory,
            "geo_double_params":self.geo_double_params,
            "geo_ascii_params":self.geo_ascii_params,
            "model_pixel_scale":self.model_pixel_scale,
            "model_tiepoint":self.model_tiepoint,
            "raster_type":self.raster_type,
            "gdal_metadata_xml":self.gdal_metadata_xml,
            "gdal_nodata_text":self.gdal_nodata_text,
            "normalized_crs":self.normalized_crs,
            "normalization":self.normalization,
        })
    }
}

#[derive(Default)]
pub(crate) struct Snapshots {
    cap: usize,
    entries: Mutex<BTreeMap<String, Arc<range::Snapshot>>>,
    physical: Mutex<BTreeMap<String, PhysicalMetadata>>,
    #[cfg(feature = "authenticated-http")]
    sessions: BTreeMap<String, Arc<crate::authenticated_http::AuthenticatedHttp>>,
    #[cfg(feature = "authenticated-http")]
    authenticated_identities: Mutex<BTreeMap<String, (String, u64)>>,
    skipped: AtomicUsize,
    // Shared by this preparation's readers, including the no-page-cache path.
    failed: Arc<AtomicBool>,
}
impl Snapshots {
    pub(crate) fn new(buffer_bytes: usize) -> Self {
        Self {
            cap: (buffer_bytes / 2).min(8 * 1024 * 1024),
            ..Self::default()
        }
    }
    #[cfg(feature = "authenticated-http")]
    pub(crate) fn with_sessions(
        buffer_bytes: usize,
        sessions: BTreeMap<String, Arc<crate::authenticated_http::AuthenticatedHttp>>,
    ) -> Result<Self> {
        if sessions.keys().any(|id| id.is_empty()) {
            return fail("authenticated native COG session requires a nonempty asset id");
        }
        Ok(Self {
            cap: (buffer_bytes / 2).min(8 * 1024 * 1024),
            sessions,
            ..Self::default()
        })
    }
    #[cfg(feature = "authenticated-http")]
    fn session(&self, a: &Asset) -> Result<Option<range::Session>> {
        let Some(session) = self.sessions.get(&a.id) else {
            return Ok(None);
        };
        if a.format != crate::Format::Cog || !session.accepts(&a.locator) {
            return fail("authenticated native COG session does not match its exact asset object");
        }
        Ok(Some(session.clone()))
    }
    #[cfg(not(feature = "authenticated-http"))]
    fn session(&self, _: &Asset) -> Result<Option<range::Session>> {
        Ok(None)
    }
    #[cfg(feature = "authenticated-http")]
    pub(crate) fn resolve_authenticated(
        &self,
        assets: &mut [Asset],
    ) -> Result<BTreeMap<String, u64>> {
        if assets.len() != self.sessions.len() {
            return fail("authenticated native COG sessions and assets must match exactly");
        }
        let mut identities = BTreeMap::new();
        let mut sizes = BTreeMap::new();
        for asset in assets {
            let session = self
                .sessions
                .get(&asset.id)
                .ok_or("authenticated native COG asset is missing its exact session")?;
            if asset.format != crate::Format::Cog || !session.accepts(&asset.locator) {
                return fail(
                    "authenticated native COG session does not match its exact asset object",
                );
            }
            let (etag, size) = session.head_identity(None)?;
            if asset
                .etag
                .as_ref()
                .is_some_and(|expected| expected != &etag)
            {
                return fail("authenticated native COG resolved ETag conflicts with request pin");
            }
            asset.etag = Some(etag.clone());
            if identities.insert(asset.id.clone(), (etag, size)).is_some() {
                return fail("duplicate authenticated native COG asset id");
            }
            sizes.insert(asset.id.clone(), size);
        }
        *self.authenticated_identities.lock().unwrap() = identities;
        Ok(sizes)
    }
    #[cfg(feature = "authenticated-http")]
    pub(crate) fn validate_authenticated(&self) -> Result<()> {
        let identities = self.authenticated_identities.lock().unwrap().clone();
        let result = (|| {
            if identities.len() != self.sessions.len() {
                return fail("authenticated native COG identities were not resolved");
            }
            for (id, session) in &self.sessions {
                let (etag, size) = identities
                    .get(id)
                    .ok_or("authenticated native COG identity is missing")?;
                session.validate_identity(etag, *size, None)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.invalidate();
        }
        result
    }
    #[cfg(feature = "authenticated-http")]
    pub(crate) fn transport_reservation(&self) -> Result<usize> {
        self.sessions
            .len()
            .checked_mul(crate::authenticated_http::WORKING_RESERVATION)
            .ok_or_else(|| "authenticated native COG transport reservation overflow".into())
    }
    #[cfg(feature = "authenticated-http")]
    pub(crate) fn transport_stats(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.sessions
                .iter()
                .map(|(id, session)| (id.clone(), session.statistics()))
                .collect(),
        )
    }
    pub(crate) fn bytes(&self) -> usize {
        let snapshots = self
            .entries
            .lock()
            .unwrap()
            .values()
            .map(|s| s.bytes())
            .sum::<usize>();
        snapshots
            + self
                .physical
                .lock()
                .unwrap()
                .values()
                .map(PhysicalMetadata::bytes)
                .sum::<usize>()
    }
    pub(crate) fn trim(&self, cap: usize) {
        let mut entries = self.entries.lock().unwrap();
        let mut retained = entries.values().map(|s| s.bytes()).sum::<usize>()
            + self
                .physical
                .lock()
                .unwrap()
                .values()
                .map(PhysicalMetadata::bytes)
                .sum::<usize>();
        while retained > cap {
            let Some((_, snapshot)) = entries.pop_last() else {
                break;
            };
            retained -= snapshot.bytes();
            self.skipped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(crate) fn inspect(&self, a: &Asset, band: usize) -> Result<Layout> {
        let (tiff, source) = open(a, 0, None, self.failed.clone(), self.session(a)?)?;
        let layout = layout(&tiff, band)?;
        let physical = physical_metadata(&tiff, &layout)?;
        if physical.bytes() > MAX_RETAINED_PHYSICAL_METADATA {
            return fail("native COG physical metadata exceeds 64 KiB per-source bound");
        }
        {
            let snapshots = self.entries.lock().unwrap();
            let mut entries = self.physical.lock().unwrap();
            let retained = entries
                .iter()
                .filter(|(id, _)| **id != a.id)
                .map(|(_, metadata)| metadata.bytes())
                .sum::<usize>()
                + snapshots
                    .values()
                    .map(|snapshot| snapshot.bytes())
                    .sum::<usize>();
            if retained
                .checked_add(physical.bytes())
                .is_none_or(|bytes| bytes > self.cap)
            {
                return fail("native COG physical metadata exceeds prepared buffer reservation");
            }
            entries.insert(a.id.clone(), physical);
        }
        if self.cap > 0 && crate::cog::remote(&a.locator) {
            let snapshot = Arc::new(source.snapshot(a));
            let mut entries = self.entries.lock().unwrap();
            let retained = entries
                .iter()
                .filter(|(id, _)| **id != a.id)
                .map(|(_, s)| s.bytes())
                .sum::<usize>()
                + self
                    .physical
                    .lock()
                    .unwrap()
                    .values()
                    .map(PhysicalMetadata::bytes)
                    .sum::<usize>();
            // If the bounded optimization does not fit, retain baseline reads.
            if retained
                .checked_add(snapshot.bytes())
                .is_some_and(|n| n <= self.cap)
            {
                entries.insert(a.id.clone(), snapshot);
            } else {
                self.skipped.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(layout)
    }
    fn get(&self, a: &Asset) -> Option<Arc<range::Snapshot>> {
        self.entries.lock().unwrap().get(&a.id).cloned()
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.failed.load(Ordering::Acquire) {
            return fail(
                "native COG preparation invalidated by an earlier source failure; prepare again",
            );
        }
        #[cfg(feature = "authenticated-http")]
        if !self.sessions.is_empty() {
            self.validate_authenticated()?;
        }
        Ok(())
    }
    pub(crate) fn invalidate(&self) {
        self.failed.store(true, Ordering::Release);
    }
    pub(crate) fn summary(&self) -> serde_json::Value {
        let entries = self.entries.lock().unwrap();
        let physical = self.physical.lock().unwrap();
        let physical_bytes = physical
            .values()
            .map(PhysicalMetadata::bytes)
            .sum::<usize>();
        let summary = serde_json::json!({"retained_sources":entries.len(),"retained_bytes":entries.values().map(|s|s.bytes()).sum::<usize>() + physical_bytes,
            "physical_metadata_sources":physical.len(),"physical_metadata_bytes":physical_bytes,
            "cap_bytes":self.cap,"skipped_sources":self.skipped.load(Ordering::Relaxed),
            "scope":"this Prepared only; immutable version-bound parser pages; never a data cache"});
        #[cfg(feature = "authenticated-http")]
        let summary = {
            let mut summary = summary;
            summary["authenticated_sources"] = serde_json::json!(self.sessions.len());
            summary["authenticated_transport"] = self.transport_stats();
            summary
        };
        summary
    }
    pub(crate) fn describe(&self, id: &str) -> Option<serde_json::Value> {
        self.physical
            .lock()
            .unwrap()
            .get(id)
            .map(PhysicalMetadata::describe)
    }
}

#[derive(Clone, Default)]
pub(crate) struct Strategy {
    cache_bytes: usize,
    pub(crate) groups: Vec<range::Group>,
    delivery: Delivery,
}
#[derive(Clone, Default)]
enum Delivery {
    #[default]
    Windows,
    Selection(crate::Window),
}
impl Strategy {
    pub(crate) fn summary(&self) -> serde_json::Value {
        let (scope, selection_bytes) = match &self.delivery {
            Delivery::Windows => ("planned_windows", 0),
            Delivery::Selection(w) => ("bounded_selection", w.width * w.height * 2),
        };
        serde_json::json!({"read_scope":scope,"retained_selection_bytes":selection_bytes,
            "decoded_cache_limit_bytes":self.cache_bytes,"coalesced_groups":self.groups.len(),
            "max_encoded_group_reservation_bytes":self.groups.iter().map(|g|g.len + 8).max().unwrap_or(0),
            "codec_working_bytes":CODEC_WORKING})
    }
}
// Match the existing cache's physical reads with the smallest LRU. This uses
// actual planned windows (including skipped masked windows), not AOI area.
fn cache_slots(sequence: &[(usize, usize)], maximum: usize) -> usize {
    fn misses(sequence: &[(usize, usize)], slots: usize) -> usize {
        let mut cache = VecDeque::new();
        let mut misses = 0;
        for tile in sequence {
            if let Some(i) = cache.iter().position(|t| t == tile) {
                cache.remove(i);
            } else {
                misses += 1;
            }
            if slots > 0 {
                cache.push_back(*tile);
                if cache.len() > slots {
                    cache.pop_front();
                }
            }
        }
        misses
    }
    let expected = misses(sequence, maximum);
    // LRU has the stack property: increasing capacity never increases misses.
    let (mut low, mut high) = (0, maximum);
    while low < high {
        let middle = (low + high) / 2;
        if misses(sequence, middle) == expected {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}
fn block_working(tile_capacity: usize, encoded: usize) -> Result<usize> {
    let decode = tile_capacity
        .checked_mul(2)
        .and_then(|n| n.checked_add(encoded))
        .ok_or("native COG decode working set overflow")?;
    // Upstream can retain a readable but mismatched wrapped payload while the
    // direct fallback holds both its raw range and its unwrapped payload copy.
    let fallback = encoded
        .checked_mul(3)
        .ok_or("native COG encoded working set overflow")?;
    decode
        .max(fallback)
        .checked_add(CODEC_WORKING)
        .ok_or_else(|| "native COG codec working set overflow".into())
}
pub(crate) fn reservation(
    l: &Layout,
    selection: &crate::Window,
    nodes: &[&Node],
    budget: usize,
    workers: usize,
) -> Result<(usize, usize, Strategy)> {
    let sample_bytes = sample_type(l)?.bytes();
    let tile = l.block[0]
        .checked_mul(l.block[1])
        .and_then(|n| n.checked_mul(sample_bytes))
        .filter(|n| *n <= MAX_TILE)
        .ok_or("native COG tile exceeds 2 MiB decode limit")?;
    let tiles: Vec<_> = nodes
        .iter()
        .flat_map(|n| match &n.operation {
            Operation::Read {
                read: Read::LibraryWindow { tiles, .. },
            } => tiles.as_slice(),
            _ => &[],
        })
        .collect();
    let encoded = tiles
        .iter()
        .map(|t| t.bytes.unwrap_or(0))
        .max()
        .unwrap_or(0) as usize
        + 8;
    let sequence: Vec<_> = tiles.iter().map(|t| (t.x, t.y)).collect();
    let slots = cache_slots(&sequence, (MAX_CACHE / tile).min(64));
    // Upstream Vec growth can retain capacity above decoded length for a
    // non-power-of-two tile. Charge capacity, including the codec's probe.
    let tile_capacity = tile
        .checked_next_power_of_two()
        .ok_or("tile capacity overflow")?;
    let retained = tile_capacity
        .checked_mul(slots)
        .ok_or("cache size overflow")?;
    let block_peak = block_working(tile_capacity, encoded)?;
    let mut working = block_peak;
    let windows = nodes
        .iter()
        .filter_map(|n| match &n.operation {
            Operation::Read {
                read: Read::LibraryWindow { window, .. },
            } => Some(window),
            _ => None,
        })
        .map(|w| {
            w.width
                .checked_mul(w.height)
                .and_then(|n| n.checked_mul(sample_bytes))
                .and_then(|n| n.checked_mul(2))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("window size overflow")?
        .into_iter()
        .max()
        .unwrap_or(0);
    let selection_bytes = selection
        .width
        .checked_mul(selection.height)
        .and_then(|n| n.checked_mul(sample_bytes))
        .ok_or("selection size overflow")?;
    let selection_tiles = crate::cog::tiles(l, selection);
    let key_set = |tiles: &[&Tile]| tiles.iter().map(|t| (t.x, t.y)).collect::<BTreeSet<_>>();
    let expected = selection_tiles
        .iter()
        .map(|t| (t.x, t.y))
        .collect::<BTreeSet<_>>();
    let first = nodes.first().and_then(|n| match &n.operation {
        Operation::Read {
            read: Read::LibraryWindow { tiles, .. },
        } => Some(tiles),
        _ => None,
    });
    if nodes.len() > 1
        && selection_bytes <= MAX_TILE
        && key_set(&tiles) == expected
        && first.is_some_and(|tiles| {
            tiles.iter().map(|t| (t.x, t.y)).collect::<BTreeSet<_>>() == expected
        })
    {
        // These phases do not overlap: upstream first produces raw selection
        // bytes while decoding blocks, then converts raw bytes to typed values.
        // Only afterward do we retain the selection and allocate/mask one chunk.
        let decode = selection_bytes
            .checked_add(block_peak)
            .ok_or("selection working set overflow")?;
        let convert = selection_bytes
            .checked_mul(2)
            .ok_or("selection working set overflow")?;
        let deliver = selection_bytes
            .checked_add(windows)
            .ok_or("selection working set overflow")?;
        let base = decode.max(convert).max(deliver);
        // Preserve the largest payload concurrency the individual-tile strategy
        // can admit. Coalescing may use spare capacity, never a reader's slot.
        let concurrency = (budget / base).max(1).min(workers.max(1));
        let available = (budget / concurrency).saturating_sub(base);
        let groups = range::groups(selection_tiles, available);
        let merged = groups.iter().map(|g| g.len + 8).max().unwrap_or(0);
        let peak = decode
            .checked_add(merged)
            .ok_or("selection encoded working set overflow")?
            .max(convert)
            .max(deliver);
        return Ok((
            0,
            peak.saturating_sub(windows),
            Strategy {
                cache_bytes: 0,
                groups,
                delivery: Delivery::Selection(selection.clone()),
            },
        ));
    }
    let available = budget.saturating_sub(retained.saturating_add(working).saturating_add(windows));
    // Only group the actual cache misses, and only if every group member is
    // consumed consecutively. This prevents refetching a half-consumed group
    // when a TIFF's physical order differs from its tile index order.
    let mut lru = VecDeque::new();
    let mut misses = Vec::new();
    for tile in &tiles {
        let key = (tile.x, tile.y);
        if let Some(i) = lru.iter().position(|t| *t == key) {
            lru.remove(i);
        } else {
            misses.push((**tile).clone());
        }
        if slots > 0 {
            lru.push_back(key);
            if lru.len() > slots {
                lru.pop_front();
            }
        }
    }
    let mut first_needed = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if let Operation::Read {
            read: Read::LibraryWindow { tiles, .. },
        } = &node.operation
        {
            for tile in tiles {
                if let Some(offset) = tile.offset {
                    first_needed.entry(offset).or_insert(index);
                }
            }
        }
    }
    let mut groups = range::groups(misses, available);
    // Do not delay an early chunk to prefetch a tile only a later chunk needs.
    // The controlled replay found no six-source wall-time benefit from that
    // broader prefetch despite its lower GET count.
    groups.retain(|g| g.first_needed_together(&first_needed));
    let merged = groups.iter().map(|g| g.len + 8).max().unwrap_or(0);
    working = working
        .checked_add(merged)
        .ok_or("encoded working set overflow")?;
    Ok((
        retained,
        working,
        Strategy {
            cache_bytes: tile * slots,
            groups,
            delivery: Delivery::Windows,
        },
    ))
}
fn open(
    a: &Asset,
    cache: usize,
    snapshot: Option<&range::Snapshot>,
    failed: Arc<AtomicBool>,
    session: Option<range::Session>,
) -> Result<(TiffFile, Arc<RangeSource>)> {
    let source = RangeSource::open(a, snapshot, failed, session)?;
    let tiff = TiffFile::from_source_with_options(
        source.clone(),
        OpenOptions {
            block_cache_bytes: cache,
            block_cache_slots: 64,
            parse_budgets: ParseBudgets {
                max_ifds: 16,
                max_ifd_entries: 256,
                max_tag_value_bytes: 1024 * 1024,
                max_metadata_value_bytes: 2 * 1024 * 1024,
            },
            decode_output_bytes: MAX_TILE,
        },
    )?;
    Ok((tiff, source))
}
fn doubles(ifd: &Ifd, tag: u16) -> Result<Vec<f64>> {
    match ifd.tag(tag).map(|t| &t.value) {
        Some(TagValue::Double(v)) => Ok(v.clone()),
        _ => fail("native COG missing/invalid DOUBLE spatial tag"),
    }
}
fn gdal_metadata_xml(i: &Ifd) -> Result<Option<&str>> {
    i.tag(42112)
        .map(|tag| {
            tag.value
                .as_str()
                .ok_or_else(|| "native COG invalid GDAL metadata XML".into())
                .and_then(|xml| {
                    if xml.len() > MAX_RETAINED_PHYSICAL_METADATA {
                        return fail("native COG GDAL metadata exceeds retained profile bound");
                    }
                    Ok(xml.trim_end_matches('\0'))
                })
        })
        .transpose()
}
fn opera_dswx_hls_1(i: &Ifd) -> Result<bool> {
    let Some(xml) = gdal_metadata_xml(i)? else {
        return Ok(false);
    };
    let doc =
        roxmltree::Document::parse(xml).map_err(|_| "native COG malformed GDAL metadata XML")?;
    if doc.root_element().tag_name().name() != "GDALMetadata" {
        return Ok(false);
    }
    let item = |name: &str| {
        doc.root_element()
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "Item")
            .find(|node| node.attribute("role").is_none() && node.attribute("name") == Some(name))
            .and_then(|node| node.text())
            .map(str::trim)
    };
    Ok(item("PROJECT") == Some("OPERA")
        && item("PRODUCT_TYPE") == Some("DSWx-HLS")
        && item("PRODUCT_VERSION") == Some("1.0"))
}
fn physical_metadata(tiff: &TiffFile, layout: &Layout) -> Result<PhysicalMetadata> {
    let i = tiff.ifd(0)?;
    let geokey_directory = i
        .tag(34735)
        .and_then(|tag| tag.value.as_u16_slice())
        .ok_or("native COG missing GeoKey directory")?
        .to_vec();
    let geo_double_params = i
        .tag(34736)
        .map(|_| doubles(i, 34736))
        .transpose()?
        .unwrap_or_default();
    let geo_ascii_params = i
        .tag(34737)
        .map(|tag| -> Result<String> {
            Ok(tag.value.as_str().ok_or("invalid GeoASCII tag")?.to_owned())
        })
        .transpose()?
        .unwrap_or_default();
    let keys = GeoKeyDirectory::parse(&geokey_directory, &geo_double_params, &geo_ascii_params)
        .ok_or("native COG malformed GeoKeys")?;
    let raster_type = match keys.get_short(1025) {
        None | Some(1) => "PixelIsArea",
        Some(2) => "PixelIsPoint",
        _ => return fail("native COG unsupported GeoTIFF raster type"),
    };
    let provider_normalized = keys.get_short(3072) == Some(32767)
        && opera_dswx_hls_1(i)?
        && wgs84_user_defined_utm(&keys).is_some();
    let raw_string = |tag: u16| -> Result<Option<String>> {
        i.tag(tag)
            .map(|tag| {
                tag.value
                    .as_str()
                    .ok_or_else(|| "native COG invalid retained ASCII tag".into())
                    .map(str::to_owned)
            })
            .transpose()
    };
    Ok(PhysicalMetadata {
        geokey_directory,
        geo_double_params,
        geo_ascii_params,
        model_pixel_scale: doubles(i, 33550)?,
        model_tiepoint: doubles(i, 33922)?,
        raster_type,
        gdal_metadata_xml: raw_string(42112)?,
        gdal_nodata_text: raw_string(42113)?,
        normalized_crs: layout.crs.clone(),
        normalization: provider_normalized.then_some(
            "OPERA DSWx-HLS 1.0 user-defined UTM code with exact WGS 84 ellipsoid normalized to its equivalent EPSG UTM CRS",
        ),
    })
}
// OPERA DSWx-HLS 1.0 writes its MGRS/UTM grid as a user-defined CRS even
// though the GeoKeys carry the standard UTM projection code and exact WGS 84
// ellipsoid. Admit only that fully specified provider representation and
// normalize it to the equivalent EPSG UTM identifier used by our CRS boundary.
fn wgs84_user_defined_utm(keys: &GeoKeyDirectory) -> Option<u32> {
    let one = |id: u16| {
        keys.get_double(id)
            .and_then(|values| (values.len() == 1).then(|| values[0]))
    };
    let projection = keys.get_short(3074)?;
    let (zone, base) = match projection {
        16001..=16060 => (projection - 16000, 32600u32),
        16101..=16160 => (projection - 16100, 32700u32),
        _ => return None,
    };
    let citation = keys.get_ascii(1026)?;
    let geodetic = keys.get_ascii(2049)?;
    (keys.get_short(1024) == Some(1)
        && keys.get_short(3072) == Some(32767)
        && keys.get_short(2048) == Some(32767)
        && keys.get_short(2050) == Some(32767)
        && keys.get_short(2054) == Some(9102)
        && keys.get_short(2056) == Some(7030)
        && keys.get_short(3076) == Some(9001)
        && citation.starts_with(&format!("UTM Zone {zone},"))
        && geodetic.contains("Unknown datum based upon the WGS 84 ellipsoid")
        && one(2057) == Some(6_378_137.0)
        && one(2059).is_some_and(|value| (value - 298.257_223_563).abs() <= 1e-12)
        && one(2061) == Some(0.0))
    .then_some(base + u32::from(zone))
}
fn layout(tiff: &TiffFile, band: usize) -> Result<Layout> {
    if band != 1 {
        return fail("native COG supports one storage-domain band (index 1)");
    }
    let i = tiff.ifd(0)?;
    let sample = SampleType::from_ifd(i)?;
    let palette = i.photometric_interpretation() == Some(3);
    if !i.is_tiled()
        || i.width() == 0
        || i.height() == 0
        || i.samples_per_pixel() != 1
        || i.planar_configuration() != 1
        || !(i.photometric_interpretation() == Some(1) || (palette && sample == SampleType::UInt8))
        || !i.extra_samples()?.is_empty()
        || i.tag(274).is_some_and(|t| t.value.as_u16() != Some(1))
    {
        return fail("native COG requires tiled single-band MinIsBlack storage or Byte palette indices, top-left orientation, no alpha");
    }
    if palette && i.color_map()?.is_none_or(|map| map.len() != 256) {
        return fail("native COG Byte palette requires exactly 256 color entries");
    }
    if !palette && i.tag(320).is_some() {
        return fail("native COG ColorMap requires Byte palette photometric storage");
    }
    if !matches!(i.compression(), 1 | 5 | 8 | 32946) || !sample.predictor(i.predictor()) {
        return fail(
            "native COG supports lossless uncompressed/LZW/Deflate with integer predictor 1/2 or Float32 predictor 1/3 only",
        );
    }
    // Never silently lose internal masks or follow unbounded auxiliary trees.
    if tiff.ifds().iter().any(|d| {
        d.tag(330).is_some()
            || d.tag(254)
                .and_then(|t| t.value.as_u32())
                .is_some_and(|v| v & 4 != 0)
    }) {
        return fail("native COG explicit masks and SubIFDs are unsupported");
    }
    if i.tag(254).and_then(|t| t.value.as_u32()).unwrap_or(0) != 0 {
        return fail("native COG first IFD must be full resolution");
    }
    let (scale_value, offset_value) = calibration(i)?;
    if i.tag(34264).is_some() {
        return fail("native COG transformation matrices are outside the validated profile");
    }
    let dir = i
        .tag(34735)
        .and_then(|t| t.value.as_u16_slice())
        .ok_or("native COG missing GeoKey directory")?;
    let double = i
        .tag(34736)
        .map(|_| doubles(i, 34736))
        .transpose()?
        .unwrap_or_default();
    let ascii = i
        .tag(34737)
        .map(|t| t.value.as_str().ok_or("invalid GeoASCII tag"))
        .transpose()?
        .unwrap_or("");
    let keys = GeoKeyDirectory::parse(dir, &double, ascii).ok_or("native COG malformed GeoKeys")?;
    let crs = CrsInfo::from_geokeys(&keys);
    let provider_utm = if opera_dswx_hls_1(i)? {
        wgs84_user_defined_utm(&keys)
    } else {
        None
    };
    let epsg = crs
        .epsg()
        .filter(|n| *n != 0 && *n != 32767)
        .or(provider_utm)
        .ok_or("native COG requires an EPSG CRS")?;
    if !matches!(keys.get_short(1024), Some(1 | 2)) || keys.get_short(4096).is_some() {
        return fail("native COG unsupported compound/geocentric CRS");
    }
    // GeoTIFF tiepoints use a pixel corner for PixelIsArea and a pixel center
    // for PixelIsPoint. Normalize both to the corner-based affine convention
    // used by GDAL/Rasterio and the rest of Tile-Ripper.
    let raster_type = match keys.get_short(1025) {
        None | Some(1) => RasterType::PixelIsArea,
        Some(2) => RasterType::PixelIsPoint,
        _ => return fail("native COG unsupported GeoTIFF raster type"),
    };
    let scale = doubles(i, 33550)?;
    let tie = doubles(i, 33922)?;
    if scale.len() != 3
        || tie.len() != 6
        || !scale.iter().chain(&tie).all(|v| v.is_finite())
        || scale[0] <= 0.
        || scale[1] <= 0.
        || tie[2] != 0.
        || tie[5] != 0.
    {
        return fail("native COG requires finite north-up scale and one 2D tiepoint");
    }
    let g = GeoTransform::from_tiepoint_and_scale_with_raster_type(
        &tie.try_into().unwrap(),
        &scale.try_into().unwrap(),
        raster_type,
    );
    let transform = [
        g.origin_x,
        g.pixel_width,
        g.skew_x,
        g.origin_y,
        g.skew_y,
        g.pixel_height,
    ];
    if !transform.iter().all(|v| v.is_finite()) {
        return fail("native COG nonfinite affine");
    }
    let bx = i
        .tile_width()
        .filter(|n| *n > 0)
        .ok_or("missing tile width")? as usize;
    let by = i
        .tile_height()
        .filter(|n| *n > 0)
        .ok_or("missing tile height")? as usize;
    let count = (i.width() as usize)
        .div_ceil(bx)
        .checked_mul((i.height() as usize).div_ceil(by))
        .filter(|n| *n <= 65_536)
        .ok_or("native COG tile metadata budget exceeded")?;
    if bx
        .checked_mul(by)
        .and_then(|n| n.checked_mul(sample.bytes()))
        .is_none_or(|n| n > MAX_TILE)
    {
        return fail("native COG tile exceeds 2 MiB decode limit");
    }
    let offsets = i.tile_offsets().ok_or("missing tile offsets")?;
    let counts = i.tile_byte_counts().ok_or("missing tile counts")?;
    if offsets.len() != count || counts.len() != count {
        return fail("native COG inconsistent tile arrays");
    }
    let mut tiles = Vec::with_capacity(count);
    for (n, (offset, bytes)) in offsets.into_iter().zip(counts).enumerate() {
        if bytes == 0
            || offset == 0
            || bytes > MAX_TILE as u64 + 65536
            || offset
                .checked_add(bytes)
                .is_none_or(|end| end > tiff.source().len())
        {
            return fail("native COG sparse/oversized/out-of-bounds tile");
        }
        tiles.push(Tile {
            x: n % (i.width() as usize).div_ceil(bx),
            y: n / (i.width() as usize).div_ceil(bx),
            offset: Some(offset),
            bytes: Some(bytes),
        });
    }
    let nodata = match i.tag(42113) {
        None => None,
        Some(tag) => {
            let value = tag
                .value
                .as_str()
                .ok_or("invalid nodata tag")?
                .trim_end_matches('\0')
                .parse::<f64>()?;
            // NaN is already invalid for every Float32 sample comparison. Keep
            // it out of Layout so serde/equality remain reflexive; the exact
            // provider token is retained separately in PhysicalMetadata.
            if sample == SampleType::Float32 && value.is_nan() {
                None
            } else {
                Some(sample.nodata(value).ok_or_else(|| {
                    "native COG nodata must be representable in its storage type".to_string()
                })?)
            }
        }
    };
    Ok(Layout {
        width: i.width() as usize,
        height: i.height() as usize,
        transform,
        crs: format!("EPSG:{epsg}"),
        variable: Variable::Band { index: 1 },
        block: [bx, by],
        dtype: sample.dtype().into(),
        nodata,
        scale: scale_value,
        offset: offset_value,
        tiles,
    })
}
// Preserve scientific calibration without applying it to native samples.
fn calibration(i: &Ifd) -> Result<(Option<f64>, Option<f64>)> {
    let Some(xml) = gdal_metadata_xml(i)? else {
        return Ok((None, None));
    };
    let doc =
        roxmltree::Document::parse(xml).map_err(|_| "native COG malformed GDAL metadata XML")?;
    if doc.root_element().tag_name().name() != "GDALMetadata" {
        return fail("native COG unexpected calibration XML root");
    }
    let (mut scale, mut offset) = (None, None);
    let mut description = false;
    for n in doc.root_element().children().filter(|n| n.is_element()) {
        if n.tag_name().name() != "Item" {
            return fail("native COG unsupported calibration XML element");
        }
        if let Some(role) = n.attribute("role") {
            // NASA HLS stores a single band description beside the numeric
            // calibration items. It has no effect on storage-domain values.
            if role == "description" {
                if n.attribute("sample") != Some("0")
                    || n.attribute("name") != Some("DESCRIPTION")
                    || description
                    || n.children().any(|n| n.is_element())
                    || n.text().is_none_or(|value| value.trim().is_empty())
                {
                    return fail("native COG ambiguous band description metadata");
                }
                description = true;
                continue;
            }
            let slot = match role {
                "scale" => &mut scale,
                "offset" => &mut offset,
                _ => return fail("native COG unsupported GDAL metadata role"),
            };
            if n.attribute("sample") != Some("0")
                || slot.is_some()
                || n.children().any(|n| n.is_element())
            {
                return fail("native COG ambiguous calibration metadata");
            }
            let v = n
                .text()
                .ok_or("empty calibration value")?
                .trim()
                .parse::<f64>()?;
            if !v.is_finite() {
                return fail("nonfinite calibration value");
            }
            *slot = Some(v);
        }
    }
    Ok((scale, offset))
}
pub(crate) fn inspect(a: &Asset, band: usize) -> Result<Layout> {
    let (t, _) = open(a, 0, None, Default::default(), None)?;
    layout(&t, band)
}
pub(crate) struct SelectedReader {
    asset: Asset,
    tiff: TiffFile,
    source: Arc<RangeSource>,
    layout: Layout,
    mask: Option<crate::spatial::Geometry>,
    buffered: Buffered,
    delivery_capacity_peak: usize,
}
enum Buffered {
    Direct,
    Pending(crate::Window),
    Ready {
        window: crate::Window,
        values: crate::Pixels,
    },
}
fn read_pixels(tiff: &TiffFile, sample: SampleType, w: &crate::Window) -> Result<crate::Pixels> {
    macro_rules! read {
        ($ty:ty, $variant:ident) => {{
            let (values, _) = tiff
                .read_window::<$ty>(0, w.y, w.x, w.height, w.width)?
                .into_raw_vec_and_offset();
            crate::Pixels::$variant(values)
        }};
    }
    Ok(match sample {
        SampleType::UInt8 => read!(u8, UInt8),
        SampleType::UInt16 => read!(u16, UInt16),
        SampleType::Int16 => read!(i16, Int16),
        SampleType::Float32 => read!(f32, Float32),
    })
}
fn selection_chunk(
    whole: &crate::Window,
    values: &crate::Pixels,
    w: &crate::Window,
) -> Result<crate::Pixels> {
    let x =
        w.x.checked_sub(whole.x)
            .ok_or("native COG chunk outside buffered selection")?;
    let y =
        w.y.checked_sub(whole.y)
            .ok_or("native COG chunk outside buffered selection")?;
    if x.checked_add(w.width).is_none_or(|end| end > whole.width)
        || y.checked_add(w.height).is_none_or(|end| end > whole.height)
    {
        return fail("native COG chunk outside buffered selection");
    }
    let len = w
        .width
        .checked_mul(w.height)
        .ok_or("native COG chunk size overflow")?;
    fn rows<T: Copy>(
        values: &[T],
        whole: &crate::Window,
        x: usize,
        y: usize,
        w: &crate::Window,
        len: usize,
    ) -> Result<Vec<T>> {
        let mut result = Vec::with_capacity(len);
        for row in y..y + w.height {
            let start = row
                .checked_mul(whole.width)
                .and_then(|n| n.checked_add(x))
                .ok_or("native COG selection offset overflow")?;
            let end = start
                .checked_add(w.width)
                .ok_or("native COG selection offset overflow")?;
            result.extend_from_slice(
                values
                    .get(start..end)
                    .ok_or("native COG selection buffer mismatch")?,
            );
        }
        Ok(result)
    }
    Ok(match values {
        crate::Pixels::UInt8(v) => crate::Pixels::UInt8(rows(v, whole, x, y, w, len)?),
        crate::Pixels::UInt16(v) => crate::Pixels::UInt16(rows(v, whole, x, y, w, len)?),
        crate::Pixels::Int16(v) => crate::Pixels::Int16(rows(v, whole, x, y, w, len)?),
        crate::Pixels::UInt32(_) => return fail("unsupported native COG sample type"),
        crate::Pixels::Float32(v) => crate::Pixels::Float32(rows(v, whole, x, y, w, len)?),
    })
}
fn capacity_bytes(values: &crate::Pixels) -> usize {
    match values {
        crate::Pixels::UInt8(v) => v.capacity(),
        crate::Pixels::UInt16(v) => v.capacity() * 2,
        crate::Pixels::Int16(v) => v.capacity() * 2,
        crate::Pixels::UInt32(v) => v.capacity() * 4,
        crate::Pixels::Float32(v) => v.capacity() * 4,
    }
}
fn apply_mask(values: &mut crate::Pixels, nodata: f64, mask: &[u8]) -> Result<()> {
    macro_rules! replace {
        ($values:expr, $nodata:expr) => {
            for (value, keep) in $values.iter_mut().zip(mask) {
                if *keep == 0 {
                    *value = $nodata;
                }
            }
        };
    }
    match values {
        crate::Pixels::UInt8(v) => replace!(v, nodata as u8),
        crate::Pixels::UInt16(v) => replace!(v, nodata as u16),
        crate::Pixels::Int16(v) => replace!(v, nodata as i16),
        crate::Pixels::Float32(v) => replace!(v, nodata as f32),
        crate::Pixels::UInt32(_) => return fail("unsupported native COG sample type"),
    }
    Ok(())
}
impl SelectedReader {
    pub(crate) fn new(
        asset: &Asset,
        s: &Selected,
        strategy: Strategy,
        snapshots: &Snapshots,
    ) -> Result<Self> {
        let expected = s.layout.as_ref().ok_or("missing native COG layout")?;
        let snapshot = snapshots.get(asset);
        let (tiff, source) = open(
            asset,
            strategy.cache_bytes,
            snapshot.as_deref(),
            snapshots.failed.clone(),
            snapshots.session(asset)?,
        )?;
        let layout = layout(&tiff, 1)?;
        if serde_json::to_value(&layout)? != serde_json::to_value(expected)? {
            snapshots.invalidate();
            return fail("native COG metadata changed since planning");
        }
        let mask = s
            .mask_geometry
            .as_ref()
            .map(|g| crate::spatial::from_geojson(g, &layout.crs))
            .transpose()?;
        source.begin_data(strategy.groups);
        let buffered = match strategy.delivery {
            Delivery::Windows => Buffered::Direct,
            Delivery::Selection(w) => Buffered::Pending(w),
        };
        Ok(Self {
            asset: asset.clone(),
            tiff,
            source,
            layout,
            mask,
            buffered,
            delivery_capacity_peak: 0,
        })
    }
    pub(crate) fn execute_node(
        &mut self,
        node: &Node,
        report: &mut ExecutionReport,
        sink: &mut impl FnMut(Chunk) -> Result<()>,
    ) -> Result<()> {
        let Operation::Read {
            read: Read::LibraryWindow {
                window, band: 1, ..
            },
        } = &node.operation
        else {
            return Ok(());
        };
        if node.asset_id != self.asset.id {
            return fail("native COG node binding mismatch");
        }
        let before = self.source.counters.lock().unwrap().clone();
        let t = Instant::now();
        let sample = sample_type(&self.layout)?;
        if let Buffered::Pending(whole) = &self.buffered {
            let whole = whole.clone();
            let values = read_pixels(&self.tiff, sample, &whole)?;
            self.buffered = Buffered::Ready {
                window: whole,
                values,
            };
        }
        let mut values = match &self.buffered {
            Buffered::Direct => read_pixels(&self.tiff, sample, window)?,
            Buffered::Ready {
                window: whole,
                values,
            } => selection_chunk(whole, values, window)?,
            Buffered::Pending(_) => return fail("native COG selection was not decoded"),
        };
        let elapsed = t.elapsed().as_secs_f64();
        let after = self.source.counters.lock().unwrap().clone();
        let selection_capacity = match &self.buffered {
            Buffered::Ready { values, .. } => capacity_bytes(values),
            _ => 0,
        };
        self.delivery_capacity_peak = self
            .delivery_capacity_peak
            .max(selection_capacity + capacity_bytes(&values));
        report.read_decode_seconds += elapsed;
        report.source_retrieval_seconds =
            Some(report.source_retrieval_seconds.unwrap_or(0.) + after.seconds - before.seconds);
        report.source_data_requests =
            Some(report.source_data_requests.unwrap_or(0) + after.requests - before.requests);
        report.source_data_bytes =
            Some(report.source_data_bytes.unwrap_or(0) + after.bytes - before.bytes);
        if let Some(g) = &self.mask {
            let t = Instant::now();
            let m = crate::spatial::mask(g, &self.layout, window)?;
            self.delivery_capacity_peak = self
                .delivery_capacity_peak
                .max(selection_capacity + capacity_bytes(&values) + m.data().len());
            // Preserve raw samples when the source has no nodata code. The
            // scientific writer carries AOI inclusion independently and folds
            // it into `valid`; inventing a storage sentinel would be lossy.
            if let Some(n) = self.layout.nodata {
                apply_mask(&mut values, n, m.data())?;
            }
            report.masking_seconds += t.elapsed().as_secs_f64();
        }
        report.decoded_bytes += values.byte_len();
        report.non_nodata_pixels += (0..values.len())
            .filter(|i| values.valid(*i, self.layout.nodata))
            .count();
        let c = Chunk {
            node_id: node.id,
            asset_id: self.asset.id.clone(),
            datetime: self.asset.datetime.clone(),
            variable: self.layout.variable.clone(),
            indices: vec![],
            window: window.clone(),
            transform: crate::spatial::window_transform(&self.layout, window),
            crs: self.layout.crs.clone(),
            nodata: self.layout.nodata,
            scale: self.layout.scale,
            offset: self.layout.offset,
            pixels: values,
            coordinates: None,
        };
        let t = Instant::now();
        sink(c)?;
        report.sink_seconds += t.elapsed().as_secs_f64();
        report.chunks += 1;
        Ok(())
    }
    pub(crate) fn finish(&self, report: &mut ExecutionReport) {
        let counters = self.source.counters.lock().unwrap();
        if report.scheduler.is_null() {
            report.scheduler = serde_json::json!({});
        }
        if !report.scheduler["native_buffers"].is_array() {
            report.scheduler["native_buffers"] = serde_json::json!([]);
        }
        report.scheduler["native_buffers"].as_array_mut().unwrap().push(serde_json::json!({
            "asset_id":self.asset.id,
            "transport_encoded_capacity_peak_bytes":counters.encoded_capacity_peak,
            "adapter_delivery_capacity_peak_bytes":self.delivery_capacity_peak,
            "scope":"observed transport and adapter Vec capacities; upstream codec/window allocations are covered by the declared reservation"}));
    }
}

#[cfg(test)]
mod efficiency_tests {
    use super::*;
    #[test]
    fn malformed_wrapped_fallback_reserves_three_encoded_capacities() {
        let decoded = 262_144;
        let encoded = 1_000_000 + 8;
        // Such a declared byte count is admitted by both our narrow profile
        // and upstream's 4*decoded+4096 compressed-input bound. A mismatched
        // leader can make three encoded copies coexist before codec entry.
        assert!(encoded < 4 * decoded + 4096);
        assert_eq!(
            block_working(decoded, encoded).unwrap(),
            3 * encoded + CODEC_WORKING
        );
        // Typical Sentinel encoded tiles remain limited by the decode phase.
        assert_eq!(
            block_working(2 << 20, 1_650_000).unwrap(),
            (4 << 20) + 1_650_000 + CODEC_WORKING
        );
        assert!(block_working(usize::MAX, encoded).is_err());
        assert!(block_working(decoded, usize::MAX).is_err());
    }
    fn fixture(window: crate::Window) -> (Layout, Vec<Node>) {
        let layout = Layout {
            width: 8192,
            height: 4096,
            transform: [0., 1., 0., 0., 0., -1.],
            crs: "EPSG:32617".into(),
            variable: Variable::Band { index: 1 },
            block: [1024, 1024],
            dtype: "UInt16".into(),
            nodata: Some(0.),
            scale: None,
            offset: None,
            tiles: (0..4)
                .flat_map(|y| {
                    (0..8).map(move |x| Tile {
                        x,
                        y,
                        offset: Some(1_000_000 + y as u64 * 20_000_000 + x as u64 * 1_600_008),
                        bytes: Some(1_600_000),
                    })
                })
                .collect(),
        };
        let mut nodes = Vec::new();
        for y in (window.y..window.y + window.height).step_by(256) {
            for x in (window.x..window.x + window.width).step_by(256) {
                let w = crate::Window {
                    x,
                    y,
                    width: 256.min(window.x + window.width - x),
                    height: 256.min(window.y + window.height - y),
                };
                nodes.push(Node {
                    id: nodes.len(),
                    dependencies: vec![],
                    asset_id: "fixture".into(),
                    operation: Operation::Read {
                        read: Read::LibraryWindow {
                            tiles: crate::cog::tiles(&layout, &w),
                            window: w,
                            band: 1,
                        },
                    },
                });
            }
        }
        (layout, nodes)
    }
    #[test]
    fn selection_buffer_preserves_payload_concurrency_and_first_read_tiles() {
        let window = crate::Window {
            x: 7124,
            y: 2004,
            width: 768,
            height: 768,
        };
        let (layout, nodes) = fixture(window.clone());
        let refs: Vec<_> = nodes.iter().collect();
        let window_pair = 256 * 256 * 4;
        let (cache, working, strategy) = reservation(&layout, &window, &refs, 16 << 20, 2).unwrap();
        assert!(matches!(strategy.delivery, Delivery::Selection(_)));
        assert_eq!(cache, 0);
        assert_eq!(strategy.cache_bytes, 0);
        assert!(strategy.groups.is_empty());
        // Two full raw selection/decode/error-capacity phases fit, including
        // prepared metadata. No allocation is moved outside the reservation.
        assert!((working + window_pair) * 2 + 64 * 1024 < 16 << 20);
        let (_, working, strategy) = reservation(&layout, &window, &refs, 32 << 20, 2).unwrap();
        assert_eq!(strategy.groups.len(), 2);
        assert!((working + window_pair) * 2 + 64 * 1024 < 32 << 20);
        let (_, working, strategy) = reservation(&layout, &window, &refs, 32 << 20, 4).unwrap();
        assert!(strategy.groups.is_empty());
        assert!((working + window_pair) * 4 + 64 * 1024 < 32 << 20);
        // Masked holes can remove delivery windows, but may not make an extra
        // source tile eligible. The first remaining read must still need every
        // tile decoded by the bounded selection read.
        let skipped = vec![&nodes[0], &nodes[nodes.len() - 1]];
        let (_, _, strategy) = reservation(&layout, &window, &skipped, 16 << 20, 2).unwrap();
        assert!(matches!(strategy.delivery, Delivery::Selection(_)));
        let mut partial = nodes.clone();
        if let Operation::Read {
            read: Read::LibraryWindow { tiles, .. },
        } = &mut partial[0].operation
        {
            tiles.pop();
        }
        let (_, _, strategy) = reservation(
            &layout,
            &window,
            &partial.iter().collect::<Vec<_>>(),
            16 << 20,
            2,
        )
        .unwrap();
        assert!(matches!(strategy.delivery, Delivery::Windows));
        let earlier = crate::Window {
            x: 6868,
            y: 1748,
            width: 768,
            height: 768,
        };
        let (layout, nodes) = fixture(earlier.clone());
        let (_, _, strategy) = reservation(
            &layout,
            &earlier,
            &nodes.iter().collect::<Vec<_>>(),
            16 << 20,
            2,
        )
        .unwrap();
        assert!(matches!(strategy.delivery, Delivery::Windows));
        let single = crate::Window {
            x: 7300,
            y: 2100,
            width: 96,
            height: 96,
        };
        let (layout, nodes) = fixture(single.clone());
        let (_, _, strategy) = reservation(
            &layout,
            &single,
            &nodes.iter().collect::<Vec<_>>(),
            16 << 20,
            2,
        )
        .unwrap();
        assert!(matches!(strategy.delivery, Delivery::Windows));
    }
    #[test]
    fn buffered_chunk_rows_edges_and_bounds_are_exact() {
        let whole = crate::Window {
            x: 7124,
            y: 2004,
            width: 5,
            height: 4,
        };
        let values = crate::Pixels::UInt16((0..20).collect());
        let w = crate::Window {
            x: 7125,
            y: 2005,
            width: 3,
            height: 2,
        };
        assert_eq!(
            selection_chunk(&whole, &values, &w)
                .unwrap()
                .as_u16()
                .unwrap(),
            [6, 7, 8, 11, 12, 13]
        );
        let edge = crate::Window {
            x: 7128,
            y: 2007,
            width: 1,
            height: 1,
        };
        assert_eq!(
            selection_chunk(&whole, &values, &edge)
                .unwrap()
                .as_u16()
                .unwrap(),
            [19]
        );
        let outside = crate::Window {
            x: 7123,
            ..edge.clone()
        };
        assert!(selection_chunk(&whole, &values, &outside).is_err());
        let short = crate::Pixels::UInt16((0..19).collect());
        assert!(selection_chunk(&whole, &short, &edge).is_err());
    }
    #[test]
    fn retained_physical_metadata_keeps_original_dswx_crs_and_nodata_tokens() {
        let asset = crate::Asset {
            etag: None,
            id: "dswx".into(),
            locator: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data/native_cog/opera_dswx_palette_uint8.tif")
                .to_string_lossy()
                .into(),
            format: crate::Format::Cog,
            datetime: None,
        };
        let snapshots = Snapshots::new(16 * 1024 * 1024);
        let layout = snapshots.inspect(&asset, 1).unwrap();
        assert_eq!(layout.crs, "EPSG:32610");
        let metadata = snapshots.describe("dswx").unwrap();
        assert_eq!(metadata["raster_type"], "PixelIsArea");
        assert_eq!(metadata["gdal_nodata_text"], "255");
        assert_eq!(metadata["normalized_crs"], "EPSG:32610");
        assert_eq!(metadata["geokey_directory"][4], 1024);
        assert_eq!(metadata["geokey_directory"][7], 1);
        assert!(metadata["normalization"]
            .as_str()
            .unwrap()
            .contains("OPERA DSWx-HLS 1.0"));
        assert!(snapshots.bytes() > 0);

        let radar = crate::Asset {
            id: "rtc".into(),
            locator: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data/native_cog/radar_float32_nan_nodata.tif")
                .to_string_lossy()
                .into(),
            ..asset
        };
        let layout = snapshots.inspect(&radar, 1).unwrap();
        assert_eq!(layout.nodata, None);
        assert_eq!(
            snapshots.describe("rtc").unwrap()["gdal_nodata_text"],
            "nan"
        );
    }
    #[test]
    fn lru_uses_smallest_capacity_without_additional_tile_reads() {
        for (window, expected) in [
            ([7300, 2100, 96, 96], 0),
            ([6144, 1024, 2048, 2048], 2),
            ([6868, 1748, 768, 768], 3),
            ([7124, 2004, 768, 768], 3),
        ] {
            let [x, y, width, height] = window;
            let mut sequence = Vec::new();
            for yy in (y..y + height).step_by(256) {
                for xx in (x..x + width).step_by(256) {
                    for ty in yy / 1024..=(yy + 256.min(y + height - yy) - 1) / 1024 {
                        for tx in xx / 1024..=(xx + 256.min(x + width - xx) - 1) / 1024 {
                            sequence.push((tx, ty));
                        }
                    }
                }
            }
            assert_eq!(cache_slots(&sequence, 4), expected);
        }
        assert_eq!(cache_slots(&[(0, 0), (1, 0), (0, 0)], 4), 2);
        assert_eq!(cache_slots(&[], 4), 0);
    }
}
