#![cfg(feature = "icechunk")]
//! Native-library generated repositories exercise real snapshot/reference parsing,
//! the ordinary conditional HTTP path, and public plan/stream/fetch integration.
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
};
use tile_ripper::{acquisition, cog::LayoutCache, *};
use zarrs::storage::{AsyncReadableStorageTraits, AsyncWritableStorageTraits, StoreKey};
use zarrs_icechunk::AsyncIcechunkStore;
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Server {
    root: PathBuf,
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(root: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let files = root.clone();
        let handle = thread::spawn(move || {
            while !flag.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => serve(&mut stream, &files),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            root,
            url,
            stop,
            thread: Some(handle),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn serve(stream: &mut TcpStream, root: &std::path::Path) {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    while !request.windows(4).any(|v| v == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buffer[..n]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let first = request.lines().next().unwrap();
    let mut words = first.split_whitespace();
    let method = words.next().unwrap();
    let key = words.next().unwrap().trim_start_matches('/');
    if key.split('/').any(|p| p == "..") {
        return;
    }
    let bytes = match fs::read(root.join(key)) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            return;
        }
    };
    let etag = format!(
        "\"{:x}\"",
        bytes
            .iter()
            .fold(0xcbf29ce484222325u64, |n, b| (n ^ *b as u64)
                .wrapping_mul(0x100000001b3))
    );
    let header = |name: &str| {
        request.lines().skip(1).find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    };
    if header("if-match").is_some_and(|v| v != etag) {
        let _ = write!(
            stream,
            "HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        return;
    }
    let (start, end, status) = if let Some(range) = header("range") {
        let Some(range) = range.strip_prefix("bytes=") else {
            return;
        };
        let (a, b) = range.split_once('-').unwrap();
        let start = if a.is_empty() {
            bytes.len() - b.parse::<usize>().unwrap()
        } else {
            a.parse().unwrap()
        };
        let end = if a.is_empty() || b.is_empty() {
            bytes.len()
        } else {
            b.parse::<usize>().unwrap() + 1
        };
        (start, end, 206)
    } else {
        (0, bytes.len(), 200)
    };
    if start > end || end > bytes.len() {
        let _ = write!(
            stream,
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        return;
    }
    let _=write!(stream,"HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nETag: {etag}\r\nLast-Modified: Mon, 01 Jan 2024 00:00:00 GMT\r\nConnection: close\r\n",end-start);
    if status == 206 {
        let _ = write!(
            stream,
            "Content-Range: bytes {}-{}/{}\r\n",
            start,
            end - 1,
            bytes.len()
        );
    }
    let _ = write!(stream, "\r\n");
    if method != "HEAD" {
        let _ = stream.write_all(&bytes[start..end]);
    }
}
fn fixture() -> (
    Server,
    tokio::runtime::Runtime,
    icechunk::Repository,
    Request,
    String,
) {
    let root = std::env::temp_dir().join(format!(
        "tile-ripper-icechunk-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (repo,snapshot)=runtime.block_on(async{
        let storage=icechunk::new_local_filesystem_storage(&root.join("repo")).await.unwrap();
        let repo=icechunk::Repository::create(Some(icechunk::RepositoryConfig { inline_chunk_threshold_bytes: Some(0), ..Default::default() }),storage,Default::default(),Default::default(),true).await.unwrap();
        let store=AsyncIcechunkStore::new(repo.writable_session("main").await.unwrap());
        store.set(&StoreKey::new("zarr.json").unwrap(),serde_json::to_vec(&serde_json::json!({"zarr_format":3,"node_type":"group","attributes":{"tile_ripper_grid":{"crs":"EPSG:32617"}}})).unwrap().into()).await.unwrap();
        for (name,dtype,shape,chunks,dims,attrs) in [
            ("x","float64",vec![2],vec![2],vec!["x"],serde_json::json!({})),
            ("y","float64",vec![2],vec![2],vec!["y"],serde_json::json!({})),
            ("time","int64",vec![1],vec![1],vec!["time"],serde_json::json!({"calendar":"proleptic_gregorian","units":"seconds since 2025-01-01 00:00:00"})),
            ("temperature","float64",vec![1,2,2],vec![1,2,2],vec!["time","y","x"],serde_json::json!({"units":"K"})),
        ] {
            let m=serde_json::json!({"zarr_format":3,"node_type":"array","shape":shape,"data_type":dtype,"chunk_grid":{"name":"regular","configuration":{"chunk_shape":chunks}},"chunk_key_encoding":{"name":"default","configuration":{"separator":"/"}},"fill_value":0,"codecs":[{"name":"bytes","configuration":{"endian":"little"}}],"dimension_names":dims,"attributes":attrs});
            store.set(&StoreKey::new(format!("{name}/zarr.json")).unwrap(),serde_json::to_vec(&m).unwrap().into()).await.unwrap();
            let bytes:Vec<u8>=match name {"x"=>[5f64,15.].into_iter().flat_map(f64::to_le_bytes).collect(),"y"=>[15f64,5.].into_iter().flat_map(f64::to_le_bytes).collect(),"time"=>0i64.to_le_bytes().to_vec(),_=>[1f64+f64::EPSILON,-0.,f64::from_bits(0x7ff8000000001234),4.].into_iter().flat_map(f64::to_le_bytes).collect()};
            store.set(&StoreKey::new(format!("{name}/c/{}",if name=="temperature"{"0/0/0"}else{"0"})).unwrap(),bytes.into()).await.unwrap();
        }
        let snapshot=store.session().write().await.commit("initial").execute().await.unwrap().to_string();
        (repo,snapshot)
    });
    let server = Server::new(root.clone());
    let request:Request=serde_json::from_value(serde_json::json!({"source":{"kind":"assets","assets":[{"id":"science","format":"icechunk","locator":format!("{}/repo#branch=main",server.url)}]},"spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[0,0,20,20]},"time":null,"variables":[{"kind":"dataset","path":"temperature"}],"quality":[],"grid":{"kind":"native"},"operations":[],"sink":{"kind":"directory","path":root.join("output")},"budget":{"buffer_bytes":4194304,"max_nodes":10000,"chunk_edge":2}})).unwrap();
    (server, runtime, repo, request, snapshot)
}
#[test]
fn branch_is_pinned_and_float64_round_trip_is_exact() {
    let (server, runtime, repo, request, snapshot) = fixture();
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    assert!(prepared.explain().to_string().contains(&snapshot));
    runtime.block_on(async {
        let store = AsyncIcechunkStore::new(repo.writable_session("main").await.unwrap());
        store
            .set(
                &StoreKey::new("temperature/c/0/0/0").unwrap(),
                [9f64; 4]
                    .into_iter()
                    .flat_map(f64::to_le_bytes)
                    .collect::<Vec<_>>()
                    .into(),
            )
            .await
            .unwrap();
        store
            .session()
            .write()
            .await
            .commit("new branch tip")
            .execute()
            .await
            .unwrap();
    });
    let mut stream = prepared.into_stream(Default::default()).unwrap();
    let chunk = stream.next().unwrap().unwrap();
    assert_eq!(
        chunk
            .pixels
            .as_f64()
            .unwrap()
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        vec![
            (1f64 + f64::EPSILON).to_bits(),
            (-0f64).to_bits(),
            0x7ff8000000001234,
            4f64.to_bits()
        ]
    );
    assert_eq!(
        chunk.validity().collect::<Vec<_>>(),
        [true, true, false, true]
    );
    assert!(stream.finish().is_ok());
    let fresh = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    let mut stream = fresh.into_stream(Default::default()).unwrap();
    assert_eq!(
        stream.next().unwrap().unwrap().pixels.as_f64().unwrap(),
        &[9.; 4]
    );
    stream.finish().unwrap();
    let mut pinned = request.clone();
    if let Source::Assets { assets } = &mut pinned.source {
        assets[0].locator = format!("{}/repo#snapshot={snapshot}", server.url);
    }
    let prepared = acquisition::prepare(&pinned, &mut LayoutCache::default()).unwrap();
    prepared.fetch().unwrap();
    let reopened = zarrs::array::Array::open(
        Arc::new(
            zarrs::filesystem::FilesystemStore::new(server.root.join("output/science.zarr"))
                .unwrap(),
        ),
        "/temperature",
    )
    .unwrap();
    let values = reopened
        .retrieve_array_subset::<Vec<f64>>(&reopened.subset_all())
        .unwrap();
    assert_eq!(values[0].to_bits(), (1f64 + f64::EPSILON).to_bits());
    assert_eq!(values[2].to_bits(), 0x7ff8000000001234);
}
#[test]
fn unsupported_selectors_and_small_budgets_fail() {
    let (_server, _runtime, _repo, mut request, _snapshot) = fixture();
    if let Source::Assets { assets } = &mut request.source {
        assets[0].locator.push_str("&snapshot=bad");
    }
    assert!(acquisition::prepare(&request, &mut LayoutCache::default()).is_err());
    if let Source::Assets { assets } = &mut request.source {
        assets[0].locator = assets[0].locator.split('&').next().unwrap().to_owned();
    }
    request.budget.buffer_bytes = 32;
    assert!(acquisition::prepare(&request, &mut LayoutCache::default()).is_err());
}

#[test]
fn unsupported_chunk_key_encoding_fails_before_data_or_output() {
    let (server, runtime, repo, request, _snapshot) = fixture();
    for encoding in [
        serde_json::json!({"name":"default","configuration":{"separator":"."}}),
        serde_json::json!({"name":"v2","configuration":{"separator":"."}}),
    ] {
        runtime.block_on(async {
            let store = AsyncIcechunkStore::new(repo.writable_session("main").await.unwrap());
            let key = StoreKey::new("temperature/zarr.json").unwrap();
            let mut metadata: serde_json::Value =
                serde_json::from_slice(&store.get(&key).await.unwrap().unwrap()).unwrap();
            metadata["chunk_key_encoding"] = encoding;
            store
                .set(&key, serde_json::to_vec(&metadata).unwrap().into())
                .await
                .unwrap();
            store
                .session()
                .write()
                .await
                .commit("unsupported encoding")
                .execute()
                .await
                .unwrap();
        });
        let error = match acquisition::prepare(&request, &mut LayoutCache::default()) {
            Ok(_) => panic!("unsupported key encoding silently admitted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("chunk key encoding"), "{error}");
        assert!(!server.root.join("output").exists());
    }
}

#[test]
fn changed_payload_invalidates_completion_and_latches_failure() {
    let (server, _runtime, _repo, request, _snapshot) = fixture();
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    let payload = fs::read_dir(server.root.join("repo/chunks"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| fs::metadata(path).unwrap().len() == 32)
        .unwrap();
    let mut count = 0;
    let result = prepared.execute(|_| {
        count += 1;
        fs::write(&payload, [0u8; 32])?;
        Ok(())
    });
    assert_eq!(count, 1);
    assert!(result.is_err());
    assert!(prepared.execute(|_| Ok(())).is_err());
    assert!(prepared.fetch().is_err());
    assert!(!server.root.join("output").exists());
}

#[test]
fn cancellation_never_returns_a_completion_receipt() {
    let (_server, _runtime, _repo, request, _snapshot) = fixture();
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    let stream = prepared.into_stream(Default::default()).unwrap();
    stream.cancel();
    assert!(stream.finish().is_err());
}

#[test]
fn virtual_external_range_is_version_pinned_and_reference_checksums_are_enforced() {
    let (server, runtime, repo, request, _snapshot) = fixture();
    let payload = server.root.join("external.bin");
    let values = [1f64 + f64::EPSILON, -0., 3., 4.]
        .into_iter()
        .flat_map(f64::to_le_bytes)
        .collect::<Vec<_>>();
    let mut object = b"pad".to_vec();
    object.extend_from_slice(&values);
    object.extend_from_slice(b"trailer");
    fs::write(&payload, &object).unwrap();
    let update = |checksum| {
        runtime.block_on(async {
            let mut session = repo.writable_session("main").await.unwrap();
            session
                .set_chunk_ref(
                    "/temperature".try_into().unwrap(),
                    icechunk::format::ChunkIndices(vec![0, 0, 0]),
                    Some(icechunk::format::manifest::ChunkPayload::Virtual(
                        icechunk::format::manifest::VirtualChunkRef {
                            location: icechunk::format::manifest::VirtualChunkLocation::from_url(
                                &format!("{}/external.bin", server.url),
                            )
                            .unwrap(),
                            offset: 3,
                            length: 32,
                            checksum,
                        },
                    )),
                )
                .await
                .unwrap();
            session.commit("virtual payload").execute().await.unwrap();
        })
    };
    update(None);
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    let mut got = Vec::new();
    prepared
        .execute(|chunk| {
            got.extend_from_slice(chunk.pixels.as_f64().unwrap());
            Ok(())
        })
        .unwrap();
    assert_eq!(got[0].to_bits(), (1f64 + f64::EPSILON).to_bits());
    let provenance = prepared.explain().to_string();
    assert!(provenance.contains("reference_checksum_present"));
    assert!(provenance.contains("external.bin"));
    object[4] ^= 1;
    fs::write(&payload, &object).unwrap();
    assert!(prepared.execute(|_| Ok(())).is_err());
    update(Some(icechunk::format::manifest::Checksum::ETag(
        icechunk_types::ETag("wrong".into()),
    )));
    assert!(acquisition::prepare(&request, &mut LayoutCache::default()).is_err());
    update(Some(icechunk::format::manifest::Checksum::LastModified(
        icechunk::format::manifest::SecondsSinceEpoch(1704067199),
    )));
    assert!(acquisition::prepare(&request, &mut LayoutCache::default()).is_err());
    update(Some(icechunk::format::manifest::Checksum::LastModified(
        icechunk::format::manifest::SecondsSinceEpoch(1704067200),
    )));
    assert!(acquisition::prepare(&request, &mut LayoutCache::default()).is_ok());
}
