//! Version-bound exact range transport for the native TIFF decoder. No URL cache.
use crate::{cog, Asset, Result};
use reqwest::{blocking::Client, header, StatusCode};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tiff_reader::source::{FileSource, TiffSource};
const PAGE: u64 = 4096;
const METADATA_CACHE: usize = 512 * 1024;
const MAX_READ: usize = 8 * 1024 * 1024;

/// Private immutable preparation snapshot, scoped to one prepared request.
/// These pages are used only while parsing metadata, never for tile decoding.
pub(crate) struct Snapshot {
    locator: String,
    etag: Option<String>,
    size: u64,
    pages: BTreeMap<u64, Arc<Vec<u8>>>,
}
impl Snapshot {
    pub(crate) fn bytes(&self) -> usize {
        self.pages.values().map(|p| p.capacity()).sum()
    }
}

#[derive(Default, Clone)]
pub(super) struct Counters {
    pub requests: u64,
    pub bytes: u64,
    pub seconds: f64,
    pub encoded_capacity_peak: usize,
}
#[derive(Clone, Debug)]
pub(crate) struct Group {
    offset: u64,
    pub(crate) len: usize,
    members: Vec<(u64, usize)>,
}
impl Group {
    pub(super) fn first_needed_together(&self, first: &BTreeMap<u64, usize>) -> bool {
        let Some(node) = first.get(&self.members[0].0) else {
            return false;
        };
        self.members
            .iter()
            .all(|member| first.get(&member.0) == Some(node))
    }
}
/// Join only selected tiles separated by a GDAL COG's <=8 wrapper bytes.
/// Vertical row gaps and unrelated compressed tiles are never prefetched.
pub(super) fn groups(mut tiles: Vec<crate::Tile>, cap: usize) -> Vec<Group> {
    let sequence: Vec<_> = tiles.iter().filter_map(|t| t.offset).collect();
    tiles.sort_by_key(|t| t.offset);
    tiles.dedup_by_key(|t| t.offset);
    let mut groups: Vec<Group> = Vec::new();
    for tile in tiles {
        let (Some(offset), Some(bytes)) = (tile.offset, tile.bytes) else {
            continue;
        };
        let Ok(len) = usize::try_from(bytes) else {
            continue;
        };
        if let Some(group) = groups.last_mut() {
            let end = group.offset + group.len as u64;
            if offset >= end && offset - end <= 8 {
                if let Some(combined) = offset
                    .checked_add(bytes)
                    .and_then(|end| end.checked_sub(group.offset))
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| n.checked_add(8).is_some_and(|n| n <= cap.min(MAX_READ)))
                {
                    group.len = combined;
                    group.members.push((offset, len));
                    continue;
                }
            }
        }
        groups.push(Group {
            offset,
            len,
            members: vec![(offset, len)],
        });
    }
    groups.retain(|g| {
        let positions: Vec<_> = sequence
            .iter()
            .enumerate()
            .filter(|(_, offset)| g.members.iter().any(|member| member.0 == **offset))
            .map(|(i, _)| i)
            .collect();
        g.members.len() > 1
            && positions.len() == g.members.len()
            && positions.last().unwrap() - positions[0] + 1 == positions.len()
    });
    groups
}
struct Encoded {
    group: usize,
    offset: u64,
    bytes: Vec<u8>,
    remaining: Vec<(u64, usize)>,
}
enum Binding {
    Local(FileSource),
    Http {
        client: Client,
        url: String,
        etag: String,
    },
}
pub(super) struct RangeSource {
    failed: Arc<AtomicBool>,
    binding: Binding,
    size: u64,
    metadata: AtomicBool,
    pages: Mutex<BTreeMap<u64, Arc<Vec<u8>>>>,
    groups: std::sync::OnceLock<Vec<Group>>,
    encoded: Mutex<Option<Encoded>>,
    pub counters: Mutex<Counters>,
}
fn error(s: &str) -> tiff_reader::TiffError {
    tiff_reader::TiffError::Other(s.into())
}
fn strong(etag: &str) -> bool {
    etag.len() >= 2
        && etag.starts_with('"')
        && etag.ends_with('"')
        && !etag[1..etag.len() - 1]
            .bytes()
            .any(|b| b < 33 || b == 127 || b == b'"')
}
impl RangeSource {
    pub fn open(
        a: &Asset,
        snapshot: Option<&Snapshot>,
        failed: Arc<AtomicBool>,
    ) -> Result<Arc<Self>> {
        if failed.load(Ordering::Acquire) {
            return Err(
                "native COG preparation invalidated by an earlier source failure; prepare again"
                    .into(),
            );
        }
        let result = Self::open_bound(a, snapshot, failed.clone());
        if result.is_err() {
            failed.store(true, Ordering::Release);
        } else if failed.load(Ordering::Acquire) {
            return Err(
                "native COG preparation invalidated by an earlier source failure; prepare again"
                    .into(),
            );
        }
        result
    }
    fn open_bound(
        a: &Asset,
        snapshot: Option<&Snapshot>,
        failed: Arc<AtomicBool>,
    ) -> Result<Arc<Self>> {
        let (binding, size) = if cog::remote(&a.locator) {
            let etag = a
                .etag
                .as_ref()
                .filter(|e| strong(e))
                .ok_or("native COG requires a pinned strong ETag; use acquisition::prepare")?
                .clone();
            // Initialization is atomic too: parallel inspection must share one
            // connection pool from its first requests, not race to build pools.
            static CLIENT: OnceLock<std::result::Result<Client, &'static str>> = OnceLock::new();
            let client = CLIENT
                .get_or_init(|| {
                    Client::builder()
                        .timeout(Duration::from_secs(45))
                        .connect_timeout(Duration::from_secs(10))
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .map_err(|_| "native COG client initialization failed")
                })
                .as_ref()
                .map_err(|e| *e)?
                .clone();
            let r = (0..=2)
                .find_map(|attempt| {
                    if attempt > 0 {
                        std::thread::sleep(Duration::from_millis(100 * attempt));
                    }
                    match client
                        .head(&a.locator)
                        .header(header::IF_MATCH, &etag)
                        .header(header::ACCEPT_ENCODING, "identity")
                        .send()
                    {
                        Ok(r)
                            if r.status().is_server_error()
                                || r.status() == StatusCode::TOO_MANY_REQUESTS =>
                        {
                            None
                        }
                        other => Some(other),
                    }
                })
                .ok_or("native COG HEAD retries exhausted")?
                .map_err(|_| "native COG HEAD transport failed")?;
            if r.status() != StatusCode::OK
                || r.headers().get(header::ETAG).and_then(|v| v.to_str().ok())
                    != Some(etag.as_str())
            {
                return Err("native COG source version/HEAD conflict".into());
            }
            let size = r
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|s| *s > 0)
                .ok_or("native COG HEAD missing object size")?;
            (
                Binding::Http {
                    client,
                    url: a.locator.clone(),
                    etag,
                },
                size,
            )
        } else {
            let source = FileSource::open(std::path::Path::new(&a.locator))
                .map_err(|_| "native COG local source open failed")?;
            let size = source.len();
            (Binding::Local(source), size)
        };
        if snapshot.is_some_and(|s| s.locator != a.locator || s.etag != a.etag || s.size != size) {
            return Err("native COG prepared metadata identity/size conflict".into());
        }
        Ok(Arc::new(Self {
            failed,
            binding,
            size,
            metadata: AtomicBool::new(true),
            pages: Mutex::new(snapshot.map(|s| s.pages.clone()).unwrap_or_default()),
            groups: OnceLock::new(),
            encoded: Mutex::new(None),
            counters: Mutex::new(Counters::default()),
        }))
    }
    pub fn snapshot(&self, a: &Asset) -> Snapshot {
        Snapshot {
            locator: a.locator.clone(),
            etag: a.etag.clone(),
            size: self.size,
            pages: self.pages.lock().unwrap().clone(),
        }
    }
    pub fn begin_data(&self, groups: Vec<Group>) {
        self.pages.lock().unwrap().clear();
        let _ = self.groups.set(groups);
        self.metadata.store(false, Ordering::Release);
        *self.counters.lock().unwrap() = Counters::default();
    }
    fn exact(&self, offset: u64, len: usize) -> tiff_reader::error::Result<Vec<u8>> {
        let end = offset
            .checked_add(len as u64)
            .filter(|end| *end <= self.size)
            .ok_or_else(|| error("native COG range outside object"))?;
        if len > MAX_READ {
            return Err(error("native COG encoded range exceeds 8 MiB limit"));
        }
        if len == 0 {
            return Ok(vec![]);
        }
        let started = Instant::now();
        let data = match &self.binding {
            Binding::Local(source) => source
                .read_exact_at(offset, len)
                .map_err(|_| error("native COG truncated local read"))?,
            Binding::Http { client, url, etag } => {
                let mut output = None;
                for attempt in 0..=2 {
                    if attempt > 0 {
                        std::thread::sleep(Duration::from_millis(100 * attempt));
                    }
                    let response = client
                        .get(url)
                        .header(header::RANGE, format!("bytes={offset}-{}", end - 1))
                        .header(header::IF_MATCH, etag)
                        .header(header::ACCEPT_ENCODING, "identity")
                        .send();
                    let mut r = match response {
                        Ok(r)
                            if r.status().is_server_error()
                                || r.status() == StatusCode::TOO_MANY_REQUESTS =>
                        {
                            continue
                        }
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let expected = format!("bytes {offset}-{}/{}", end - 1, self.size);
                    if r.status() != StatusCode::PARTIAL_CONTENT
                        || r.headers().get(header::ETAG).and_then(|v| v.to_str().ok())
                            != Some(etag.as_str())
                        || r.headers()
                            .get(header::CONTENT_RANGE)
                            .and_then(|v| v.to_str().ok())
                            != Some(expected.as_str())
                        || r.headers()
                            .get(header::CONTENT_ENCODING)
                            .is_some_and(|v| v != "identity")
                        || r.headers()
                            .get(header::CONTENT_LENGTH)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<usize>().ok())
                            .is_some_and(|n| n != len)
                    {
                        return Err(error(
                            "native COG invalid range response or source version conflict",
                        ));
                    }
                    // A len+1 read_to_end probe can double Vec capacity even
                    // for an exact body. Keep encoded allocation exactly len.
                    let mut bytes = vec![0; len];
                    let mut extra = [0u8; 1];
                    if r.read_exact(&mut bytes).is_err() || !matches!(r.read(&mut extra), Ok(0)) {
                        continue;
                    }
                    output = Some(bytes);
                    break;
                }
                output.ok_or_else(|| error("native COG range failed after bounded retries"))?
            }
        };
        let mut count = self.counters.lock().unwrap();
        count.requests += 1;
        count.bytes += data.len() as u64;
        count.seconds += started.elapsed().as_secs_f64();
        count.encoded_capacity_peak = count.encoded_capacity_peak.max(data.capacity());
        Ok(data)
    }
}
impl TiffSource for RangeSource {
    fn len(&self) -> u64 {
        self.size
    }
    fn read_exact_at(&self, offset: u64, len: usize) -> tiff_reader::error::Result<Vec<u8>> {
        if self.failed.load(Ordering::Acquire) {
            return Err(error(
                "native COG preparation invalidated by an earlier source failure; prepare again",
            ));
        }
        let result = self.read_bound(offset, len);
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        } else if self.failed.load(Ordering::Acquire) {
            return Err(error(
                "native COG preparation invalidated by an earlier source failure; prepare again",
            ));
        }
        result
    }
}
impl RangeSource {
    fn read_bound(&self, offset: u64, len: usize) -> tiff_reader::error::Result<Vec<u8>> {
        let end = offset
            .checked_add(len as u64)
            .filter(|end| *end <= self.size)
            .ok_or_else(|| error("native COG range outside object"))?;
        if len > MAX_READ {
            return Err(error("native COG range allocation limit exceeded"));
        }
        if !self.metadata.load(Ordering::Acquire) {
            let groups = self.groups.get().map(Vec::as_slice).unwrap_or(&[]);
            let Some((index, group, member, wrapper)) =
                groups.iter().enumerate().find_map(|(index, g)| {
                    g.members.iter().find_map(|member| {
                        if *member == (offset, len) {
                            Some((index, g, *member, 0))
                        } else if member.0.checked_sub(4) == Some(offset)
                            && (len == member.1 + 4 || len == member.1 + 8)
                        {
                            Some((index, g, *member, len - member.1))
                        } else {
                            None
                        }
                    })
                })
            else {
                return self.exact(offset, len);
            };
            let mut encoded = self.encoded.lock().unwrap();
            let first = if wrapper == 0 {
                group.offset
            } else {
                let Some(first) = group.offset.checked_sub(4) else {
                    return self.exact(offset, len);
                };
                first
            };
            if encoded
                .as_ref()
                .is_none_or(|b| b.group != index || b.offset != first)
            {
                // Drop previous encoded capacity before admitting another GET.
                *encoded = None;
                *encoded = Some(Encoded {
                    group: index,
                    offset: first,
                    bytes: self.exact(first, group.len + wrapper)?,
                    remaining: group.members.clone(),
                });
            }
            let block = encoded.as_mut().unwrap();
            let start = (offset - block.offset) as usize;
            let result = block
                .bytes
                .get(
                    start
                        ..start
                            .checked_add(len)
                            .ok_or_else(|| error("native COG coalesced range overflow"))?,
                )
                .ok_or_else(|| error("native COG coalesced range mismatch"))?
                .to_vec();
            let mut count = self.counters.lock().unwrap();
            count.encoded_capacity_peak = count
                .encoded_capacity_peak
                .max(block.bytes.capacity() + result.capacity());
            drop(count);
            block.remaining.retain(|r| *r != member);
            if block.remaining.is_empty() {
                *encoded = None;
            }
            return Ok(result);
        }
        if len > METADATA_CACHE {
            return Err(error("native COG metadata range budget exceeded"));
        }
        let mut result = Vec::with_capacity(len);
        let mut pages = self.pages.lock().unwrap();
        let mut at = offset;
        while at < end {
            let base = at / PAGE * PAGE;
            if !pages.contains_key(&base) {
                if (pages.len() + 1) * PAGE as usize > METADATA_CACHE {
                    return Err(error("native COG metadata range budget exceeded"));
                }
                pages.insert(
                    base,
                    Arc::new(self.exact(base, (self.size - base).min(PAGE) as usize)?),
                );
            }
            let page = &pages[&base];
            let first = (at - base) as usize;
            let n = (end - at).min(page.len() as u64 - first as u64) as usize;
            result.extend_from_slice(&page[first..first + n]);
            at += n as u64;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    fn tile(offset: u64, bytes: u64) -> crate::Tile {
        crate::Tile {
            x: offset as usize,
            y: 0,
            offset: Some(offset),
            bytes: Some(bytes),
        }
    }
    struct Fixture(Asset);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tile-ripper-range-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&path, (0..8192).map(|n| n as u8).collect::<Vec<_>>()).unwrap();
            Self(Asset {
                id: "fixture".into(),
                locator: path.to_string_lossy().into(),
                etag: None,
                format: crate::Format::Cog,
                datetime: None,
            })
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0.locator);
        }
    }

    #[test]
    fn source_failure_survives_restore_and_invalidates_cached_and_uncached_readers() {
        let f = Fixture::new();
        let failed = Arc::new(AtomicBool::new(false));
        let first = RangeSource::open(&f.0, None, failed.clone()).unwrap();
        let second = RangeSource::open(&f.0, None, failed.clone()).unwrap();
        second.read_exact_at(0, 8).unwrap();
        first.begin_data(Vec::new());
        std::fs::write(&f.0.locator, b"x").unwrap();
        assert!(first.read_exact_at(0, 8).is_err());
        std::fs::write(&f.0.locator, (0..8192).map(|n| n as u8).collect::<Vec<_>>()).unwrap();
        assert!(first.read_exact_at(0, 8).is_err());
        assert!(second.read_exact_at(0, 8).is_err());
        assert!(RangeSource::open(&f.0, None, failed).is_err());
        assert!(RangeSource::open(&f.0, None, Default::default()).is_ok());
    }
    #[test]
    fn coalescing_requires_bounded_adjacent_consecutive_misses() {
        let a = tile(100, 100);
        let b = tile(208, 100);
        assert_eq!(groups(vec![a.clone(), b.clone()], 216).len(), 1);
        assert!(groups(vec![a.clone(), b.clone()], 215).is_empty());
        assert!(groups(vec![a.clone(), tile(209, 100)], 1024).is_empty());
        assert!(groups(vec![a.clone(), tile(1000, 100), b.clone()], 1024).is_empty());
        assert!(groups(vec![a.clone(), b, a], 1024).is_empty());
    }
    #[test]
    fn merged_wrapped_and_direct_reads_preserve_bytes_and_counters() {
        let f = Fixture::new();
        let original = std::fs::read(&f.0.locator).unwrap();
        for wrapper in [0, 4, 8] {
            let source = RangeSource::open(&f.0, None, Default::default()).unwrap();
            source.begin_data(groups(vec![tile(100, 100), tile(208, 100)], 216));
            let shift = if wrapper == 0 { 0 } else { 4 };
            for offset in [100 - shift, 208 - shift] {
                let got = source.read_exact_at(offset, 100 + wrapper).unwrap();
                assert_eq!(
                    got,
                    original[offset as usize..offset as usize + 100 + wrapper]
                );
                assert_eq!(got.capacity(), 100 + wrapper);
            }
            let count = source.counters.lock().unwrap();
            assert_eq!(count.requests, 1);
            assert_eq!(count.bytes, (208 + wrapper) as u64);
            assert!(source.encoded.lock().unwrap().is_none());
        }
    }
    #[test]
    fn metadata_snapshots_are_identity_bound_and_never_supply_data() {
        let f = Fixture::new();
        let first = RangeSource::open(&f.0, None, Default::default()).unwrap();
        first.read_exact_at(0, 16).unwrap();
        let snapshot = first.snapshot(&f.0);
        assert_eq!(snapshot.bytes(), PAGE as usize);
        let second = RangeSource::open(&f.0, Some(&snapshot), Default::default()).unwrap();
        assert_eq!(
            second.read_exact_at(0, 16).unwrap(),
            (0..16).collect::<Vec<u8>>()
        );
        assert_eq!(second.counters.lock().unwrap().requests, 0);
        second.begin_data(vec![]);
        second.read_exact_at(0, 16).unwrap();
        assert_eq!(second.counters.lock().unwrap().requests, 1);
        let mut changed = f.0.clone();
        changed.etag = Some("\"different\"".into());
        assert!(RangeSource::open(&changed, Some(&snapshot), Default::default()).is_err());
        std::fs::write(&f.0.locator, [0; 8193]).unwrap();
        assert!(RangeSource::open(&f.0, Some(&snapshot), Default::default()).is_err());
    }
}
