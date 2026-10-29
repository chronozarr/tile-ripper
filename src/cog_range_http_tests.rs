//! The initial metadata read remains conditional, bounded and separate from data.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::atomic::AtomicUsize,
    thread::{self, JoinHandle},
};

#[derive(Clone, Debug)]
struct Request {
    method: String,
    range: Option<String>,
    if_match: Option<String>,
}
struct HttpFixture {
    asset: Asset,
    object: Vec<u8>,
    requests: Arc<Mutex<Vec<Request>>>,
    fault: Arc<AtomicUsize>,
    head_size: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl HttpFixture {
    fn new(size: usize) -> Self {
        Self::from_bytes((0..size).map(|i| i as u8).collect())
    }
    fn from_bytes(object: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let asset = Asset {
            id: "metadata-http".into(),
            locator: format!("http://{}/source.tif", listener.local_addr().unwrap()),
            etag: Some("\"v1\"".into()),
            format: crate::Format::Cog,
            datetime: None,
        };
        let head_size = Arc::new(AtomicUsize::new(object.len()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let fault = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let (bytes, received, bad, stop, size) = (
            object.clone(),
            requests.clone(),
            fault.clone(),
            stopped.clone(),
            head_size.clone(),
        );
        let worker = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(_) => break,
                };
                // BSD sockets can inherit the listener's nonblocking mode.
                // Give the client a bounded opportunity to send its headers.
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let method = line.split_whitespace().next().unwrap().to_owned();
                let mut headers = BTreeMap::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let (key, value) = line.split_once(':').unwrap();
                    headers.insert(key.to_ascii_lowercase(), value.trim().to_owned());
                }
                let range = headers.get("range").cloned();
                received.lock().unwrap().push(Request {
                    method: method.clone(),
                    range: range.clone(),
                    if_match: headers.get("if-match").cloned(),
                });
                let response = if method == "HEAD" {
                    format!(
                        "HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        size.load(Ordering::Acquire)
                    ).into_bytes()
                } else {
                    assert_eq!(method, "GET");
                    let (start, end) = range
                        .as_deref()
                        .unwrap()
                        .strip_prefix("bytes=")
                        .unwrap()
                        .split_once('-')
                        .unwrap();
                    let (start, end): (usize, usize) =
                        (start.parse().unwrap(), end.parse().unwrap());
                    let mode = bad.load(Ordering::Acquire);
                    let etag = if mode == 1 { "\"changed\"" } else { "\"v1\"" };
                    let reported_start = if mode == 2 { start + 1 } else { start };
                    let encoding = if mode == 3 {
                        "Content-Encoding: gzip\r\n"
                    } else {
                        ""
                    };
                    let length = end - start + 1;
                    let reported_length = if mode == 4 { length + 1 } else { length };
                    let mut response = format!(
                        "HTTP/1.1 206 Partial Content\r\nETag: {etag}\r\nContent-Range: bytes {reported_start}-{end}/{}\r\nContent-Length: {reported_length}\r\n{encoding}Connection: close\r\n\r\n",
                        bytes.len()
                    ).into_bytes();
                    let body_end = if mode == 5 { end } else { end + 1 };
                    response.extend_from_slice(&bytes[start..body_end]);
                    response
                };
                let _ = socket.write_all(&response);
            }
        });
        Self {
            asset,
            object,
            requests,
            fault,
            head_size,
            stopped,
            worker: Some(worker),
        }
    }
    fn gets(&self) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "GET")
            .cloned()
            .collect()
    }
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn initial_remote_header_and_second_page_share_one_conditional_range() {
    let f = HttpFixture::new(3 * PAGE as usize + 17);
    let source = RangeSource::open(&f.asset, None, Default::default(), None).unwrap();
    for (offset, len) in [(0, 16), (PAGE + 13, 33), (PAGE - 8, 16)] {
        assert_eq!(
            source.read_exact_at(offset, len).unwrap(),
            f.object[offset as usize..offset as usize + len]
        );
    }
    let snapshot = source.snapshot(&f.asset);
    assert_eq!(snapshot.bytes(), 2 * PAGE as usize);
    assert_eq!(snapshot.pages.len(), 2);
    assert_eq!(
        source.counters.lock().unwrap().encoded_capacity_peak,
        2 * PAGE as usize
    );
    assert!(snapshot
        .pages
        .values()
        .all(|page| page.capacity() == PAGE as usize));
    assert_eq!(f.gets().len(), 1);
    assert_eq!(f.gets()[0].range.as_deref(), Some("bytes=0-8191"));
    assert!(f
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.if_match.as_deref() == Some("\"v1\"")));
    // Later metadata misses retain the ordinary single-page request size.
    assert_eq!(
        source.read_exact_at(2 * PAGE + 1, 10).unwrap(),
        f.object[8193..8203]
    );
    assert_eq!(f.gets()[1].range.as_deref(), Some("bytes=8192-12287"));
    source.begin_data(vec![]);
    assert!(source.pages.lock().unwrap().is_empty());
    assert_eq!(source.read_exact_at(0, 16).unwrap(), f.object[..16]);
    assert_eq!(f.gets()[2].range.as_deref(), Some("bytes=0-15"));
    assert_eq!(source.counters.lock().unwrap().requests, 1);
    assert_eq!(source.counters.lock().unwrap().bytes, 16);
}

