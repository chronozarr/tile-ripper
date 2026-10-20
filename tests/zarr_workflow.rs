use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tile_ripper::{cog::LayoutCache, *};
use zarrs::{array::Array, array::ArraySubset, filesystem::FilesystemStore};
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
fn fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "tile-ripper-zarr-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join(".zgroup"), r#"{"zarr_format":2}"#).unwrap();
    fs::write(source.join(".zattrs"),r#"{"tile_ripper_grid":{"crs":"EPSG:32617","transform":[0,10,0,80,0,-10]},"title":"native fixture"}"#).unwrap();
    let s = Arc::new(FilesystemStore::new(&source).unwrap());
    for (n, dtype, shape, chunks, dims) in [
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
        fs::create_dir(source.join(n)).unwrap();
        let m = serde_json::json!({"zarr_format":2,"shape":shape,"chunks":chunks,"dtype":dtype,"order":"C","fill_value":0,"filters":null,"compressor":null,"dimension_separator":"."});
        fs::write(
            source.join(n).join(".zarray"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        let mut attrs = serde_json::json!({"_ARRAY_DIMENSIONS":dims});
        if n == "time" {
            attrs["units"] = "seconds since 1970-01-01 00:00:00".into();
            attrs["calendar"] = "proleptic_gregorian".into();
        }
        if n == "red" {
            attrs["_FillValue"] = 0.into();
            attrs["scale_factor"] = 0.1.into();
            attrs["units"] = "DN".into();
        }
        fs::write(
            source.join(n).join(".zattrs"),
            serde_json::to_vec(&attrs).unwrap(),
        )
        .unwrap();
        let ar = Array::open(s.clone(), &format!("/{n}")).unwrap();
        match n {
            "time" => ar
                .store_array_subset(&ar.subset_all(), &[1735689600i64, 1735776000, 1735948800])
                .unwrap(),
            "x" => ar
                .store_array_subset(
                    &ar.subset_all(),
                    (0..8).map(|i| 5. + i as f64 * 10.).collect::<Vec<_>>(),
                )
                .unwrap(),
            "y" => ar
                .store_array_subset(
                    &ar.subset_all(),
                    (0..8).map(|i| 75. - i as f64 * 10.).collect::<Vec<_>>(),
                )
                .unwrap(),
            _ => ar
                .store_array_subset(
                    &ar.subset_all(),
                    (0..192).map(|i| i as u16 + 1).collect::<Vec<_>>(),
                )
                .unwrap(),
        }
    }
    // Missing chunk spans two observations: zarrs must materialize native fill.
    fs::remove_file(source.join("red/0.1.1")).unwrap();
    Fixture {
        root: root.clone(),
        request: Request {
            source: Source::Assets {
                assets: vec![Asset {
                    id: "cube".into(),
                    locator: source.to_string_lossy().into(),
                    format: Format::ZarrV2,
                    datetime: None,
                    etag: None,
                }],
            },
            spatial: Spatial::Bounds {
                crs: "EPSG:32617".into(),
                bbox: [15., 15., 65., 65.],
            },
            time: Some(Time::Instants {
                values: vec!["2025-01-01T00:00:00Z".into(), "2025-01-04T00:00:00Z".into()],
            }),
            variables: vec![Variable::Dataset { path: "red".into() }],
            quality: vec![],
            grid: Grid::Native,
            operations: vec![],
            sink: Sink::Callback,
            budget: Budget {
                buffer_bytes: 8192,
                max_nodes: 1000,
                chunk_edge: 3,
            },
        },
    }
}
#[test]
fn shared_api_native_slices_values_fill_coordinates_and_metadata() {
    let mut f = fixture();
    let r = &mut f.request;
    let mut cache = LayoutCache::default();
    let p = plan(r, &mut cache).unwrap();
    assert_eq!(
        p.selected[0].window,
        Window {
            x: 1,
            y: 1,
            width: 6,
            height: 6
        }
    );
    assert_eq!(
        p.selected[0].dimensions.as_ref().unwrap().indices[0],
        vec![0, 2]
    );
    assert!(p.nodes.iter().any(|n| matches!(
        n.operation,
        Operation::Read {
            read: Read::ChunkObject { .. }
        }
    )));
    let mut values = 0;
    let report = execute(r, &mut cache, |c| {
        assert!(matches!(c.variable, Variable::Dataset { .. }));
        assert_eq!(c.scale, Some(0.1));
        assert_eq!(c.nodata, Some(0.));
        let t = c.indices[0];
        for y in 0..c.window.height {
            for x in 0..c.window.width {
                let sy = c.window.y + y;
                let sx = c.window.x + x;
                let expected = if t < 2 && sy >= 4 && sx >= 4 {
                    0
                } else {
                    (t * 64 + sy * 8 + sx + 1) as u16
                };
                assert_eq!(c.pixels.as_u16().unwrap()[y * c.window.width + x], expected);
                values += 1;
            }
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(values, 72);
    assert_eq!(report.decoded_bytes, 144);
    assert!(report.source_data_requests.unwrap() > 0);
    assert!(report.source_data_bytes.unwrap() > 0);
    assert!(execute(r, &mut cache, |_| Err("cancelled".into())).is_err());
    let out = f.root.join("out");
    r.sink = Sink::ZarrDirectory {
        path: out.to_string_lossy().into(),
    };
    fetch(r, &mut cache).unwrap();
    assert!(fetch(r, &mut cache).is_err());
    let s = Arc::new(FilesystemStore::new(out.join("cube.zarr")).unwrap());
    let x = Array::open(s.clone(), "/x").unwrap();
    assert_eq!(
        x.retrieve_array_subset::<Vec<f64>>(&x.subset_all())
            .unwrap(),
        vec![15., 25., 35., 45., 55., 65.]
    );
    let time = Array::open(s.clone(), "/time").unwrap();
    assert_eq!(
        time.retrieve_array_subset::<Vec<i64>>(&time.subset_all())
            .unwrap(),
        vec![1735689600, 1735948800]
    );
    let red = Array::open(s, "/red").unwrap();
    assert_eq!(red.shape(), &[2, 6, 6]);
    let attrs: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("cube.zarr/.zattrs")).unwrap()).unwrap();
    assert_eq!(
        attrs["tile_ripper_grid"]["transform"],
        serde_json::json!([10., 10., 0., 70., 0., -10.])
    );
}
#[test]
fn rejects_incompatible_grid_codec_budget_and_aliases_before_sink() {
    let mut f = fixture();
    let mut cache = LayoutCache::default();
    f.request.variables.push(Variable::Dataset {
        path: "/red".into(),
    });
    assert!(plan(&f.request, &mut cache).is_err());
    f.request.variables.pop();
    let path = f.root.join("source/red/.zarray");
    let original = fs::read(&path).unwrap();
    let mut m: serde_json::Value = serde_json::from_slice(&original).unwrap();
    m["shape"] = serde_json::json!([3, 7, 8]);
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    assert!(plan(&f.request, &mut cache).is_err());
    m = serde_json::from_slice(&original).unwrap();
    m["compressor"] = serde_json::json!({"id":"blosc"});
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    assert!(plan(&f.request, &mut cache).is_err());
    fs::write(&path, original).unwrap();
    f.request.budget.buffer_bytes = 64;
    assert!(plan(&f.request, &mut cache).is_err());
}
#[test]
fn corrupt_data_cleans_new_output_and_empty_queries_write_only_manifest() {
    let mut f = fixture();
    let mut cache = LayoutCache::default();
    let out = f.root.join("out");
    f.request.sink = Sink::ZarrDirectory {
        path: out.to_string_lossy().into(),
    };
    fs::write(f.root.join("source/red/0.0.0"), b"bad").unwrap();
    plan(&f.request, &mut cache).unwrap(); // Metadata-only planning never decodes corrupt data.
    assert!(fetch(&f.request, &mut cache).is_err());
    assert!(!out.exists());
    f.request.time = Some(Time::Instants {
        values: vec!["2000-01-01T00:00:00Z".into()],
    });
    let report = fetch(&f.request, &mut cache).unwrap();
    assert_eq!(report.chunks, 0);
    assert_eq!(fs::read_dir(out).unwrap().count(), 1);
}
#[test]
fn malformed_coordinates_are_rejected() {
    let f = fixture();
    let s = Arc::new(FilesystemStore::new(f.root.join("source")).unwrap());
    let x = Array::open(s, "/x").unwrap();
    x.store_array_subset(
        &ArraySubset::new_with_start_shape(vec![2], vec![1]).unwrap(),
        &[999f64],
    )
    .unwrap();
    assert!(plan(&f.request, &mut LayoutCache::default()).is_err());
}
