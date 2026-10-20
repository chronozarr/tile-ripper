use std::process::Command;
fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .args(args)
        .env_remove("TILE_RIPPER_COG_BACKEND")
        .output()
        .unwrap()
}
#[test]
fn help_and_actionable_errors_do_not_access_network_or_echo_secrets() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--dry-run"));
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("--backend native|gdal"));
    assert!(text.contains("--gdal-free"));
    assert!(text.contains("stream emits NDJSON descriptors"));
    for (args, expected) in [
        (
            vec!["fetch", "--source", "https://secret:password@example.com/"],
            "missing --format",
        ),
        (vec!["plan", "--surprise", "secret"], "unknown option"),
        (vec!["plan", "missing.json"], "cannot read request file"),
        (
            vec!["plan", "examples/weatherbench2.json", "--bbox", "1,2,3,4"],
            "do not mix",
        ),
        (vec!["plan", "--output"], "requires a value"),
        (
            vec!["plan", "missing.json", "--backend", "automatic"],
            "--backend requires native or gdal",
        ),
        (
            vec![
                "plan",
                "missing.json",
                "--backend",
                "native",
                "--backend",
                "gdal",
            ],
            "duplicate option",
        ),
        (
            vec!["plan", "missing.json", "--gdal-free", "--backend", "gdal"],
            "--gdal-free cannot be combined",
        ),
    ] {
        let result = run(&args);
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("password"));
    }
}

#[test]
fn explicit_backend_and_linkage_guard_match_compiled_features_before_io() {
    for (backend, available, unavailable) in [
        (
            "native",
            cfg!(feature = "native-cog"),
            "native COG backend is not compiled",
        ),
        (
            "gdal",
            cfg!(feature = "gdal"),
            "GDAL COG backend is not compiled",
        ),
    ] {
        let result = run(&["plan", "missing.json", "--backend", backend]);
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains(if available {
                "cannot read request file"
            } else {
                unavailable
            }),
            "{error}"
        );
    }
    let result = run(&["plan", "missing.json", "--gdal-free"]);
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        error.contains(if cfg!(feature = "gdal") {
            "--gdal-free requires a binary built without GDAL"
        } else {
            "cannot read request file"
        }),
        "{error}"
    );
}

#[test]
fn invalid_inherited_cog_backend_is_rejected_before_catalog_discovery() {
    let result = Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .args(["plan", "docs/native-sentinel/request.json"])
        .env("TILE_RIPPER_COG_BACKEND", "unavailable")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("COG backend unavailable"), "{error}");
    assert!(!error.contains("https://"));
}