#[test]
fn initial_remote_read_ahead_is_clipped_to_the_exact_object_size() {
    for size in [17, PAGE as usize + 113] {
        let f = HttpFixture::new(size);
        let source = RangeSource::open(&f.asset, None, Default::default(), None).unwrap();
        assert_eq!(source.read_exact_at(0, 8).unwrap(), f.object[..8]);
        assert_eq!(
            source.read_exact_at(size as u64 - 1, 1).unwrap(),
            f.object[size - 1..]
        );
        assert_eq!(f.gets().len(), 1);
        assert_eq!(f.gets()[0].range, Some(format!("bytes=0-{}", size - 1)));
        assert_eq!(source.snapshot(&f.asset).bytes(), size);
    }
}

#[test]
fn read_ahead_still_enforces_the_per_source_metadata_cache_cap() {
    let f = HttpFixture::new(METADATA_CACHE + PAGE as usize);
    let source = RangeSource::open(&f.asset, None, Default::default(), None).unwrap();
    source.read_exact_at(0, 8).unwrap();
    source
        .read_exact_at(2 * PAGE, METADATA_CACHE - 2 * PAGE as usize)
        .unwrap();
    assert_eq!(source.snapshot(&f.asset).bytes(), METADATA_CACHE);
    assert_eq!(f.gets().len(), METADATA_CACHE / PAGE as usize - 1);
    assert!(source
        .read_exact_at(METADATA_CACHE as u64, 1)
        .unwrap_err()
        .to_string()
        .contains("metadata range budget"));
    assert_eq!(f.gets().len(), METADATA_CACHE / PAGE as usize - 1);
}

#[test]
fn invalid_initial_remote_ranges_poison_the_preparation_before_caching_pages() {
    for mode in 1..=4 {
        let f = HttpFixture::new(3 * PAGE as usize);
        let failed = Arc::new(AtomicBool::new(false));
        let source = RangeSource::open(&f.asset, None, failed.clone(), None).unwrap();
        f.fault.store(mode, Ordering::Release);
        assert!(source
            .read_exact_at(0, 8)
            .unwrap_err()
            .to_string()
            .contains("invalid range response or source version conflict"));
        assert!(failed.load(Ordering::Acquire));
        assert!(source.pages.lock().unwrap().is_empty());
        let count = f.requests.lock().unwrap().len();
        assert!(source.read_exact_at(0, 8).is_err());
        assert!(RangeSource::open(&f.asset, None, failed, None).is_err());
        assert_eq!(f.requests.lock().unwrap().len(), count);
    }
}

#[test]
fn truncated_initial_remote_body_is_retried_and_never_cached() {
    let f = HttpFixture::new(3 * PAGE as usize);
    let failed = Arc::new(AtomicBool::new(false));
    let source = RangeSource::open(&f.asset, None, failed.clone(), None).unwrap();
    f.fault.store(5, Ordering::Release);
    assert!(source
        .read_exact_at(0, 8)
        .unwrap_err()
        .to_string()
        .contains("range failed after bounded retries"));
    assert!(failed.load(Ordering::Acquire));
    assert!(source.pages.lock().unwrap().is_empty());
    assert_eq!(f.gets().len(), 3);
    assert!(source.read_exact_at(0, 8).is_err());
    assert_eq!(f.gets().len(), 3);
}

