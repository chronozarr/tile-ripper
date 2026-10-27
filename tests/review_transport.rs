//! Hermetic HTTP controls run in the normal feature matrix, without providers.
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tile_ripper::{acquisition, cog::LayoutCache, Request};

type Objects = Arc<Mutex<BTreeMap<String, (Vec<u8>, usize)>>>;
struct Server {
    objects: Objects,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    url: String,
    pause_data: Arc<AtomicBool>,
    data_started: Arc<AtomicBool>,
    release_data: Arc<AtomicBool>,
}
impl Server {
    fn new(missing: bool) -> Self {
        let mut objects = BTreeMap::new();
        let mut insert = |key: &str, bytes: Vec<u8>| {
            objects.insert(key.to_owned(), (bytes, 1));
        };
        insert(".zgroup", br#"{"zarr_format":2}"#.to_vec());
        insert(
            ".zattrs",
            br#"{"tile_ripper_grid":{"crs":"EPSG:32617","transform":[0,1,0,2,0,-1]}}"#.to_vec(),
        );
        for (name, dtype, shape, dims, bytes) in [
            (
                "x",
                "<f8",
                vec![2],
                vec!["x"],
                [0.5f64, 1.5]
                    .into_iter()
                    .flat_map(f64::to_le_bytes)
                    .collect::<Vec<_>>(),
            ),
            (
                "y",
                "<f8",
                vec![2],
                vec!["y"],
                [1.5f64, 0.5]
                    .into_iter()
                    .flat_map(f64::to_le_bytes)
                    .collect(),
            ),
            (
                "time",
                "<i8",
                vec![1],
                vec!["time"],
                0i64.to_le_bytes().to_vec(),
            ),
            (
                "value",
                "<u2",
                vec![1, 2, 2],
                vec!["time", "y", "x"],
                [1u16, 2, 3, 4]
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ),
        ] {
            insert(&format!("{name}/.zarray"), serde_json::to_vec(&serde_json::json!({"zarr_format":2,"shape":shape,"chunks":shape,"dtype":dtype,"order":"C","filters":null,"compressor":null,"fill_value":0})).unwrap());
            let mut attrs = serde_json::json!({"_ARRAY_DIMENSIONS":dims});
            if name == "time" {
                attrs["units"] = "seconds since 1970-01-01 00:00:00".into();
                attrs["calendar"] = "proleptic_gregorian".into();
            }
            insert(
                &format!("{name}/.zattrs"),
                serde_json::to_vec(&attrs).unwrap(),
            );
            if name != "value" || !missing {
                insert(
                    &format!("{name}/{}", if name == "value" { "0.0.0" } else { "0" }),
                    bytes,
                );
            }
        }
        let objects = Arc::new(Mutex::new(objects));
        let stopped = Arc::new(AtomicBool::new(false));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let state = objects.clone();
        let stop = stopped.clone();
        let pause_data = Arc::new(AtomicBool::new(false));
        let data_started = Arc::new(AtomicBool::new(false));
        let release_data = Arc::new(AtomicBool::new(false));
        let (pause, started, release) = (
            pause_data.clone(),
            data_started.clone(),
            release_data.clone(),
        );
        let worker = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("fixture accept failed: {e}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&request);
                let mut lines = request.lines();
                let first = lines
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<_>>();
                if first.len() < 2 {
                    continue;
                }
                let head = first[0] == "HEAD";
                let key = first[1].trim_start_matches('/');
                if !head && key == "value/0.0.0" && pause.load(Ordering::Acquire) {
                    started.store(true, Ordering::Release);
                    while !release.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(2));
                    }
                }
                let headers = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
                    .collect::<BTreeMap<_, _>>();
                let object = state.lock().unwrap().get(key).cloned();
                let etag = object
                    .as_ref()
                    .map(|(_, version)| format!("\"fixture-{version}\""));
                let conflict = headers
                    .get("if-match")
                    .is_some_and(|expected| Some(expected) != etag.as_ref())
                    || (headers
                        .get("if-none-match")
                        .is_some_and(|value| value == "*")
                        && object.is_some());
                let status = if conflict {
                    "412 Precondition Failed"
                } else if object.is_some() {
                    "200 OK"
                } else {
                    "404 Not Found"
                };
                let body = if conflict {
                    Vec::new()
                } else {
                    object.map(|(bytes, _)| bytes).unwrap_or_default()
                };
                let version_header = etag
                    .map(|etag| format!("ETag: {etag}\r\n"))
                    .unwrap_or_default();
                let header = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n{version_header}Connection: close\r\n\r\n",body.len());
                if stream.write_all(header.as_bytes()).is_ok() && !head {
                    let _ = stream.write_all(&body);
                }
            }
        });
        Self {
            objects,
            stopped,
            worker: Some(worker),
            url,
            pause_data,
            data_started,
            release_data,
        }
    }
    fn request(&self) -> Request {
        serde_json::from_value(serde_json::json!({
            "source":{"kind":"assets","assets":[{"id":"fixture","locator":self.url,"format":"zarr_v2","datetime":null}]},
            "spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[0,0,2,2]},
            "time":null,"variables":[{"kind":"dataset","path":"value"}],
            "grid":{"kind":"native"},"sink":{"kind":"callback"},
            "budget":{"buffer_bytes":1048576,"max_nodes":1000,"chunk_edge":2}
        })).unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.release_data.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn http_chunks_have_a_validated_completion_receipt() {
    let server = Server::new(false);
    let prepared = acquisition::prepare(&server.request(), &mut LayoutCache::default()).unwrap();
    let mut values = Vec::new();
    let report = prepared
        .execute(|chunk| {
            values.extend_from_slice(chunk.pixels.as_u16().unwrap());
            Ok(())
        })
        .unwrap();
    assert_eq!(values, [1, 2, 3, 4]);
    assert_eq!(report.chunks, 1);
}

#[test]
fn metadata_changed_after_delivery_prevents_completion() {
    let server = Server::new(false);
    let prepared = acquisition::prepare(&server.request(), &mut LayoutCache::default()).unwrap();
    let delivered = AtomicUsize::new(0);
    let error = prepared
        .execute(|_| {
            delivered.fetch_add(1, Ordering::Relaxed);
            server.objects.lock().unwrap().get_mut(".zattrs").unwrap().1 += 1;
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert_eq!(delivered.load(Ordering::Relaxed), 1);
    assert!(error.contains("version conflict"), "{error}");
}

#[test]
fn a_previously_missing_chunk_cannot_appear_before_completion() {
    let server = Server::new(true);
    let prepared = acquisition::prepare(&server.request(), &mut LayoutCache::default()).unwrap();
    let error = prepared
        .execute(|chunk| {
            assert_eq!(chunk.pixels.as_u16().unwrap(), [0; 4]);
            server
                .objects
                .lock()
                .unwrap()
                .insert("value/0.0.0".into(), (vec![1; 8], 2));
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("version conflict") || error.contains("appeared"),
        "{error}"
    );
}

#[test]
fn abrupt_exit_during_zarr_fetch_leaves_only_hidden_staging_and_retry_succeeds() {
    use std::{
        fs,
        path::PathBuf,
        process::{Child, Command, Stdio},
        time::Instant,
    };
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Running(Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let temporary = Temporary(std::env::temp_dir().join(format!(
        "tile-ripper-interrupted-http-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir(&temporary.0).unwrap();
    let server = Server::new(false);
    server.pause_data.store(true, Ordering::Release);
    let request_file = temporary.0.join("request.json");
    fs::write(
        &request_file,
        serde_json::to_vec(&server.request()).unwrap(),
    )
    .unwrap();
    let output = temporary.0.join("result");
    let mut running = Running(
        Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
            .arg("fetch")
            .arg(&request_file)
            .arg("--output")
            .arg(&output)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while !server.data_started.load(Ordering::Acquire) {
        assert!(
            running.0.try_wait().unwrap().is_none(),
            "fetch exited before requesting data"
        );
        assert!(
            Instant::now() < deadline,
            "fetch did not reach its data read"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let stages = || {
        fs::read_dir(&temporary.0)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                path.file_name()?
                    .to_str()?
                    .starts_with(".result.partial-")
                    .then_some(path)
            })
            .collect::<Vec<_>>()
    };
    assert!(!output.exists(), "an unfinished output became visible");
    let partials = stages();
    assert_eq!(partials.len(), 1);
    assert!(partials[0].join("fixture.zarr/.zgroup").is_file());
    running.0.kill().unwrap();
    assert!(!running.0.wait().unwrap().success());
    server.release_data.store(true, Ordering::Release);
    assert!(!output.exists());
    assert!(
        partials[0].is_dir(),
        "abrupt exit unexpectedly ran staging cleanup"
    );
    let retry = Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .arg("fetch")
        .arg(&request_file)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["status"], "complete");
    assert_eq!(
        stages(),
        partials,
        "retry adopted or removed an abandoned stage"
    );
}