#[test]
#[cfg(all(not(feature = "gdal"), feature = "native-cog"))]
fn gdal_free_geotiff_fetch_has_an_actionable_zarr_alternative_before_network() {
    let result = run(&[
        "fetch",
        "docs/native-sentinel/request.json",
        "--gdal-free",
        "--output",
        "unused",
        "--output-format",
        "geotiff",
    ]);
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("--output-format zarr_v2"), "{error}");
}

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "tile-ripper-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn zarr_stream_uses_a_callback_and_accepts_gdal_free_builds() {
    let f = Fixture::new();
    let source = f.0.join("source.zarr");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join(".zgroup"), r#"{"zarr_format":2}"#).unwrap();
    std::fs::write(
        source.join(".zattrs"),
        r#"{"tile_ripper_grid":{"crs":"EPSG:32617","transform":[0,10,0,20,0,-10]}}"#,
    )
    .unwrap();
    for (name, dtype, shape, dims, bytes) in [
        (
            "x",
            "<f8",
            vec![2],
            vec!["x"],
            [5f64, 15.]
                .into_iter()
                .flat_map(f64::to_le_bytes)
                .collect::<Vec<_>>(),
        ),
        (
            "y",
            "<f8",
            vec![2],
            vec!["y"],
            [15f64, 5.].into_iter().flat_map(f64::to_le_bytes).collect(),
        ),
        (
            "time",
            "<i8",
            vec![1],
            vec!["time"],
            1735689600i64.to_le_bytes().to_vec(),
        ),
        (
            "red",
            "<u2",
            vec![1, 2, 2],
            vec!["time", "y", "x"],
            [1u16, 2, 3, 4]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
    ] {
        let path = source.join(name);
        std::fs::create_dir(&path).unwrap();
        let meta = serde_json::json!({"zarr_format":2,"shape":shape,"chunks":shape,"dtype":dtype,"order":"C","filters":null,"compressor":null,"fill_value":0});
        std::fs::write(path.join(".zarray"), serde_json::to_vec(&meta).unwrap()).unwrap();
        let mut attrs = serde_json::json!({"_ARRAY_DIMENSIONS":dims});
        if name == "time" {
            attrs["units"] = "seconds since 1970-01-01 00:00:00".into();
            attrs["calendar"] = "proleptic_gregorian".into();
        }
        std::fs::write(path.join(".zattrs"), serde_json::to_vec(&attrs).unwrap()).unwrap();
        std::fs::write(path.join(if name == "red" { "0.0.0" } else { "0" }), bytes).unwrap();
    }
    let output = f.0.join("must-not-exist");
    let request = serde_json::json!({"source":{"kind":"assets","assets":[{"id":"z","locator":source,"format":"zarr_v2"}]},"spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[0,0,20,20]},"variables":[{"kind":"dataset","path":"red"}],"grid":{"kind":"native"},"sink":{"kind":"directory","path":output},"budget":{"buffer_bytes":4096,"max_nodes":1000,"chunk_edge":2}});
    let path = f.0.join("request.json");
    std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let mut args = vec!["stream", path.to_str().unwrap()];
    if !cfg!(feature = "gdal") {
        args.push("--gdal-free");
    }
    let result = run(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let records = String::from_utf8_lossy(&result.stdout)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["type"], "chunk");
    assert_eq!(records[0]["valid_pixels"], 4);
    assert_eq!(records[1]["type"], "complete");
    assert!(!output.exists());
}

#[test]
#[cfg(all(feature = "native-cog", feature = "gdal"))]
fn explicit_native_profile_error_does_not_fall_back_to_gdal() {
    let f = Fixture::new();
    let source = f.0.join("striped.tif");
    let mut ds = gdal::DriverManager::get_driver_by_name("GTiff")
        .unwrap()
        .create_with_band_type::<u16, _>(&source, 2, 2, 1)
        .unwrap();
    ds.set_geo_transform(&[0., 10., 0., 20., 0., -10.]).unwrap();
    ds.set_spatial_ref(&gdal::spatial_ref::SpatialRef::from_epsg(32617).unwrap())
        .unwrap();
    drop(ds);
    let request = serde_json::json!({"source":{"kind":"assets","assets":[{"id":"c","locator":source,"format":"cog"}]},"spatial":{"kind":"bounds","crs":"EPSG:32617","bbox":[0,0,20,20]},"variables":[{"kind":"band","index":1}],"grid":{"kind":"native"},"sink":{"kind":"callback"},"budget":{"buffer_bytes":33554432,"max_nodes":1000,"chunk_edge":2}});
    let path = f.0.join("request.json");
    std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let select = |backend: &str, inherited: &str| {
        Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
            .args(["plan", path.to_str().unwrap(), "--backend", backend])
            .env("TILE_RIPPER_COG_BACKEND", inherited)
            .output()
            .unwrap()
    };
    let native = select("native", "gdal");
    assert!(!native.status.success());
    assert!(String::from_utf8_lossy(&native.stderr).contains("native COG requires tiled"));
    let gdal = select("gdal", "native");
    assert!(
        gdal.status.success(),
        "{}",
        String::from_utf8_lossy(&gdal.stderr)
    );
}

#[test]
fn existing_output_is_rejected_before_remote_inspection() {
    let result = run(&["fetch", "examples/weatherbench2.json", "--output", "."]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("choose --output NEW_DIRECTORY"));
}