fn cog_fixture() -> HttpFixture {
    HttpFixture::from_bytes(
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data/native_cog/hls_int16.tif"),
        )
        .unwrap(),
    )
}
fn native_backend_test() -> bool {
    if crate::cog::native_selected() {
        return true;
    }
    // Exercise the native reader even when GDAL is the compiled default,
    // without changing another test's process-global backend environment.
    let name = std::thread::current().name().unwrap().to_owned();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--test-threads=1"])
        .env("TILE_RIPPER_COG_BACKEND", "native")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    false
}
fn cog_request(f: &HttpFixture) -> crate::Request {
    serde_json::from_value(serde_json::json!({
        "source":{"kind":"assets","assets":[f.asset.clone()]},
        "spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[500000.,3999040.,500960.,4000000.]},
        "variables":[{"kind":"band","index":1}],"grid":{"kind":"native"},
        "sink":{"kind":"callback"},"budget":{"buffer_bytes":16777216,"max_nodes":128,"chunk_edge":64}
    })).unwrap()
}
fn heads(f: &HttpFixture) -> usize {
    f.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.method == "HEAD")
        .count()
}

#[test]
fn acquisition_receipts_remove_only_adjacent_heads_and_revalidate_every_execution() {
    if !native_backend_test() {
        return;
    }
    let f = cog_fixture();
    let prepared =
        crate::acquisition::prepare(&cog_request(&f), &mut crate::cog::LayoutCache::default())
            .unwrap();
    assert_eq!(heads(&f), 1);
    let mut previous = Vec::new();
    for iteration in 0..2 {
        let mut values = Vec::new();
        prepared
            .execute(|chunk| {
                values.extend_from_slice(chunk.pixels.as_i16().unwrap());
                Ok(())
            })
            .unwrap();
        assert_eq!(heads(&f), 3 + iteration * 2);
        if iteration == 0 {
            previous = values;
        } else {
            assert_eq!(values, previous);
        }
    }
    // Same ETag is insufficient: a later execution must freshly reject length
    // changes rather than reuse either prior execution's identity receipt.
    f.head_size.store(f.object.len() + 1, Ordering::Release);
    let mut delivered = 0;
    assert!(prepared
        .execute(|_| {
            delivered += 1;
            Ok(())
        })
        .unwrap_err()
        .to_string()
        .contains("length differs"));
    assert_eq!(delivered, 0);
    assert_eq!(heads(&f), 6);
    f.head_size.store(f.object.len(), Ordering::Release);
    assert!(prepared.execute(|_| Ok(())).is_err());
    assert_eq!(heads(&f), 6);
}

#[test]
fn concurrent_executions_each_own_a_fresh_receipt_and_postvalidation() {
    if !native_backend_test() {
        return;
    }
    let f = cog_fixture();
    let prepared =
        crate::acquisition::prepare(&cog_request(&f), &mut crate::cog::LayoutCache::default())
            .unwrap();
    let (entered, ready) = std::sync::mpsc::channel();
    thread::scope(|scope| {
        let mut releases = Vec::new();
        let mut workers = Vec::new();
        for _ in 0..2 {
            let prepared = &prepared;
            let entered = entered.clone();
            let (release, wait) = std::sync::mpsc::channel();
            releases.push(release);
            workers.push(scope.spawn(move || {
                prepared.execute(|chunk| {
                    assert_eq!(chunk.pixels.len(), 32 * 32);
                    entered.send(())?;
                    wait.recv_timeout(Duration::from_secs(5))?;
                    Ok(())
                })
            }));
        }
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(heads(&f), 3); // resolution plus one fresh pass per execution
        for release in releases {
            release.send(()).unwrap();
        }
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
    });
    assert_eq!(heads(&f), 5); // both executions performed fresh final HEADs
}

