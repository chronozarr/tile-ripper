//! Version-bound exact range transport for the native TIFF decoder. No URL cache.
use crate::{cog, Asset, Result};
use reqwest::{blocking::Client, header, Method, StatusCode, Url};
use std::{
    collections::BTreeMap,
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
pub(super) fn groups(tiles: Vec<crate::Tile>, cap: usize) -> Vec<Group> {
    let sequence: Vec<_> = tiles.iter().filter_map(|t| t.offset).collect();
    let mut groups = physical_groups(tiles, cap);
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

fn physical_groups(mut tiles: Vec<crate::Tile>, cap: usize) -> Vec<Group> {
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
    groups
}

/// Recover first-use subgroups without fetching tiles needed by a later node.
/// The ordinary `groups` path above retains its bounded-selection behavior.
pub(super) fn groups_by_first_use(
    tiles: Vec<crate::Tile>,
    first: &BTreeMap<u64, usize>,
    cap: usize,
) -> Vec<Group> {
    // Count before filtering or partitioning. A repeated miss must never become
    // eligible merely because its other occurrence lies outside this subgroup.
    // Keep positions for every declared offset, even if its byte count is absent.
    let mut uses = BTreeMap::<u64, (usize, usize)>::new();
    for (position, offset) in tiles.iter().filter_map(|t| t.offset).enumerate() {
        uses.entry(offset)
            .and_modify(|(_, count)| *count += 1)
            .or_insert((position, 1));
    }
    let mut result = Vec::new();
    for run in physical_groups(tiles, cap) {
        let mut start = 0;
        while start < run.members.len() {
            let offset = run.members[start].0;
            let Some(node) = first.get(&offset) else {
                start += 1;
                continue;
            };
            if !matches!(uses.get(&offset), Some((_, 1))) {
                start += 1;
                continue;
            }
            let mut end = start + 1;
            while end < run.members.len()
                && first.get(&run.members[end].0) == Some(node)
                && matches!(uses.get(&run.members[end].0), Some((_, 1)))
            {
                end += 1;
            }
            salvage_first_use(&run.members[start..end], &uses, first, cap, &mut result);
            start = end;
        }
    }
    result
}

fn salvage_first_use(
    members: &[(u64, usize)],
    uses: &BTreeMap<u64, (usize, usize)>,
    first: &BTreeMap<u64, usize>,
    cap: usize,
    result: &mut Vec<Group>,
) {
    if members.len() < 2 {
        return;
    }
    // Check the whole interval before splitting: physical [A,B,C] consumed as
    // [B,C,A] is legal even though its [A,B] prefix is not consecutive.
    if let Some(group) = first_use_group(members, uses, first, cap) {
        result.push(group);
        return;
    }
    // Conservative salvage, not an optimal interval partition. Borrowed halves
    // bound recursion depth and avoid copying rejected tiles or member vectors.
    let middle = members.len() / 2;
    salvage_first_use(&members[..middle], uses, first, cap, result);
    salvage_first_use(&members[middle..], uses, first, cap, result);
}

fn first_use_group(
    members: &[(u64, usize)],
    uses: &BTreeMap<u64, (usize, usize)>,
    first: &BTreeMap<u64, usize>,
    cap: usize,
) -> Option<Group> {
    let mut minimum = usize::MAX;
    let mut maximum = 0;
    for (offset, _) in members {
        let (position, count) = uses.get(offset)?;
        if *count != 1 {
            return None;
        }
        minimum = minimum.min(*position);
        maximum = maximum.max(*position);
    }
    if maximum.checked_sub(minimum)?.checked_add(1)? != members.len() {
        return None;
    }
    for pair in members.windows(2) {
        let end = pair[0].0.checked_add(pair[0].1 as u64)?;
        if pair[1].0.checked_sub(end)? > 8 {
            return None;
        }
    }
    let offset = members.first()?.0;
    let last = members.last()?;
    let len = usize::try_from(last.0.checked_add(last.1 as u64)?.checked_sub(offset)?).ok()?;
    if len.checked_add(8)? > cap.min(MAX_READ) {
        return None;
    }
    let group = Group {
        offset,
        len,
        members: members.to_vec(),
    };
    group.first_needed_together(first).then_some(group)
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
        url: Url,
        etag: String,
        transport: crate::http_transport::Transport,
    },
}
#[cfg(feature = "authenticated-http")]
pub(super) type Session = Arc<crate::authenticated_http::AuthenticatedHttp>;
#[cfg(not(feature = "authenticated-http"))]
pub(super) type Session = ();
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
        session: Option<Session>,
    ) -> Result<Arc<Self>> {
        Self::open_with_identity(a, snapshot, failed, session, None)
    }
    pub(super) fn open_with_identity(
        a: &Asset,
        snapshot: Option<&Snapshot>,
        failed: Arc<AtomicBool>,
        session: Option<Session>,
        identity: Option<&crate::stac_workflow::CogIdentity>,
    ) -> Result<Arc<Self>> {
        if failed.load(Ordering::Acquire) {
            return Err(
                "native COG preparation invalidated by an earlier source failure; prepare again"
                    .into(),
            );
        }
        let result = Self::open_bound(a, snapshot, failed.clone(), session, identity);
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
        session: Option<Session>,
        identity: Option<&crate::stac_workflow::CogIdentity>,
    ) -> Result<Arc<Self>> {
        let (binding, size) = if cog::remote(&a.locator) {
            let etag = a
                .etag
                .as_ref()
                .filter(|e| strong(e))
                .ok_or("native COG requires a pinned strong ETag; use acquisition::prepare")?
                .clone();
            // Reuse the identity check's anonymous connection pool. Authenticated
            // sessions still own their separate exact-object transport.
            let client = crate::http_transport::cog_client()?;
            let url = Url::parse(&a.locator).map_err(|_| "invalid native COG URL")?;
            let fresh_size = if session.is_none() {
                identity.map(|identity| identity.size_for(a)).transpose()?
            } else {
                None
            };
            #[cfg(feature = "authenticated-http")]
            let transport = session
                .map(crate::http_transport::Transport::Authenticated)
                .unwrap_or_default();
            #[cfg(not(feature = "authenticated-http"))]
            let transport = {
                let _ = session;
                crate::http_transport::Transport::default()
            };
            let size = if let Some(size) = fresh_size {
                size
            } else {
                let r = (0..=2)
                    .find_map(|attempt| {
                        if attempt > 0 {
                            std::thread::sleep(Duration::from_millis(100 * attempt));
                        }
                        let mut headers = header::HeaderMap::new();
                        headers
                            .insert(header::IF_MATCH, header::HeaderValue::from_str(&etag).ok()?);
                        match transport.send(&client, Method::HEAD, url.clone(), headers, None) {
                            Ok(r)
                                if r.status() >= 500
                                    || r.status() == StatusCode::TOO_MANY_REQUESTS.as_u16() =>
                            {
                                None
                            }
                            other => Some(other),
                        }
                    })
                    .ok_or("native COG HEAD retries exhausted")?
                    .map_err(|_| "native COG HEAD transport failed")?;
                if r.status() != StatusCode::OK.as_u16()
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
                drop(r);
                size
            };
            (
                Binding::Http {
                    client,
                    url,
                    etag,
                    transport,
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
            Binding::Http {
                client,
                url,
                etag,
                transport,
            } => {
                let mut output = None;
                for attempt in 0..=2 {
                    if attempt > 0 {
                        std::thread::sleep(Duration::from_millis(100 * attempt));
                    }
                    let mut headers = header::HeaderMap::new();
                    let range = format!("bytes={offset}-{}", end - 1);
                    let (Ok(range), Ok(etag_header)) = (
                        header::HeaderValue::from_str(&range),
                        header::HeaderValue::from_str(etag),
                    ) else {
                        return Err(error("native COG invalid conditional range headers"));
                    };
                    headers.insert(header::RANGE, range);
                    headers.insert(header::IF_MATCH, etag_header);
                    let response = transport.send(client, Method::GET, url.clone(), headers, None);
                    let mut r = match response {
                        Ok(r)
                            if r.status() >= 500
                                || r.status() == StatusCode::TOO_MANY_REQUESTS.as_u16() =>
                        {
                            continue
                        }
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let expected = format!("bytes {offset}-{}/{}", end - 1, self.size);
                    if r.status() != StatusCode::PARTIAL_CONTENT.as_u16()
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
                    let mut bytes = Vec::with_capacity(len);
                    if r.read_body(len as u64, &mut bytes).is_err() || bytes.len() != len {
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
                // The first remote parser read usually needs both header
                // pages. Read ahead at most one page, only while this cache
                // is empty; retain ordinary pages so all later reads and
                // snapshot accounting keep their existing 4 KiB bounds.
                let initial_remote =
                    base == 0 && pages.is_empty() && matches!(&self.binding, Binding::Http { .. });
                let bytes = (self.size - base).min(if initial_remote { 2 * PAGE } else { PAGE });
                let additional = bytes.div_ceil(PAGE) as usize;
                if (pages.len() + additional) * PAGE as usize > METADATA_CACHE {
                    return Err(error("native COG metadata range budget exceeded"));
                }
                let response = self.exact(base, bytes as usize)?;
                if response.len() > PAGE as usize {
                    // The response plus split copies peak at 16 KiB while
                    // the cache is empty, below its 512 KiB plane limit.
                    // The parser's requested result allocation is unchanged.
                    for (index, page) in response.chunks(PAGE as usize).enumerate() {
                        pages.insert(base + index as u64 * PAGE, Arc::new(page.to_vec()));
                    }
                } else {
                    pages.insert(base, Arc::new(response));
                }
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
#[path = "cog_range_http_tests.rs"]
mod http_tests;

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
        let first = RangeSource::open(&f.0, None, failed.clone(), None).unwrap();
        let second = RangeSource::open(&f.0, None, failed.clone(), None).unwrap();
        second.read_exact_at(0, 8).unwrap();
        first.begin_data(Vec::new());
        std::fs::write(&f.0.locator, b"x").unwrap();
        assert!(first.read_exact_at(0, 8).is_err());
        std::fs::write(&f.0.locator, (0..8192).map(|n| n as u8).collect::<Vec<_>>()).unwrap();
        assert!(first.read_exact_at(0, 8).is_err());
        assert!(second.read_exact_at(0, 8).is_err());
        assert!(RangeSource::open(&f.0, None, failed, None).is_err());
        assert!(RangeSource::open(&f.0, None, Default::default(), None).is_ok());
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
    fn first_use_salvages_rejected_same_node_intervals() {
        for (offsets, expected) in [
            (
                vec![100, 208, 1000, 316, 424],
                vec![vec![(100, 100), (208, 100)], vec![(316, 100), (424, 100)]],
            ),
            (
                vec![100, 1000, 208, 316, 2000, 424, 532, 3000, 640],
                // Bisection conservatively salvages this internal B/C pair;
                // it does not promise an optimal partition of every interval.
                vec![vec![(208, 100), (316, 100)]],
            ),
        ] {
            let tiles: Vec<_> = offsets.iter().map(|offset| tile(*offset, 100)).collect();
            let first = offsets.into_iter().map(|offset| (offset, 0)).collect();
            assert!(groups(tiles.clone(), 4096).is_empty());
            let actual = groups_by_first_use(tiles, &first, 4096);
            assert_eq!(
                actual.iter().map(|g| g.members.clone()).collect::<Vec<_>>(),
                expected
            );
            assert!(actual.iter().all(|g| g.first_needed_together(&first)));
        }
    }
    #[test]
    fn first_use_splits_nodes_before_merging_and_preserves_permutations() {
        let tiles = vec![
            tile(100, 100),
            tile(208, 100),
            tile(316, 100),
            tile(424, 100),
        ];
        let first = BTreeMap::from([(100, 0), (208, 0), (316, 1), (424, 1)]);
        let old = groups(tiles.clone(), 1024);
        assert_eq!(old.len(), 1);
        assert!(!old[0].first_needed_together(&first));
        let actual = groups_by_first_use(tiles, &first, 1024);
        assert_eq!(actual.len(), 2);
        assert_eq!(actual[0].members, vec![(100, 100), (208, 100)]);
        assert_eq!(actual[1].members, vec![(316, 100), (424, 100)]);

        // Physical A/B/C is consumed B/C/A. Test the complete interval before
        // considering prefixes, so this already-legal permutation stays legal.
        let tiles = vec![tile(208, 100), tile(316, 100), tile(100, 100)];
        let first = BTreeMap::from([(100, 0), (208, 0), (316, 0)]);
        let old = groups(tiles.clone(), 324);
        let actual = groups_by_first_use(tiles, &first, 324);
        assert_eq!(old.len(), 1);
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].members, old[0].members);
        assert_eq!(actual[0].len, old[0].len);
    }
    #[test]
    fn first_use_counts_repeats_and_unusable_offsets_before_partitioning() {
        let first = BTreeMap::from([(100, 0), (208, 0), (316, 1), (1000, 0)]);
        // Counting within the first-node partition would incorrectly admit A/B.
        assert!(groups_by_first_use(
            vec![
                tile(100, 100),
                tile(208, 100),
                tile(316, 100),
                tile(100, 100)
            ],
            &first,
            1024
        )
        .is_empty());
        let mut unusable = tile(1000, 100);
        unusable.bytes = None;
        // Its declared offset still interrupts A/B's global logical interval.
        assert!(
            groups_by_first_use(vec![tile(100, 100), unusable, tile(208, 100)], &first, 1024)
                .is_empty()
        );
    }
    #[test]
    fn first_use_keeps_gap_wrapper_capacity_and_max_read_limits() {
        let first = BTreeMap::from([(100, 0), (208, 0), (209, 0)]);
        let tiles = vec![tile(100, 100), tile(208, 100)];
        assert_eq!(groups_by_first_use(tiles.clone(), &first, 216).len(), 1);
        assert!(groups_by_first_use(tiles, &first, 215).is_empty());
        assert!(groups_by_first_use(vec![tile(100, 100), tile(209, 100)], &first, 1024).is_empty());

        for (half, expected) in [(MAX_READ / 2 - 4, 1), (MAX_READ / 2, 0)] {
            let second = 100 + half as u64;
            let first = BTreeMap::from([(100, 0), (second, 0)]);
            let actual = groups_by_first_use(
                vec![tile(100, half as u64), tile(second, half as u64)],
                &first,
                usize::MAX,
            );
            assert_eq!(actual.len(), expected);
            assert!(actual.iter().all(|g| g.len + 8 <= MAX_READ));
        }
    }
    #[test]
    fn merged_wrapped_and_direct_reads_preserve_bytes_and_counters() {
        let f = Fixture::new();
        let original = std::fs::read(&f.0.locator).unwrap();
        for wrapper in [0, 4, 8] {
            let source = RangeSource::open(&f.0, None, Default::default(), None).unwrap();
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
        let first = RangeSource::open(&f.0, None, Default::default(), None).unwrap();
        first.read_exact_at(0, 16).unwrap();
        let snapshot = first.snapshot(&f.0);
        assert_eq!(snapshot.bytes(), PAGE as usize);
        let second = RangeSource::open(&f.0, Some(&snapshot), Default::default(), None).unwrap();
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
        assert!(RangeSource::open(&changed, Some(&snapshot), Default::default(), None).is_err());
        std::fs::write(&f.0.locator, [0; 8193]).unwrap();
        assert!(RangeSource::open(&f.0, Some(&snapshot), Default::default(), None).is_err());
    }
}
