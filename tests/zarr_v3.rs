//! Persistent v3 lifecycle guards; Python-generated cross-reader fixtures live in
//! bench/zarr_v3_input.py and are the independent scientific oracle.
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tile_ripper::{acquisition, cog::LayoutCache, *};
use zarrs::{array::Array, filesystem::FilesystemStore};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    request: Request,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn fixture(sharded: bool, codec: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "tile-ripper-v3-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let source = root.join("source.zarr");
    fs::create_dir(&source).unwrap();
    let store = Arc::new(FilesystemStore::new(&source).unwrap());
    fs::write(source.join("zarr.json"),r#"{"zarr_format":3,"node_type":"group","attributes":{"tile_ripper_grid":{"crs":"EPSG:32617","transform":[0,10,0,80,0,-10]}}}"#).unwrap();
    for (name, dtype, shape, chunks, dims) in [
        ("x", "float64", vec![8], vec![8], vec!["x"]),
        ("y", "float64", vec![8], vec![8], vec!["y"]),
        ("time", "int64", vec![3], vec![3], vec!["time"]),
        (
            "signal",
            "uint16",
            vec![3, 8, 8],
            vec![2, 4, 4],
            vec!["time", "y", "x"],
        ),
        (
            "temperature",
            "float32",
            vec![3, 8, 8],
            vec![2, 4, 4],
            vec!["time", "y", "x"],
        ),
    ] {
        let mut codecs =
            vec![serde_json::json!({"name":"bytes","configuration":{"endian":"little"}})];
        if codec == "gzip" {
            codecs.push(serde_json::json!({"name":"gzip","configuration":{"level":1}}));
        }
        if codec == "zstd" {
            codecs.push(
                serde_json::json!({"name":"zstd","configuration":{"level":1,"checksum":true}}),
            );
        }
        let mut outer = chunks.clone();
        if sharded && shape.len() == 3 {
            outer = chunks.iter().map(|c| c * 2).collect();
            codecs = vec![
                serde_json::json!({"name":"sharding_indexed","configuration":{"chunk_shape":chunks,"codecs":codecs,"index_codecs":[{"name":"bytes","configuration":{"endian":"little"}},{"name":"crc32c"}],"index_location":"end"}}),
            ];
        }
        let mut attrs = serde_json::json!({});
        if name == "time" {
            attrs = serde_json::json!({"units":"nanoseconds since 2025-01-01 00:00:00","calendar":"proleptic_gregorian"});
        }
        if name == "temperature" {
            attrs["_FillValue"] = serde_json::json!("NaN");
        }
        let m = serde_json::json!({"zarr_format":3,"node_type":"array","shape":shape,"data_type":dtype,"chunk_grid":{"name":"regular","configuration":{"chunk_shape":outer}},"chunk_key_encoding":{"name":"default","configuration":{"separator":"/"}},"fill_value":if dtype=="float32"{serde_json::json!("NaN")}else{serde_json::json!(0)},"codecs":codecs,"dimension_names":dims,"attributes":attrs});
        let a = Array::new_with_metadata(
            store.clone(),
            &format!("/{name}"),
            serde_json::from_value(m).unwrap(),
        )
        .unwrap();
        a.store_metadata().unwrap();
        match name {
            "x" => a
                .store_array_subset(
                    &a.subset_all(),
                    (0..8).map(|i| i as f64 * 10. + 5.).collect::<Vec<_>>(),
                )
                .unwrap(),
            "y" => a
                .store_array_subset(
                    &a.subset_all(),
                    (0..8).map(|i| 75. - i as f64 * 10.).collect::<Vec<_>>(),
                )
                .unwrap(),
            "time" => a
                .store_array_subset(&a.subset_all(), &[0i64, 1, 2])
                .unwrap(),
            "signal" => a
                .store_array_subset(
                    &a.subset_all(),
                    (0..192)
                        .map(|i| {
                            if i / 64 < 2 && i % 64 / 8 < 4 && i % 8 < 4 {
                                0
                            } else {
                                i as u16
                            }
                        })
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            _ => a
                .store_array_subset(
                    &a.subset_all(),
                    (0..192)
                        .map(|i| {
                            if i % 11 == 0 {
                                f32::from_bits(0x7fc01234)
                            } else {
                                -(i as f32) / 7.
                            }
                        })
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
        }
    }
    Fixture {
        root: root.clone(),
        request: Request {
            source: Source::Assets {
                assets: vec![Asset {
                    id: "science".into(),
                    locator: source.to_string_lossy().into(),
                    format: Format::ZarrV3,
                    datetime: None,
                    etag: None,
                }],
            },
            spatial: Spatial::Bounds {
                crs: "EPSG:32617".into(),
                bbox: [15., 15., 65., 65.],
            },
            time: Some(Time::Instants {
                values: vec![
                    "2025-01-01T00:00:00Z".into(),
                    "2025-01-01T00:00:00.000000002Z".into(),
                ],
            }),
            variables: vec![
                Variable::Dataset {
                    path: "signal".into(),
                },
                Variable::Dataset {
                    path: "temperature".into(),
                },
            ],
            quality: vec![],
            grid: Grid::Native,
            operations: vec![],
            sink: Sink::Directory {
                path: root.join("output").to_string_lossy().into(),
            },
            budget: Budget {
                buffer_bytes: 4 * 1024 * 1024,
                max_nodes: 10000,
                chunk_edge: 3,
            },
        },
    }
}
#[test]
fn ordinary_and_sharded_use_same_native_stream_and_exact_bits() {
    for sharded in [false, true] {
        for codec in ["raw", "gzip", "zstd"] {
            let f = fixture(sharded, codec);
            let p = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
            let mut seen = std::collections::BTreeSet::new();
            let mut stream = p.into_stream(Default::default()).unwrap();
            for c in stream.by_ref() {
                let c = c.unwrap();
                let Variable::Dataset { path } = &c.variable else {
                    panic!()
                };
                let ti = c.indices[0];
                assert!([0, 2].contains(&ti));
                for y in 0..c.window.height {
                    for x in 0..c.window.width {
                        let sy = y + c.window.y;
                        let sx = x + c.window.x;
                        let i = ti * 64 + sy * 8 + sx;
                        let pos = y * c.window.width + x;
                        assert!(seen.insert((path.clone(), ti, sy, sx)));
                        if path == "signal" {
                            let expected = if ti < 2 && sy < 4 && sx < 4 {
                                0
                            } else {
                                i as u16
                            };
                            assert_eq!(c.pixels.as_u16().unwrap()[pos], expected);
                            assert!(c.validity().nth(pos).unwrap());
                            assert_eq!(c.nodata, None);
                        } else {
                            let expected = if i % 11 == 0 {
                                f32::from_bits(0x7fc01234)
                            } else {
                                -(i as f32) / 7.
                            };
                            assert_eq!(
                                c.pixels.as_f32().unwrap()[pos].to_bits(),
                                expected.to_bits()
                            );
                            assert_eq!(c.validity().nth(pos).unwrap(), !expected.is_nan());
                        }
                    }
                }
            }
            let report = stream.finish().unwrap();
            assert_eq!(seen.len(), 144);
            assert!(
                report.scheduler["reserved_bytes_peak"].as_u64().unwrap()
                    <= f.request.budget.buffer_bytes as u64
            );
        }
    }
}
#[test]
fn late_local_shard_mutation_invalidates_provisional_chunks() {
    let f = fixture(true, "gzip");
    let p = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let mut stream = p
        .into_stream(stream::StreamOptions { capacity: 0 })
        .unwrap();
    assert!(stream.next().unwrap().is_ok());
    let path = f.root.join("source.zarr/signal/c/0/0/0");
    let mut b = fs::read(&path).unwrap();
    b[0] ^= 1;
    fs::write(path, b).unwrap();
    assert!(stream.finish().is_err());
}
#[test]
fn absent_coordinates_unsupported_crs_and_small_budgets_fail_preparation() {
    let mut f = fixture(true, "raw");
    f.request.budget.buffer_bytes = 128;
    assert!(acquisition::prepare(&f.request, &mut LayoutCache::default()).is_err());
    f.request.budget.buffer_bytes = 4 * 1024 * 1024;
    fs::remove_file(f.root.join("source.zarr/x/c/0")).unwrap();
    assert!(acquisition::prepare(&f.request, &mut LayoutCache::default()).is_err());
    let mut f = fixture(false, "raw");
    f.request.spatial = Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox: [15., 15., 65., 65.],
    };
    assert!(acquisition::prepare(&f.request, &mut LayoutCache::default()).is_err());
    let f = fixture(false, "raw");
    let path = f.root.join("source.zarr/time/zarr.json");
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["attributes"]["_FillValue"] = serde_json::json!(0);
    fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
    assert!(acquisition::prepare(&f.request, &mut LayoutCache::default()).is_err());
}
#[test]
fn cancellation_never_returns_a_successful_finish() {
    let f = fixture(true, "zstd");
    let p = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let mut stream = p
        .into_stream(stream::StreamOptions { capacity: 0 })
        .unwrap();
    assert!(stream.next().unwrap().is_ok());
    stream.cancel();
    assert!(stream.finish().is_err());
}

#[test]
fn legacy_public_entry_points_prepare_v3_bindings_too() {
    let mut f = fixture(true, "raw");
    f.request.sink = Sink::Callback;
    let plan = tile_ripper::plan(&f.request, &mut LayoutCache::default()).unwrap();
    assert_eq!(plan.selected.len(), 2);
    let mut count = 0;
    tile_ripper::execute(&f.request, &mut LayoutCache::default(), |_| {
        count += 1;
        Ok(())
    })
    .unwrap();
    assert!(count > 0);
    f.request.sink = Sink::ZarrDirectory {
        path: f.root.join("out").to_string_lossy().into(),
    };
    tile_ripper::fetch(&f.request, &mut LayoutCache::default()).unwrap();
    assert!(f.root.join("out/manifest.json").is_file());
}