#[test]
fn fresh_postvalidation_rejects_same_etag_length_changes_and_data_conflicts() {
    if !native_backend_test() {
        return;
    }
    for change_length in [true, false] {
        let f = cog_fixture();
        let prepared =
            crate::acquisition::prepare(&cog_request(&f), &mut crate::cog::LayoutCache::default())
                .unwrap();
        if change_length {
            let error = prepared
                .execute(|_| {
                    f.head_size.store(f.object.len() + 1, Ordering::Release);
                    Ok(())
                })
                .unwrap_err()
                .to_string();
            assert!(error.contains("length differs"));
            assert_eq!(heads(&f), 3);
        } else {
            f.fault.store(1, Ordering::Release);
            let mut delivered = 0;
            assert!(prepared
                .execute(|_| {
                    delivered += 1;
                    Ok(())
                })
                .is_err());
            assert_eq!(delivered, 0);
            assert!(prepared.execute(|_| Ok(())).is_err());
        }
    }
}

#[test]
fn receipt_and_length_guards_do_not_depend_on_retained_metadata_pages() {
    if !native_backend_test() {
        return;
    }
    let f = cog_fixture();
    let mut request = cog_request(&f);
    let identities =
        crate::stac_workflow::resolve_identities(&mut request, &mut [], &Default::default())
            .unwrap();
    let mut sources = crate::storage::Sources::new(&request).unwrap();
    sources.install_cog_identities(identities);
    let plan = crate::planner::plan_on(&request, &mut crate::cog::LayoutCache::default(), &sources)
        .unwrap();
    sources.clear_cog_reader_identities();
    sources.native.trim(0);
    assert_eq!(sources.native.summary()["retained_sources"], 0);
    assert_eq!(heads(&f), 1);
    let view = sources
        .validated_cog_readers(&request, &Default::default())
        .unwrap();
    assert!(sources.native.reader_identities.is_none());
    let other_view = sources
        .validated_cog_readers(&request, &Default::default())
        .unwrap();
    assert!(!Arc::ptr_eq(
        view.native.reader_identities.as_ref().unwrap(),
        other_view.native.reader_identities.as_ref().unwrap()
    ));
    crate::scheduler::execute(&request, &plan, 0., &view, &AtomicBool::new(false), |_| {
        Ok(())
    })
    .unwrap();
    sources
        .validate_cog_identities(&request, &Default::default())
        .unwrap();
    assert_eq!(heads(&f), 4);
    assert!(f.gets().len() > 1); // execution reread its own parser pages
}

#[test]
fn absent_receipts_fall_back_to_heads_and_foreign_bindings_fail_closed() {
    let f = HttpFixture::new(3 * PAGE as usize);
    let request = cog_request(&f);
    let identities =
        crate::stac_workflow::validate_identities(&request, &Default::default()).unwrap();
    let identity = identities.get(&f.asset).unwrap().unwrap();
    for changed in ["id", "locator", "etag"] {
        let mut asset = f.asset.clone();
        match changed {
            "id" => asset.id.push_str("other"),
            "locator" => asset.locator.push_str("?other"),
            _ => asset.etag = Some("\"other\"".into()),
        }
        assert!(RangeSource::open_with_identity(
            &asset,
            None,
            Default::default(),
            None,
            Some(identity)
        )
        .is_err());
    }
    assert_eq!(heads(&f), 1);
    RangeSource::open(&f.asset, None, Default::default(), None).unwrap();
    assert_eq!(heads(&f), 2);
}

#[test]
fn nonpositive_lengths_are_rejected_during_preparation_and_prevalidation() {
    if !native_backend_test() {
        return;
    }
    for during_preparation in [true, false] {
        let f = cog_fixture();
        if during_preparation {
            f.head_size.store(0, Ordering::Release);
        }
        let prepared =
            crate::acquisition::prepare(&cog_request(&f), &mut crate::cog::LayoutCache::default());
        if during_preparation {
            assert!(prepared.is_err());
            assert!(f.gets().is_empty());
        } else {
            let prepared = prepared.unwrap();
            f.head_size.store(0, Ordering::Release);
            assert!(prepared.execute(|_| Ok(())).is_err());
        }
    }
}
