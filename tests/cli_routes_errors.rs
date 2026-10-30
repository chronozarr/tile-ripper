//! Exercise the public process boundary, without credentials or provider traffic.
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::Read,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

const SIGNATURE: &str = "cli_test_signature_must_not_escape_7f1a";
const PRIVATE_URL: &str =
    "https://private-user:private-password@example.invalid/asset.tif?sig=cli_test_signature_must_not_escape_7f1a#private-fragment";

fn run_with(args: &[&str], configure: impl FnOnce(&mut Command)) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tile-ripper"));
    command
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost");
    for key in [
        "TILE_RIPPER_COG_BACKEND",
        "TILE_RIPPER_AUTH_NETRC",
        "TILE_RIPPER_HTTP_TRACE",
        "TILE_RIPPER_WORKERS",
        "TILE_RIPPER_PER_HOST",
        "NETRC",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(key);
    }
    configure(&mut command);
    let mut child = command.spawn().expect("spawn actual tile-ripper CLI");
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let err = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out.join();
            let _ = err.join();
            panic!("CLI exceeded the bounded local-fixture deadline");
        }
        thread::sleep(Duration::from_millis(10));
    };
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

fn run(args: &[&str]) -> Output {
    run_with(args, |_| {})
}

fn no_secrets(output: &Output) {
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        for secret in [
            SIGNATURE,
            "private-password",
            "private-user",
            "private-fragment",
        ] {
            assert!(
                !text.contains(secret),
                "CLI exposed a private locator component"
            );
        }
    }
}

fn json_error(output: &Output, code: &str, stage: &str) -> Value {
    no_secrets(output);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "failure polluted successful stdout"
    );
    // Parsing the entire stream rejects both plaintext prefixes and extra JSON objects.
    let document: Value = serde_json::from_slice(&output.stderr)
        .expect("stderr must contain exactly one JSON error document");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["document_type"], "error");
    let error = document["error"].as_object().expect("typed error object");
    for key in ["code", "stage", "message", "retryable", "http_status"] {
        assert!(error.contains_key(key), "error contract omitted {key}");
    }
    assert_eq!(document["error"]["code"], code);
    assert_eq!(document["error"]["stage"], stage);
    assert!(!document["error"]["message"]
        .as_str()
        .expect("error message string")
        .trim()
        .is_empty());
    document
}

