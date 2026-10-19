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
fn fixture(format: Format) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "tile-ripper-shared-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let source = root.join(if format == Format::Cog {
        "source.tif"
    } else {
        "source.zarr"
    });
    if format == Format::Cog {
        let mut ds = gdal::DriverManager::get_driver_by_name("GTiff")
            .unwrap()
            .create_with_band_type::<u16, _>(&source, 8, 8, 1)
            .unwrap();
        ds.set_geo_transform(&[0., 10., 0., 80., 0., -10.]).unwrap();
        ds.set_spatial_ref(&gdal::spatial_ref::SpatialRef::from_epsg(32617).unwrap())
            .unwrap();
        let mut b = ds.rasterband(1).unwrap();
        b.set_no_data_value(Some(0.)).unwrap();
        b.set_scale(0.1).unwrap();
        b.set_offset(-1.).unwrap();
        let values = (0..64)
            .map(|i| {
                if i / 8 >= 4 && i % 8 >= 4 {
                    0
                } else {
                    i as u16 + 65
                }
            })
            .collect();
        b.write(
            (0, 0),
            (8, 8),
            &mut gdal::raster::Buffer::new((8, 8), values),
        )
        .unwrap();
    } else {
        fs::create_dir(&source).unwrap();
        fs::write(source.join(".zgroup"), r#"{"zarr_format":2}"#).unwrap();
        fs::write(
            source.join(".zattrs"),
            r#"{"tile_ripper_grid":{"crs":"EPSG:32617","transform":[0,10,0,80,0,-10]}}"#,
        )
        .unwrap();
        let store = Arc::new(FilesystemStore::new(&source).unwrap());
        for (name, dtype, shape, chunks, dims) in [
            ("x", "<f8", vec![8], vec![8], vec!["x"]),
            ("y", "<f8", vec![8], vec![8], vec!["y"]),
            ("time", "<i8", vec![3], vec![3], vec!["time"]),
            (
                "red",
                "<u2",
                vec![3, 8, 8],
                vec![2, 4, 4],
                vec!["time", "y", "x"],
            ),
        ] {
            fs::create_dir(source.join(name)).unwrap();
            let m = serde_json::json!({"zarr_format":2,"shape":shape,"chunks":chunks,"dtype":dtype,"order":"C","filters":null,"compressor":null,"fill_value":0});
            fs::write(
                source.join(name).join(".zarray"),
                serde_json::to_vec(&m).unwrap(),
            )
            .unwrap();
            let mut attrs = serde_json::json!({"_ARRAY_DIMENSIONS":dims});
            if name == "time" {
                attrs["units"] = "seconds since 1970-01-01 00:00:00".into();
                attrs["calendar"] = "proleptic_gregorian".into();
            }
            if name == "red" {
                attrs["scale_factor"] = 0.1.into();
                attrs["add_offset"] = (-1.).into();
            }
            fs::write(
                source.join(name).join(".zattrs"),
                serde_json::to_vec(&attrs).unwrap(),
            )
            .unwrap();
            let ar = Array::open(store.clone(), &format!("/{name}")).unwrap();
            match name {
                "time" => ar
                    .store_array_subset_elements(
                        &ar.subset_all(),
                        &[1735689600i64, 1735776000, 1735948800],
                    )
                    .unwrap(),
                "x" => ar
                    .store_array_subset_elements(
                        &ar.subset_all(),
                        &(0..8).map(|i| i as f64 * 10. + 5.).collect::<Vec<_>>(),
                    )
                    .unwrap(),
                "y" => ar
                    .store_array_subset_elements(
                        &ar.subset_all(),
                        &(0..8).map(|i| 75. - i as f64 * 10.).collect::<Vec<_>>(),
                    )
                    .unwrap(),
                _ => ar
                    .store_array_subset_elements(
                        &ar.subset_all(),
                        &(1..=192).map(|i| i as u16).collect::<Vec<_>>(),
                    )
                    .unwrap(),
            }
        }
        fs::remove_file(source.join("red/0.1.1")).unwrap();
    }
    Fixture {
        root: root.clone(),
        request: Request {
            source: Source::Assets {
                assets: vec![Asset {
                    id: "source".into(),
                    locator: source.to_string_lossy().into(),
                    format,
                    datetime: Some("2025-01-02T00:00:00Z".into()),
                    etag: None,
                }],
            },
            spatial: Spatial::Bounds {
                crs: "EPSG:32617".into(),
                bbox: [15., 15., 65., 65.],
            },
            time: Some(Time::Interval {
                start: "2025-01-02T00:00:00Z".into(),
                end: "2025-01-02T00:00:00Z".into(),
            }),
            variables: vec![if format == Format::Cog {
                Variable::Band { index: 1 }
            } else {
                Variable::Dataset { path: "red".into() }
            }],
            quality: vec![],
            grid: Grid::Native,
            operations: vec![],
            sink: Sink::Directory {
                path: root.join("out").to_string_lossy().into(),
            },
            budget: Budget {
                buffer_bytes: 8192,
                max_nodes: 1000,
                chunk_edge: 3,
            },
        },
    }
}
#[test]
fn same_public_lifecycle_preserves_native_selection_and_provenance() {
    for format in [Format::Cog, Format::ZarrV2] {
        let f = fixture(format);
        let mut cache = LayoutCache::default();
        let prepared = acquisition::prepare(&f.request, &mut cache).unwrap();
        let inspection = prepared.explain();
        assert_eq!(inspection["document_type"], "inspection_only");
        assert!(inspection["logical_request"]["spatial"].is_object());
        assert!(
            inspection["output_estimates"]["native_value_bytes"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(inspection["output_estimates"]["encoded_file_bytes"].is_null());
        assert_eq!(inspection["saved_plan_execution_supported"], false);
        assert_eq!(prepared.plan().selected[0].timestamps.len(), 1);
        let mut seen = std::collections::BTreeMap::new();
        let report = prepared
            .execute(|c| {
                assert_eq!(c.nodata, Some(0.));
                assert_eq!(c.scale, Some(0.1));
                assert_eq!(c.offset, Some(-1.));
                for y in 0..c.window.height {
                    for x in 0..c.window.width {
                        let sy = c.window.y + y;
                        let sx = c.window.x + x;
                        assert_eq!(
                            c.pixels.as_u16().unwrap()[y * c.window.width + x],
                            if sy >= 4 && sx >= 4 {
                                0
                            } else {
                                (sy * 8 + sx + 65) as u16
                            }
                        );
                        assert!(seen.insert((sy, sx), ()).is_none());
                    }
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(seen.len(), 36);
        if format == Format::ZarrV2 {
            let planned: std::collections::BTreeSet<_> = prepared
                .plan()
                .nodes
                .iter()
                .filter_map(|n| match &n.operation {
                    Operation::Read {
                        read: Read::ChunkObject { key, .. },
                    } => Some(key.clone()),
                    _ => None,
                })
                .collect();
            let actual: std::collections::BTreeSet<_> = report
                .source_reads
                .iter()
                .map(|r| {
                    assert_eq!(r.requests, 1);
                    r.key.clone()
                })
                .collect();
            assert_eq!(planned, actual);
            assert_eq!(actual.len(), 4);
        }
        let result = prepared.fetch().unwrap();
        assert_eq!(result["status"], "complete");
        assert_eq!(result["schema_version"], 3);
        assert!(result["outputs"].as_array().is_some_and(|v| !v.is_empty()));
        for o in result["outputs"].as_array().unwrap() {
            assert_eq!(o["nodata"], 0.);
            assert_eq!(o["dtype"], "uint16");
            assert_eq!(o["timestamps"].as_array().unwrap().len(), 1);
            assert!(f
                .root
                .join("out")
                .join(o["file"].as_str().unwrap())
                .exists());
        }
        assert!(prepared.fetch().is_err());
    }
}
#[test]
fn empty_time_queries_and_partial_edges_share_the_lifecycle() {
    for format in [Format::Cog, Format::ZarrV2] {
        let mut f = fixture(format);
        let mut cache = LayoutCache::default();
        f.request.time = Some(Time::Instants {
            values: vec!["2000-01-01T00:00:00Z".into()],
        });
        let p = acquisition::prepare(&f.request, &mut cache).unwrap();
        assert!(p.plan().selected.is_empty());
        assert_eq!(p.fetch().unwrap()["outputs"], serde_json::json!([]));
        f.request.time = None;
        f.request.spatial = Spatial::Bounds {
            crs: "EPSG:32617".into(),
            bbox: [-10., 65., 15., 90.],
        };
        let p = acquisition::prepare(&f.request, &mut cache).unwrap();
        assert_eq!(
            p.plan().selected[0].window,
            Window {
                x: 0,
                y: 0,
                width: 2,
                height: 2
            }
        );
    }
}
#[test]
fn unsupported_missing_and_source_changes_fail_before_callbacks() {
    for format in [Format::Cog, Format::ZarrV2] {
        let mut f = fixture(format);
        let mut cache = LayoutCache::default();
        let original = f.request.variables.clone();
        f.request.variables = vec![if format == Format::Cog {
            Variable::Band { index: 2 }
        } else {
            Variable::Dataset {
                path: "absent".into(),
            }
        }];
        assert!(acquisition::prepare(&f.request, &mut cache).is_err());
        f.request.variables = original;
        f.request.grid = Grid::Target {
            crs: "EPSG:4326".into(),
            resolution: [1., 1.],
            resampling: "nearest".into(),
        };
        assert!(acquisition::prepare(&f.request, &mut cache).is_err());
        f.request.grid = Grid::Native;
        let prepared = acquisition::prepare(&f.request, &mut cache).unwrap();
        let path = f.root.join(if format == Format::Cog {
            "source.tif"
        } else {
            "source.zarr/red/0.0.0"
        });
        fs::write(path, b"changed source").unwrap();
        let mut calls = 0;
        assert!(prepared
            .execute(|_| {
                calls += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(calls, 0);
        assert!(prepared.fetch().is_err());
        assert!(!f.root.join("out").exists());
    }
}
#[test]
fn cancellation_mutation_during_execution_and_corrupt_cleanup() {
    for format in [Format::Cog, Format::ZarrV2] {
        let f = fixture(format);
        let mut cache = LayoutCache::default();
        let p = acquisition::prepare(&f.request, &mut cache).unwrap();
        assert!(p.execute(|_| Err("cancel".into())).is_err());
    }
    let f = fixture(Format::ZarrV2);
    let mut cache = LayoutCache::default();
    let p = acquisition::prepare(&f.request, &mut cache).unwrap();
    let attr = f.root.join("source.zarr/.zattrs");
    let original = fs::read(&attr).unwrap();
    assert!(p
        .execute(|_| {
            fs::write(&attr, b"mutated")?;
            Ok(())
        })
        .is_err());
    fs::write(attr, original).unwrap();
    fs::write(f.root.join("source.zarr/red/0.0.0"), b"bad").unwrap();
    let p = acquisition::prepare(&f.request, &mut cache).unwrap();
    assert!(p.fetch().is_err());
    assert!(!f.root.join("out").exists());
}
#[test]
fn missing_chunk_creation_conflicts_and_inspection_cannot_modify_execution() {
    let f = fixture(Format::ZarrV2);
    let mut cache = LayoutCache::default();
    let p = acquisition::prepare(&f.request, &mut cache).unwrap();
    let mut doc = p.explain();
    doc["plan"]["selected"] = serde_json::json!([]);
    assert_eq!(p.plan().selected.len(), 1);
    p.execute(|_| Ok(())).unwrap();
    fs::write(f.root.join("source.zarr/red/0.1.1"), vec![0u8; 64]).unwrap();
    assert!(p.execute(|_| Ok(())).is_err());
}
#[test]
fn diagnostics_remove_signed_urls_and_invalid_catalog_semantics() {
    let mut f = fixture(Format::Cog);
    let Source::Assets { assets } = &mut f.request.source else {
        unreachable!()
    };
    assets[0].locator =
        "https://name:password@example.invalid/a.tif?token=NEVER_PRINT#fragment".into();
    f.request.variables = vec![Variable::Band { index: 0 }];
    f.request.grid = Grid::Target {
        crs: "EPSG:4326".into(),
        resolution: [1., 1.],
        resampling: "nearest".into(),
    };
    let error = match acquisition::prepare(&f.request, &mut LayoutCache::default()) {
        Ok(_) => panic!(),
        Err(e) => e.to_string(),
    };
    assert!(!error.contains("NEVER_PRINT") && !error.contains("password"));
}

#[test]
fn local_references_preserve_literal_question_marks() {
    for format in [Format::Cog, Format::ZarrV2] {
        let mut f = fixture(format);
        let Source::Assets { assets } = &mut f.request.source else {
            unreachable!()
        };
        let path = f.root.join(if format == Format::Cog {
            "source?subset.tif"
        } else {
            "source?subset.zarr"
        });
        fs::rename(&assets[0].locator, &path).unwrap();
        assets[0].locator = path.to_string_lossy().into();
        let expected = assets[0].locator.clone();
        let p = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
        assert_eq!(p.explain()["sources"][0]["reference"], expected);
        p.execute(|_| Ok(())).unwrap();
    }
}

#[test]
fn buffered_output_eviction_preserves_values_and_deterministic_objects() {
    let mut f = fixture(Format::ZarrV2);
    f.request.time = None;
    f.request.spatial = Spatial::Bounds {
        crs: "EPSG:32617".into(),
        bbox: [10., 10., 80., 80.],
    };
    f.request.budget.buffer_bytes = 1024;
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let mut expected = std::collections::BTreeMap::new();
    let streamed = prepared
        .execute(|chunk| {
            assert_eq!(chunk.validity().count(), chunk.pixels.len());
            assert_eq!(chunk.pixel_center(chunk.window.width, 0), None);
            let center = chunk.pixel_center(0, 0).unwrap();
            assert_eq!(
                center,
                [
                    chunk.window.x as f64 * 10. + 5.,
                    75. - chunk.window.y as f64 * 10.
                ]
            );
            for y in 0..chunk.window.height {
                for x in 0..chunk.window.width {
                    expected.insert(
                        (chunk.indices[0], chunk.window.y + y, chunk.window.x + x),
                        chunk.pixels.as_u16().unwrap()[y * chunk.window.width + x],
                    );
                }
            }
            Ok(())
        })
        .unwrap();
    let manifest = prepared.fetch().unwrap();
    let io = &manifest["observed"]["output_io"];
    assert!(io["buffered_bytes_peak"].as_u64().unwrap() <= 256);
    assert_eq!(
        manifest["observed"]["source_data_requests"].as_u64(),
        streamed.source_data_requests
    );
    let read = |root: &std::path::Path| {
        let ar = Array::open(
            Arc::new(FilesystemStore::new(root.join("source.zarr")).unwrap()),
            "/red",
        )
        .unwrap();
        ar.retrieve_array_subset_elements::<u16>(&ar.subset_all())
            .unwrap()
    };
    let out = f.root.join("out");
    let actual = read(&out);
    for t in 0..3 {
        for y in 0..7 {
            for x in 0..7 {
                assert_eq!(actual[(t * 7 + y) * 7 + x], expected[&(t, y, x + 1)]);
            }
        }
    }
    // Repeat with a different cache capacity: encoded data and metadata must be
    // byte-identical even when eviction and write order differ. Timings excluded.
    f.request.sink = Sink::Directory {
        path: f.root.join("out2").to_string_lossy().into(),
    };
    f.request.budget.buffer_bytes = 8192;
    acquisition::fetch(&f.request, &mut LayoutCache::default()).unwrap();
    fn objects(
        root: &std::path::Path,
        base: &std::path::Path,
        out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(root).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                objects(&p, base, out);
            } else if p.file_name().unwrap() != "manifest.json" {
                out.insert(
                    p.strip_prefix(base).unwrap().to_owned(),
                    fs::read(&p).unwrap(),
                );
            }
        }
    }
    let (mut first, mut second) = (Default::default(), Default::default());
    objects(&out, &out, &mut first);
    objects(&f.root.join("out2"), &f.root.join("out2"), &mut second);
    assert_eq!(first, second);
}

#[test]
fn output_object_bookkeeping_is_bounded_before_directory_creation() {
    let mut f = fixture(Format::ZarrV2);
    f.request.time = None;
    f.request.budget.chunk_edge = 1;
    f.request.budget.max_nodes = 64;
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    assert!(prepared
        .fetch()
        .unwrap_err()
        .to_string()
        .contains("output object bookkeeping"));
    assert!(!f.root.join("out").exists());
}
