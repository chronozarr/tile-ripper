use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tile_ripper::swot_discovery::*;

fn query() -> Query {
    Query {
        collection: COLLECTION.into(),
        collection_concept_id: None,
        bbox: [-73., 41., -70., 44.],
        start: "2024-02-16T00:00:00Z".into(),
        end: "2024-02-18T00:00:00Z".into(),
        limit: 20,
        limits: Limits::default(),
    }
}
fn collection() -> Value {
    json!({"hits":1,"items":[{"meta":{"concept-id":"C3233944993-POCLOUD","revision-id":24,"provider-id":"POCLOUD"},"umm":{"ShortName":COLLECTION,"Version":"D"}}]})
}
fn records() -> Vec<Value> {
    serde_json::from_slice::<Value>(include_bytes!("data/cmr/original.json")).unwrap()["items"]
        .as_array()
        .unwrap()
        .clone()
}
struct Response {
    status: u16,
    body: Vec<u8>,
    headers: Vec<(&'static str, String)>,
    delay: u64,
}
fn page(items: Vec<Value>, hits: usize, after: Option<&str>) -> Response {
    let mut headers = vec![];
    if let Some(a) = after {
        headers.push(("CMR-Search-After", a.into()));
    }
    Response {
        status: 200,
        body: serde_json::to_vec(&json!({"hits":hits,"items":items})).unwrap(),
        headers,
        delay: 0,
    }
}
fn first() -> Response {
    Response {
        status: 200,
        body: serde_json::to_vec(&collection()).unwrap(),
        headers: vec![],
        delay: 0,
    }
}
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/search/", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let log = requests.clone();
        let worker = thread::spawn(move || {
            for response in responses {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if std::time::Instant::now() >= deadline {
                                return;
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = vec![];
                while !request.ends_with(b"\r\n\r\n") {
                    let mut b = [0];
                    if stream.read(&mut b).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(b[0]);
                    assert!(request.len() < 16000);
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                thread::sleep(Duration::from_millis(response.delay));
                let mut header = format!(
                    "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    response.body.len()
                );
                for (k, v) in response.headers {
                    header.push_str(&format!("{k}: {v}\r\n"));
                }
                header.push_str("\r\n");
                let _ = stream
                    .write_all(header.as_bytes())
                    .and_then(|_| stream.write_all(&response.body));
            }
        });
        Self {
            url,
            requests,
            worker: Some(worker),
        }
    }
    fn discover(&self, q: &Query) -> Report {
        CmrClient::loopback_fixture(&self.url)
            .unwrap()
            .discover(q)
            .unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn frozen_authoritative_metadata_and_anonymous_query() {
    let s = Server::new(vec![first(), page(records(), 6, None)]);
    let r = s.discover(&query());
    assert_eq!(r.status, Completion::Complete);
    assert_eq!(r.candidates.len(), 6);
    let c = r
        .candidates
        .iter()
        .find(|c| c.concept_id == "G3927359490-POCLOUD")
        .unwrap();
    assert!(c.exclusions.is_empty(), "{:?}", c.exclusions);
    assert_eq!(c.source_size_bytes, Some(89242093));
    assert_eq!(c.processing_family, FamilyStatus::AppearsRasterDPgd0);
    assert_eq!(c.scientific_profile, InspectionStatus::NotInspected);
    assert_eq!(c.native_selection, NativeSelectionStatus::Unresolved);
    assert_eq!(
        c.coverage.as_ref().unwrap().polygons[0][0],
        [-72.42544746840993, 42.22719163055778]
    );
    let log = s.requests.lock().unwrap();
    assert_eq!(log.len(), 2);
    let u = reqwest::Url::parse(&format!(
        "http://test{}",
        log[1].split_whitespace().nth(1).unwrap()
    ))
    .unwrap();
    let params: Vec<_> = u.query_pairs().collect();
    assert!(params
        .iter()
        .any(|(k, v)| k == "bounding_box" && v == "-73,41,-70,44"));
    assert!(params
        .iter()
        .any(|(k, v)| k == "collection_concept_id" && v == "C3233944993-POCLOUD"));
    assert_eq!(
        params
            .iter()
            .filter(|(k, _)| k == "sort_key[]")
            .map(|(_, v)| v.as_ref())
            .collect::<Vec<_>>(),
        ["start_date", "producer_granule_id"]
    );
    assert!(!log.join("").to_lowercase().contains("authorization:"));
    assert!(!log.join("").to_lowercase().contains("cookie:"));
}
#[test]
fn pagination_deduplication_and_order_are_deterministic() {
    let items = records();
    let mut q = query();
    q.limits.page_size = 2;
    let s = Server::new(vec![
        first(),
        page(vec![items[1].clone(), items[0].clone()], 4, Some("[1,2]")),
        page(vec![items[1].clone(), items[2].clone()], 4, Some("[3,4]")),
        page(vec![], 4, None),
    ]);
    let a = s.discover(&q);
    assert_eq!(a.status, Completion::Complete);
    assert_eq!(a.statistics.duplicates, 1);
    assert_eq!(a.candidates.len(), 3);
    assert!(s.requests.lock().unwrap()[2]
        .to_lowercase()
        .contains("cmr-search-after: [1,2]"));
    let t = Server::new(vec![
        first(),
        page(
            vec![items[2].clone(), items[0].clone(), items[1].clone()],
            3,
            None,
        ),
    ]);
    let b = t.discover(&query());
    assert_eq!(a.candidates, b.candidates);
}
#[test]
fn explicit_limits_never_mislabel_first_four_as_complete() {
    for variant in 0..3 {
        let mut q = query();
        q.limits.page_size = 2;
        if variant == 0 {
            q.limit = 2;
        } else if variant == 1 {
            q.limits.max_pages = 1;
        } else {
            q.limits.max_records = 2;
        }
        let s = Server::new(vec![
            first(),
            page(records()[..2].to_vec(), 100, Some("[1]")),
        ]);
        let r = s.discover(&q);
        assert_eq!(r.status, Completion::Truncated);
        assert_eq!(r.candidates.len(), 2);
        assert!(!r.reasons.is_empty());
    }
}
#[test]
fn empty_result_is_complete() {
    let s = Server::new(vec![first(), page(vec![], 0, None)]);
    let r = s.discover(&query());
    assert_eq!(r.status, Completion::Complete);
    assert!(r.candidates.is_empty());
}

#[test]
fn multiple_dates_with_overlapping_footprints_keep_distinct_source_identities() {
    let a = records()[2].clone();
    let mut b: Value = serde_json::from_str(
        &serde_json::to_string(&a)
            .unwrap()
            .replace("20240216", "20240217")
            .replace("2024-02-16", "2024-02-17"),
    )
    .unwrap();
    b["meta"]["concept-id"] = json!("G1234-POCLOUD");
    let s = Server::new(vec![first(), page(vec![b, a], 2, None)]);
    let r = s.discover(&query());
    assert_eq!(r.status, Completion::Complete);
    assert_ne!(
        r.candidates[0].acquisition_start,
        r.candidates[1].acquisition_start
    );
    assert_ne!(r.candidates[0].source_url, r.candidates[1].source_url);
    assert_eq!(r.candidates[0].coverage, r.candidates[1].coverage);
    assert!(r.candidates.iter().all(|c| c.exclusions.is_empty()));
}

#[test]
fn timestamp_sort_uses_instants_including_fractional_seconds_and_offsets() {
    let mut a = records()[0].clone();
    let mut b = records()[1].clone();
    a["umm"]["TemporalExtent"]["RangeDateTime"]["BeginningDateTime"] =
        json!("2024-02-16T02:51:38Z");
    b["umm"]["TemporalExtent"]["RangeDateTime"]["BeginningDateTime"] =
        json!("2024-02-16T03:51:38.1+01:00");
    let first_id = a["meta"]["concept-id"].as_str().unwrap().to_owned();
    let s = Server::new(vec![first(), page(vec![b, a], 2, None)]);
    let r = s.discover(&query());
    assert_eq!(r.candidates[0].concept_id, first_id);
    assert!(r.candidates.iter().all(|c| c.exclusions.is_empty()));
}

#[test]
fn cli_truncation_is_nonzero_and_preserves_explicit_json_without_overwrite() {
    let s = Server::new(vec![
        first(),
        page(records()[..2].to_vec(), 100, Some("[1]")),
    ]);
    let path = std::env::temp_dir().join(format!(
        "swot-discovery-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .args([
            "discover",
            "--collection",
            COLLECTION,
            "--bbox",
            "-73,41,-70,44",
            "--start",
            "2024-02-16T00:00:00Z",
            "--end",
            "2024-02-18T00:00:00Z",
            "--limit",
            "2",
            "--page-size",
            "2",
            "--cmr-fixture",
            &s.url,
            "--output",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let report = Report::from_json(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(report.status, Completion::Truncated);
    let existing = std::fs::read(&path).unwrap();
    let t = Server::new(vec![first(), page(vec![], 0, None)]);
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .args([
            "discover",
            "--collection",
            COLLECTION,
            "--bbox",
            "-73,41,-70,44",
            "--start",
            "2024-02-16T00:00:00Z",
            "--end",
            "2024-02-18T00:00:00Z",
            "--limit",
            "2",
            "--cmr-fixture",
            &t.url,
            "--output",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), existing);
    std::fs::remove_file(path).unwrap();
    let rejected = std::process::Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .args(["discover", "--auth", "earthdata", "--netrc", "DO_NOT_READ"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
}
#[test]
fn query_refusals_and_fixture_origin_are_local() {
    for variant in 0..10 {
        let mut q = query();
        match variant {
            0 => q.bbox = [-180., -90., 180., 90.],
            1 => q.bbox = [170., 0., -170., 1.],
            2 => q.start = "".into(),
            3 => q.end = "2025-01-01T00:00:00Z".into(),
            4 => q.end = q.start.clone(),
            5 => q.collection = "other".into(),
            6 => q.limit = 101,
            7 => q.limits.max_pages = 11,
            8 => q.limits.retries = 3,
            _ => q.bbox[0] = f64::NAN,
        };
        assert!(q.validate().is_err());
    }
    for u in [
        "http://localhost:12/",
        "https://127.0.0.1/",
        "http://example.com/",
        "http://127.0.0.1/?token=secret",
        "http://user:password@127.0.0.1/",
    ] {
        assert!(CmrClient::loopback_fixture(u).is_err());
    }
}
#[test]
fn missing_ambiguous_conflicting_and_unsupported_metadata_are_explicit() {
    let original = records()[2].clone();
    for variant in 0..12 {
        let mut item = original.clone();
        match variant {
            0 => item["umm"]["RelatedUrls"] = json!([]),
            1 => {
                let links = item["umm"]["RelatedUrls"].as_array_mut().unwrap();
                links.push(json!({"Type":"GET DATA","URL":"https://evil.example/signed?credential=DO_NOT_EMIT"}));
            }
            2 => {
                let text = serde_json::to_string(&item)
                    .unwrap()
                    .replace("PGD0", "PID0");
                item = serde_json::from_str(&text).unwrap();
            }
            3 => item["umm"]["DataGranule"]["Identifiers"]
                .as_array_mut()
                .unwrap()
                .push(json!({"IdentifierType":"CRID","Identifier":"PID0"})),
            4 => item["umm"]["TemporalExtent"] = json!({}),
            5 => item["umm"]["SpatialExtent"] = json!({}),
            6 => item["meta"]["collection-concept-id"] = json!("C9-POCLOUD"),
            7 => item["umm"]["GranuleUR"] = json!("https://bad.example/?secret=DO_NOT_EMIT"),
            8 => {
                item["umm"]["TemporalExtent"]["RangeDateTime"]["EndingDateTime"] =
                    json!("2020-01-01T00:00:00Z")
            }
            9 => item["meta"]["provider-id"] = json!("OTHER"),
            10 => item["meta"]["concept-type"] = json!("collection"),
            _ => item["meta"]["deleted"] = json!(true),
        }
        let s = Server::new(vec![first(), page(vec![item], 1, None)]);
        let r = s.discover(&query());
        assert_eq!(r.status, Completion::Complete);
        assert!(!r.candidates[0].exclusions.is_empty(), "case {variant}");
        assert!(!serde_json::to_string(&r).unwrap().contains("DO_NOT_EMIT"));
    }
}
#[test]
fn duplicate_identity_conflict_is_excluded() {
    let a = records()[0].clone();
    let mut b = a.clone();
    b["umm"]["PGEVersionClass"]["PGEVersion"] = json!("999");
    let s = Server::new(vec![first(), page(vec![a.clone(), b.clone()], 2, None)]);
    let t = Server::new(vec![first(), page(vec![b, a], 2, None)]);
    let x = s.discover(&query());
    let y = t.discover(&query());
    assert_eq!(x.candidates, y.candidates);
    assert!(x.candidates[0]
        .exclusions
        .iter()
        .any(|e| e.contains("conflicting records")));
}
#[test]
fn collection_resolution_refuses_unverified_or_ambiguous_ids() {
    for v in 0..3 {
        let mut q = query();
        let mut response = first();
        if v == 0 {
            q.collection_concept_id = Some("C1-POCLOUD".into());
        } else {
            let mut c = collection();
            if v == 1 {
                c["hits"] = json!(2);
            } else {
                c["items"][0]["umm"]["Version"] = json!("C");
            }
            response.body = serde_json::to_vec(&c).unwrap();
        }
        let s = Server::new(vec![response]);
        let r = s.discover(&q);
        assert_eq!(r.status, Completion::Failed);
        assert_eq!(r.statistics.http_requests, 1);
    }
}
#[test]
fn byte_parse_http_redirect_and_deadline_failures_are_bounded() {
    for v in 0..7 {
        let mut q = query();
        q.limits.retries = 0;
        let mut response = page(vec![], 0, None);
        match v {
            0 => {
                q.limits.max_response_bytes = 1024;
                response.body = vec![b' '; 1025];
            }
            1 => response.body = b"{bad".to_vec(),
            2 => response.headers.push(("CMR-Timed-Out", "true".into())),
            3 => {
                response.status = 302;
                response
                    .headers
                    .push(("Location", "https://example.com/DO_NOT_VISIT".into()));
            }
            4 => {
                q.limits.timeout_seconds = 1;
                q.limits.request_timeout_seconds = 1;
                response.delay = 1200;
            }
            5 => {
                response.status = 429;
            }
            _ => {
                response.body =
                    serde_json::to_vec(&json!({"hits":0,"items":[],"unbounded":"x".repeat(17000)}))
                        .unwrap()
            }
        }
        let s = Server::new(vec![first(), response]);
        let r = s.discover(&q);
        assert_eq!(r.status, Completion::Failed, "case {v}");
        assert_eq!(r.statistics.http_requests, 2);
        assert!(!serde_json::to_string(&r).unwrap().contains("DO_NOT_VISIT"));
        if v == 0 {
            assert!(
                r.statistics.response_body_bytes
                    <= 1025 + serde_json::to_vec(&collection()).unwrap().len()
            );
        }
        if v == 4 {
            assert!(r.statistics.elapsed_seconds < 2.);
        }
    }
}
#[test]
fn transient_retry_and_missing_repeated_cursor() {
    let mut retry = first();
    retry.status = 503;
    let s = Server::new(vec![retry, first(), page(vec![], 0, None)]);
    let r = s.discover(&query());
    assert_eq!(r.status, Completion::Complete);
    assert_eq!(r.statistics.http_requests, 3);
    for repeated in [false, true] {
        let mut q = query();
        q.limits.page_size = 1;
        let items = records();
        let mut responses = vec![
            first(),
            page(
                vec![items[0].clone()],
                5,
                if repeated { Some("[1]") } else { None },
            ),
        ];
        if repeated {
            responses.push(page(vec![items[1].clone()], 5, Some("[1]")));
        }
        let s = Server::new(responses);
        let r = s.discover(&q);
        assert_eq!(r.status, Completion::Failed);
        assert!(!r.candidates.is_empty());
    }
}
#[cfg(feature = "native-hdf5")]
#[test]
fn explicit_bridge_preserves_identity_policy_window_and_admission() {
    use tile_ripper::{swot, swot_discovery::bridge::*, Budget, Spatial};
    let s = Server::new(vec![first(), page(records(), 6, None)]);
    let r = s.discover(&query());
    let mut selection = JobSelection {
        observations: vec![Selection {
            concept_id: "G3927359490-POCLOUD".into(),
            observation_id: "original".into(),
            window: NativeWindow {
                start: [600, 400],
                count: [128, 64],
                step: [1, 2],
            },
            quality_policy: swot::Policy::cookbook(),
            expected_etag: Some("\"expected\"".into()),
        }],
        source_budget: Budget {
            buffer_bytes: 64 * 1024 * 1024,
            max_nodes: 4096,
            chunk_edge: 32,
        },
        limits: tile_ripper::swot_job::Limits {
            memory_bytes: 128 * 1024 * 1024,
            output_chunk_edge: 128,
            max_output_objects: 10000,
        },
    };
    let d = draft(std::slice::from_ref(&r), &selection).unwrap();
    let e = &d.uninspected_manifest().observations[0];
    assert_eq!(e.asset().unwrap().id, selection.observations[0].concept_id);
    assert_eq!(e.asset().unwrap().etag.as_deref(), Some("\"expected\""));
    assert_eq!(
        e.asset().unwrap().datetime,
        e.catalog.as_ref().map(|c| c.acquisition_start.clone())
    );
    assert!(matches!(
        e.request.spatial,
        Spatial::ArraySlice {
            start: [600, 400],
            count: [128, 64],
            step: [1, 2],
            ..
        }
    ));
    selection.observations = vec![selection.observations[0].clone(); 5];
    assert!(draft(std::slice::from_ref(&r), &selection).is_err());
    selection.observations.truncate(1);
    selection.limits.memory_bytes = 1024;
    assert!(draft(std::slice::from_ref(&r), &selection).is_err());
    selection.limits.memory_bytes = 128 * 1024 * 1024;
    selection.observations[0].window.count[0] = 0;
    assert!(draft(&[r], &selection).is_err());
}

#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
#[test]
fn oversized_selection_is_refused_before_credentials_or_source_inspection() {
    let path = std::env::temp_dir().join(format!("swot-selection-{}.json", std::process::id()));
    std::fs::write(&path, vec![b' '; 65537]).unwrap();
    let r = std::process::Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .args([
            "discover-job",
            path.to_str().unwrap(),
            "--auth",
            "earthdata",
            "--netrc",
            "DO_NOT_READ",
            "--output",
            "DO_NOT_CREATE.json",
        ])
        .output()
        .unwrap();
    assert!(!r.status.success());
    assert!(String::from_utf8(r.stderr)
        .unwrap()
        .contains("input bound (65536 bytes)"));
    std::fs::remove_file(path).unwrap();
}