#[test]
fn products_add_routes_without_replacing_existing_profiles() {
    let output = run(&["products"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let products: Value = serde_json::from_slice(&output.stdout).unwrap();
    let explicit_json = run(&["products", "--json"]);
    assert!(explicit_json.status.success());
    assert!(explicit_json.stderr.is_empty());
    let explicit_products: Value = serde_json::from_slice(&explicit_json.stdout).unwrap();
    assert_eq!(
        products, explicit_products,
        "products --json changed the successful document"
    );
    assert_eq!(products["schema"], "tile-ripper-products-v1");
    for key in ["profiles", "array_profiles", "capability", "build"] {
        assert!(
            products.get(key).is_some(),
            "legacy field {key} disappeared"
        );
    }
    let expected: BTreeSet<_> = [
        "landsat-c2-l2-sr",
        "landsat-tm-c2-l2-sr",
        "landsat-etm-c2-l2-sr",
        "hls-l30",
        "hls-s30",
        "opera-dswx-hls",
        "opera-rtc-s1",
        "opera-dswx-s1",
        "opera-dist-alert",
        "opera-dswx-ni-prototype",
        "opera-dist-alert-s1",
        "opera-dist-ann-hls",
    ]
    .into_iter()
    .collect();
    let profiles = products["profiles"].as_array().unwrap();
    let actual: BTreeSet<_> = profiles.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(actual, expected);
    assert_eq!(profiles.len(), expected.len(), "duplicate profile IDs");
    for profile in profiles {
        for key in [
            "family",
            "native_grid",
            "catalog",
            "bands",
            "quality_policies",
        ] {
            assert!(profile.get(key).is_some(), "existing profile lost {key}");
        }
    }
    let routes = &products["routes"];
    assert_eq!(routes["schema"], "tile-ripper-routes-v1");
    for (feature, compiled) in [
        ("native_cog", cfg!(feature = "native-cog")),
        ("gdal", cfg!(feature = "gdal")),
        ("native_hdf5", cfg!(feature = "native-hdf5")),
        ("authenticated_http", cfg!(feature = "authenticated-http")),
        ("icechunk", cfg!(feature = "icechunk")),
    ] {
        assert_eq!(routes["build_features"][feature], compiled, "{feature}");
    }
    let generic = routes["generic"].as_array().unwrap();
    let ids: BTreeSet<_> = generic.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        BTreeSet::from(["earth-search-sentinel-2-c1-l2a", "explicit-cog"])
    );
    let s2 = generic
        .iter()
        .find(|r| r["id"] == "earth-search-sentinel-2-c1-l2a")
        .unwrap();
    assert_eq!(s2["collection"], "sentinel-2-c1-l2a");
    let explicit = generic.iter().find(|r| r["id"] == "explicit-cog").unwrap();
    assert_eq!(explicit["request_source_kind"], "assets");
    assert_eq!(explicit["asset_format"], "cog");
    assert_eq!(explicit["request_file_required"], true);
    assert!(explicit.get("cli_format_flag").is_some_and(Value::is_null));
    assert!(explicit.get("request_format").is_none());
    assert_eq!(explicit["example_request"], "examples/cog-request.json");
    assert_eq!(explicit["discovery"], false);
    assert_eq!(explicit["automatic_planetary_computer_signing"], false);
    assert_eq!(explicit["explicit_authentication"]["cli_supported"], false);
    assert_eq!(
        explicit["chronozarr_bridge"],
        "conditional_on_required_source_metadata"
    );
    for route in generic {
        assert_eq!(
            route["available_in_build"],
            cfg!(feature = "native-cog") || cfg!(feature = "gdal")
        );
        assert_eq!(
            route["native_profile_available_in_build"],
            cfg!(feature = "native-cog")
        );
        assert_eq!(
            route["required_features_any"],
            serde_json::json!([["native-cog"], ["gdal"]])
        );
    }
    assert!(!generic.iter().any(|r| r["collection"] == "sentinel-1-rtc"));
    let named: BTreeSet<_> = routes["named_products"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["product"].as_str().unwrap())
        .collect();
    assert!(expected.is_subset(&named));
    let rtc = routes["named_products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|route| route["product"] == "opera-rtc-s1")
        .unwrap();
    assert_eq!(rtc["profile_catalog"]["collection"], "OPERA_L2_RTC-S1_V1");
    assert!(rtc["provider_routes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|provider| provider["provider"] == "earthdata"));
    assert!(!rtc["provider_routes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|provider| provider["provider"] == "planetary_computer"));
    for route in routes["named_products"].as_array().unwrap() {
        assert_eq!(route["available_in_build"], cfg!(feature = "native-cog"));
        let profile = profiles
            .iter()
            .find(|p| p["id"] == route["product"])
            .unwrap();
        assert_eq!(route["profile_catalog"], profile["catalog"]);
        for provider in route["provider_routes"].as_array().unwrap() {
            assert_eq!(provider["credential_status"], "not_checked");
            assert_eq!(
                provider["available_in_build"],
                cfg!(feature = "native-cog")
                    && (provider["provider"] != "earthdata"
                        || cfg!(feature = "authenticated-http"))
            );
        }
    }
    assert_eq!(
        routes["array_products"]["available_in_build"],
        cfg!(feature = "native-hdf5")
    );
    let bridge = &routes["chronozarr_bridge"];
    assert_eq!(bridge["status"], "checkout_only");
    assert_eq!(bridge["entrypoint"], "chronozarr.convert_tile_ripper");
    assert_eq!(bridge["input_schema"], "acquisition_result");
    assert_eq!(bridge["input_format"], "zarr_v2");
    assert_eq!(bridge["input_encoding"], "uncompressed_c_order_unfiltered");
    assert_eq!(bridge["installed_by_tile_ripper"], false);
    assert_eq!(bridge["published_in_chronozarr_0_5_1"], false);
}

#[test]
fn parse_errors_are_one_json_document_by_default_and_with_explicit_json() {
    for args in [
        vec!["plan", "--not-a-real-option", PRIVATE_URL],
        vec![
            "plan",
            "--human",
            "--not-a-real-option",
            PRIVATE_URL,
            "--json",
        ],
    ] {
        let output = run(&args);
        let document = json_error(&output, "INVALID_ARGUMENTS", "cli");
        assert!(document["error"]["http_status"].is_null());
    }
}

#[test]
fn final_human_flag_preserves_safe_plaintext_parse_error() {
    let output = run(&[
        "plan",
        "--json",
        "--not-a-real-option",
        PRIVATE_URL,
        "--human",
    ]);
    no_secrets(&output);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(serde_json::from_slice::<Value>(&output.stderr).is_err());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("unknown option"), "{error}");
}

#[cfg(feature = "native-cog")]
mod native {
    use super::*;
    use std::{
        fs,
        io::Write,
        net::TcpListener,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "tile-ripper-cli-contract-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn request(&self, value: Value) -> PathBuf {
            let path = self.0.join("request.json");
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct StatusServer {
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
        stopped: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl StatusServer {
        fn new(status: u16) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stopped = Arc::new(AtomicBool::new(false));
            let (recorded, stop) = (requests.clone(), stopped.clone());
            let worker = thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => panic!("loopback fixture failed: {error}"),
                    };
                    // Do not inherit a nonblocking accepted socket on platforms
                    // where the listener's flag can propagate to new streams.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while !request.ends_with(b"\r\n\r\n") && request.len() < 16384 {
                        match stream.read(&mut byte) {
                            Ok(0) => break,
                            Ok(_) => request.push(byte[0]),
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                continue
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                break
                            }
                            Err(error) => panic!("loopback request-header read failed: {error}"),
                        }
                    }
                    // An incomplete read is not a request and must never get a
                    // synthetic status which the request counter did not see.
                    if !request.ends_with(b"\r\n\r\n") {
                        continue;
                    }
                    if let Some(first) = String::from_utf8_lossy(&request).lines().next() {
                        recorded.lock().unwrap().push(first.to_owned());
                    }
                    let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            Self {
                url,
                requests,
                stopped,
                worker: Some(worker),
            }
        }
        fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }
    impl Drop for StatusServer {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            self.worker.take().unwrap().join().unwrap();
        }
    }

    fn cog_request(locator: &str, output: Option<&Path>) -> Value {
        serde_json::json!({
            "source":{"kind":"assets","assets":[{"id":"cli-fixture","locator":locator,"format":"cog","datetime":"2025-07-13T15:17:29Z"}]},
            "spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[500000,3999040,500960,4000000]},
            "variables":[{"kind":"band","index":1}],"grid":{"kind":"native"},
            "sink":output.map_or_else(||serde_json::json!({"kind":"callback"}),|path|serde_json::json!({"kind":"directory","path":path})),
            "budget":{"buffer_bytes":16777216,"max_nodes":4096,"chunk_edge":64}
        })
    }

    #[test]
    #[cfg(feature = "authenticated-http")]
    fn incompatible_product_checkpoint_is_an_actionable_argument_error_before_io() {
        let fixture = Fixture::new();
        let checkpoint = fixture.0.join("must-not-create-checkpoint");
        let netrc = fixture.0.join("must-not-open-private-netrc");
        let path = fixture.request(serde_json::json!({
            "product":"hls-l30",
            "aoi":{"kind":"bounds","crs":"EPSG:4326","bbox":[-79,35,-78,36]},
            "start":"2024-06-01T00:00:00Z","end":"2024-06-30T23:59:59Z",
            "bands":["B04"],"observations":[]
        }));
        let output = run(&[
            "plan",
            path.to_str().unwrap(),
            "--provider",
            "planetary_computer",
            "--checkpoint",
            checkpoint.to_str().unwrap(),
            "--auth",
            "earthdata",
            "--netrc",
            netrc.to_str().unwrap(),
            "--json",
        ]);
        let document = json_error(&output, "INVALID_ARGUMENTS", "cli");
        assert_eq!(document["error"]["message"],
            "product workflow requires native COG/scientific Zarr; quality policies belong in product JSON; archive authentication is product-specific (see qualification documentation)");
        assert_eq!(document["error"]["retryable"], false);
        assert!(document["error"]["http_status"].is_null());
        assert!(
            !checkpoint.exists(),
            "invalid product flags created checkpoint state"
        );
        assert!(!netrc.exists());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("must-not-open-private-netrc"));
    }

    #[test]
    fn unsupported_pc_sentinel1_is_typed_before_network_for_both_plan_modes() {
        let fixture = Fixture::new();
        let proxy = StatusServer::new(403);
        let path = fixture.request(serde_json::json!({
            "source":{"kind":"stac","endpoint":format!("https://planetarycomputer.microsoft.com/api/stac/v1?sig={SIGNATURE}"),"collection":"sentinel-1-rtc","limits":{"max_items":1,"page_size":1,"max_assets":2,"max_output_pixels":100000,"max_retries":0,"timeout_seconds":1}},
            "spatial":{"kind":"bounds","crs":"EPSG:4326","bbox":[-72.32,8.65,-72.30,8.67]},
            "time":{"kind":"interval","start":"2026-10-27T00:00:00Z","end":"2026-10-27T23:59:59Z"},
            "variables":[{"kind":"named","name":"VV"}],"grid":{"kind":"native"},"sink":{"kind":"callback"},
            "budget":{"buffer_bytes":16777216,"max_nodes":4096,"chunk_edge":64}
        }));
        for discover_only in [false, true] {
            let mut args = vec!["plan", path.to_str().unwrap(), "--backend", "native"];
            if discover_only {
                args.push("--discover-only");
            }
            let output = run_with(&args, |command| {
                // A regression cannot turn this validation test into provider traffic.
                for key in [
                    "HTTP_PROXY",
                    "HTTPS_PROXY",
                    "ALL_PROXY",
                    "http_proxy",
                    "https_proxy",
                    "all_proxy",
                ] {
                    command.env(key, &proxy.url);
                }
                command.env("NO_PROXY", "").env("no_proxy", "");
            });
            let document = json_error(&output, "UNSUPPORTED_STAC_COLLECTION", "discovery");
            assert_eq!(document["error"]["retryable"], false);
            assert!(document["error"]["http_status"].is_null());
            assert_eq!(
                document["error"]["details"]["supported_collections"],
                serde_json::json!(["sentinel-2-c1-l2a"])
            );
        }
        assert_eq!(
            proxy.count(),
            0,
            "unsupported route performed transport work"
        );
    }

    #[test]
    fn identity_403_preserves_status_and_stage_without_signature_leak() {
        let fixture = Fixture::new();
        let server = StatusServer::new(403);
        let path = fixture.request(cog_request(
            &format!("{}/scene.tif?sig={SIGNATURE}#private-fragment", server.url),
            None,
        ));
        let output = run(&["plan", path.to_str().unwrap(), "--backend", "native"]);
        let document = json_error(&output, "SOURCE_IDENTITY_HTTP_ERROR", "source_identity");
        assert_eq!(document["error"]["http_status"], 403);
        assert_eq!(document["error"]["retryable"], false);
        assert_eq!(
            document["error"]["source"],
            format!("{}/scene.tif", server.url)
        );
        assert_eq!(document["error"]["details"]["method"], "HEAD");
        assert_eq!(document["error"]["details"]["attempts"], 1);
        assert_eq!(server.count(), 1, "nonretryable status was retried");
        assert!(server
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|line| line.starts_with("HEAD ")));
        assert!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains(SIGNATURE)),
            "fixture never received the actual signed locator"
        );
    }

    #[test]
    fn exhausted_429_and_503_keep_last_status_and_retryability() {
        for status in [429, 503] {
            let fixture = Fixture::new();
            let server = StatusServer::new(status);
            let path = fixture.request(cog_request(
                &format!("{}/scene.tif?sig={SIGNATURE}", server.url),
                None,
            ));
            let output = run(&[
                "plan",
                path.to_str().unwrap(),
                "--backend",
                "native",
                "--json",
            ]);
            let document = json_error(&output, "SOURCE_IDENTITY_HTTP_ERROR", "source_identity");
            assert_eq!(document["error"]["http_status"], status);
            assert_eq!(document["error"]["retryable"], true);
            assert_eq!(document["error"]["details"]["attempts"], 3);
            assert_eq!(document["error"]["details"]["max_retries"], 2);
            assert_eq!(server.count(), 3, "bounded default retry attempts changed");
            assert!(server
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|line| line.starts_with("HEAD ")));
        }
    }

    #[test]
    fn unsupported_native_storage_is_a_safe_cli_failure() {
        let fixture = Fixture::new();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/native_cog/unsupported_int32.tif");
        let path = fixture.request(cog_request(source.to_str().unwrap(), None));
        let output = run(&["plan", path.to_str().unwrap(), "--backend", "native"]);
        let document = json_error(&output, "OPERATION_FAILED", "cli");
        assert!(document["error"]["http_status"].is_null());
    }

    #[test]
    fn successful_native_uint16_fetch_keeps_success_stdout_contract() {
        let fixture = Fixture::new();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/native_cog/qa_uint16_no_nodata.tif");
        let destination = fixture.0.join("result.zarr");
        let path = fixture.request(cog_request(source.to_str().unwrap(), Some(&destination)));
        let output = run(&[
            "fetch",
            path.to_str().unwrap(),
            "--backend",
            "native",
            "--output-format",
            "zarr_v2",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let manifest: Value =
            serde_json::from_slice(&output.stdout).expect("success remains one JSON document");
        assert_eq!(manifest["document_type"], "acquisition_result");
        assert_eq!(manifest["status"], "complete");
        assert_eq!(manifest["format"], "zarr_v2");
        assert_eq!(manifest["outputs"].as_array().unwrap().len(), 1);
        assert_eq!(manifest["outputs"][0]["native_dtype"], "<u2");
        let attrs: Value =
            serde_json::from_slice(&fs::read(destination.join(".zattrs")).unwrap()).unwrap();
        assert_eq!(attrs["status"], "complete");
        let array: Value = serde_json::from_slice(
            &fs::read(destination.join("observations/000000/values/.zarray")).unwrap(),
        )
        .unwrap();
        assert_eq!(array["dtype"], "<u2");
    }
}
