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

#[derive(Default, Clone)]
pub(super) struct Counters {
    pub requests: u64,
    pub bytes: u64,
    pub seconds: f64,
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
    binding: Binding,
    size: u64,
    metadata: AtomicBool,
    pages: Mutex<BTreeMap<u64, Vec<u8>>>,
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
    pub fn open(a: &Asset) -> Result<Arc<Self>> {
        let (binding, size) = if cog::remote(&a.locator) {
            let etag = a
                .etag
                .as_ref()
                .filter(|e| strong(e))
                .ok_or("native COG requires a pinned strong ETag; use acquisition::prepare")?
                .clone();
            static CLIENT: OnceLock<Client> = OnceLock::new();
            let client = if let Some(c) = CLIENT.get() {
                c.clone()
            } else {
                let c = Client::builder()
                    .timeout(Duration::from_secs(45))
                    .connect_timeout(Duration::from_secs(10))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|_| "native COG client initialization failed")?;
                let _ = CLIENT.set(c.clone());
                c
            };
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
        Ok(Arc::new(Self {
            binding,
            size,
            metadata: AtomicBool::new(true),
            pages: Mutex::new(BTreeMap::new()),
            counters: Mutex::new(Counters::default()),
        }))
    }
    pub fn begin_data(&self) {
        self.pages.lock().unwrap().clear();
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
                    let mut bytes = Vec::with_capacity(len);
                    if r.by_ref()
                        .take(len as u64 + 1)
                        .read_to_end(&mut bytes)
                        .is_err()
                        || bytes.len() != len
                    {
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
        Ok(data)
    }
}
impl TiffSource for RangeSource {
    fn len(&self) -> u64 {
        self.size
    }
    fn read_exact_at(&self, offset: u64, len: usize) -> tiff_reader::error::Result<Vec<u8>> {
        let end = offset
            .checked_add(len as u64)
            .filter(|end| *end <= self.size)
            .ok_or_else(|| error("native COG range outside object"))?;
        if len > MAX_READ {
            return Err(error("native COG range allocation limit exceeded"));
        }
        if !self.metadata.load(Ordering::Acquire) {
            return self.exact(offset, len);
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
                    self.exact(base, (self.size - base).min(PAGE) as usize)?,
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
