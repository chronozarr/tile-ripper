//! Identity pool reuse must never become a cache of source versions.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    thread::{self, JoinHandle},
};

#[derive(Clone)]
struct Reply {
    etag: Option<String>,
    length: Option<u64>,
    status: u16,
}
struct Fixture {
    endpoint: String,
    reply: Arc<Mutex<Reply>>,
    connections: Arc<AtomicUsize>,
    heads: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_object(None)
    }
    fn with_object(object: Option<Vec<u8>>) -> Self {
        let object = object.map(Arc::new);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!(
            "http://{}/source.tif?sig=test-only-secret",
            listener.local_addr().unwrap()
        );
        let reply = Arc::new(Mutex::new(Reply {
            etag: Some("\"v1\"".into()),
            length: Some(object.as_ref().map_or(123, |v| v.len() as u64)),
            status: 200,
        }));
        let connections = Arc::new(AtomicUsize::new(0));
        let heads = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (r, c, h, s) = (
            reply.clone(),
            connections.clone(),
            heads.clone(),
            stop.clone(),
        );
        let worker = thread::spawn(move || {
            let mut handlers = Vec::new();
            while !s.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        c.fetch_add(1, Ordering::Relaxed);
                        let (r, h, s, o) = (r.clone(), h.clone(), s.clone(), object.clone());
                        handlers.push(thread::spawn(move || serve(socket, r, h, s, o)));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(_) => break,
                }
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        Self {
            endpoint,
            reply,
            connections,
            heads,
            stop,
            worker: Some(worker),
        }
    }
    fn request(&self) -> Request {
        serde_json::from_value(json!({
            "source":{"kind":"assets","assets":[{"id":"one","locator":self.endpoint,"format":"cog","etag":"\"v1\""}]},
            "spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[0,0,1,1]},
            "variables":[{"kind":"band","index":1}],"grid":{"kind":"native"},
            "sink":{"kind":"callback"},"budget":{"buffer_bytes":4194304,"max_nodes":100,"chunk_edge":32}
        })).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn serve(
    mut socket: TcpStream,
    reply: Arc<Mutex<Reply>>,
    heads: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    object: Option<Arc<Vec<u8>>>,
) {
    socket
        .set_read_timeout(Some(Duration::from_millis(25)))
        .unwrap();
    let mut reader = BufReader::new(socket.try_clone().unwrap());
    while !stop.load(Ordering::Acquire) {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(_) => break,
            _ => {}
        }
        let head = line.starts_with("HEAD ");
        let mut headers = std::collections::BTreeMap::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                return;
            }
            if line == "\r\n" {
                break;
            }
            let (key, value) = line.split_once(':').unwrap();
            headers.insert(key.to_ascii_lowercase(), value.trim().to_owned());
        }
        if !head {
            assert_eq!(headers.get("if-match").map(String::as_str), Some("\"v1\""));
            let (start, end) = headers["range"]
                .strip_prefix("bytes=")
                .unwrap()
                .split_once('-')
                .unwrap();
            let (start, end): (usize, usize) = (start.parse().unwrap(), end.parse().unwrap());
            let object = object.as_ref().unwrap();
            let header = format!("HTTP/1.1 206 Partial Content\r\nETag: \"v1\"\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n", object.len(),end-start+1);
            if socket
                .write_all(header.as_bytes())
                .and_then(|_| socket.write_all(&object[start..=end]))
                .is_err()
            {
                break;
            }
            continue;
        }
        heads.fetch_add(1, Ordering::Relaxed);
        let r = reply.lock().unwrap().clone();
        let mut response = format!("HTTP/1.1 {} Result\r\nConnection: keep-alive\r\n", r.status);
        if r.status == 302 {
            response.push_str("Location: /redirected.tif?token=test-redirect-secret\r\n");
        }
        if let Some(etag) = r.etag {
            response.push_str(&format!("ETag: {etag}\r\n"));
        }
        if let Some(length) = r.length {
            response.push_str(&format!("Content-Length: {length}\r\n"));
        }
        response.push_str("\r\n");
        if socket.write_all(response.as_bytes()).is_err() {
            break;
        }
    }
}

fn limits() -> Limits {
    Limits {
        max_retries: 0,
        timeout_seconds: 2,
        ..Default::default()
    }
}

#[test]
#[cfg(feature = "native-cog")]
fn catalog_prepare_and_execution_use_scoped_receipts_but_keep_fresh_boundary_checks() {
    if !crate::cog::native_selected() {
        return;
    }
    let object = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/native_cog/hls_int16.tif"),
    )
    .unwrap();
    let size = object.len() as u64;
    let f = Fixture::with_object(Some(object));
    let mut core = f.request();
    core.spatial = Spatial::Bounds {
        crs: "EPSG:32617".into(),
        bbox: [500000., 3999040., 500960., 4000000.],
    };
    let catalog = StacRequest {
        endpoint: f.endpoint.clone(),
        collection: "sentinel-2-c1-l2a".into(),
        aoi: core.spatial.clone(),
        start: "2025-07-13T00:00:00Z".into(),
        end: "2025-07-14T00:00:00Z".into(),
        bands: vec!["B04".into()],
        output: "unused-callback".into(),
        provider: Provider::EarthSearch,
        limits: limits(),
    };
    let discovered = Discovered {
        request: catalog,
        sources: Vec::new(),
        core,
        geographic_bounds: [-81., 36., -80., 37.],
        report: DiscoveryReport::default(),
    };
    let prepared = discovered.prepare(&mut LayoutCache::default()).unwrap();
    assert_eq!(f.heads.load(Ordering::Relaxed), 1);
    prepared
        .execute(|chunk| {
            assert_eq!(chunk.pixels.len(), 32 * 32);
            Ok(())
        })
        .unwrap();
    assert_eq!(f.heads.load(Ordering::Relaxed), 3);
    f.reply.lock().unwrap().length = Some(size + 1);
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
    assert_eq!(f.heads.load(Ordering::Relaxed), 4);
}

#[test]
fn identity_pool_reuses_connections_but_rechecks_every_phase() {
    let f = Fixture::new();
    let request = f.request();
    for _ in 0..3 {
        let identities = remote_identities(&request, &limits()).unwrap();
        assert_eq!(identities[0].1.etag, "\"v1\"");
        assert_eq!(identities[0].1.bytes, 123);
    }
    assert_eq!(f.heads.load(Ordering::Relaxed), 3);
    assert_eq!(f.connections.load(Ordering::Relaxed), 1);
    f.reply.lock().unwrap().etag = Some("\"changed\"".into());
    let error = remote_identities(&request, &limits())
        .unwrap_err()
        .to_string();
    assert!(error.contains("version conflict"));
    assert!(!error.contains("test-only-secret"));
    assert_eq!(f.heads.load(Ordering::Relaxed), 4);
    assert_eq!(f.connections.load(Ordering::Relaxed), 1);
}

#[test]
fn pooled_identity_checks_still_reject_missing_and_weak_metadata() {
    let f = Fixture::new();
    let request = f.request();
    for (etag, length) in [
        (None, Some(123)),
        (Some("W/\"weak\"".into()), Some(123)),
        (Some("unquoted".into()), Some(123)),
        (Some("\"has space\"".into()), Some(123)),
        (Some("\"v1\"".into()), None),
        (Some("\"v1\"".into()), Some(0)),
    ] {
        *f.reply.lock().unwrap() = Reply {
            etag,
            length,
            status: 200,
        };
        assert!(remote_identities(&request, &limits()).is_err());
    }
    assert_eq!(f.heads.load(Ordering::Relaxed), 6);
}

#[test]
fn pooled_identity_checks_do_not_follow_redirects() {
    let f = Fixture::new();
    f.reply.lock().unwrap().status = 302;
    let error = remote_identities(&f.request(), &limits())
        .unwrap_err()
        .to_string();
    assert!(!error.contains("test-only-secret"));
    assert_eq!(f.heads.load(Ordering::Relaxed), 1);
}

#[test]
fn identity_receipts_reject_other_success_statuses_even_with_plausible_headers() {
    let f = Fixture::new();
    for status in [204, 206] {
        *f.reply.lock().unwrap() = Reply {
            etag: Some("\"v1\"".into()),
            length: Some(123),
            status,
        };
        assert!(validate_identities(&f.request(), &limits())
            .err()
            .unwrap()
            .to_string()
            .contains("requires HTTP 200"));
    }
    assert_eq!(f.heads.load(Ordering::Relaxed), 2);
}
