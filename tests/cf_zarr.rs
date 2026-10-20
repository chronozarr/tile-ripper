use std::{fs, sync::Arc};
use tile_ripper::{acquisition, cog::LayoutCache, Pixels, Request, Source};
fn array(
    root: &std::path::Path,
    name: &str,
    shape: &[usize],
    chunks: &[usize],
    dtype: &str,
    dims: &[&str],
    bytes: Option<Vec<u8>>,
) {
    let p = root.join(name);
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join(".zarray"),serde_json::to_vec(&serde_json::json!({"zarr_format":2,"shape":shape,"chunks":chunks,"dtype":dtype,"order":"C","filters":null,"compressor":null,"fill_value":if dtype=="<f4" {serde_json::json!("NaN")}else{serde_json::Value::Null}})).unwrap()).unwrap();
    fs::write(
        p.join(".zattrs"),
        serde_json::to_vec(&serde_json::json!({"_ARRAY_DIMENSIONS":dims})).unwrap(),
    )
    .unwrap();
    if let Some(bytes) = bytes {
        fs::write(p.join("0"), bytes).unwrap();
    }
}
#[test]
fn conventional_float_callback_and_native_output_preserve_source_coordinates() {
    let root = std::env::temp_dir().join(format!("tile-cf-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(".zgroup"), r#"{"zarr_format":2}"#).unwrap();
    fs::write(
        source.join(".zattrs"),
        r#"{"title":"independent CF fixture"}"#,
    )
    .unwrap();
    for (name, values) in [
        ("longitude", vec![0., 5., 10., 15.]),
        ("latitude", vec![30., 40., 50.]),
    ] {
        array(
            &source,
            name,
            &[values.len()],
            &[values.len()],
            "<f8",
            &[name],
            Some(values.iter().flat_map(|v: &f64| v.to_le_bytes()).collect()),
        );
    }
    array(
        &source,
        "time",
        &[3],
        &[3],
        "<i8",
        &["time"],
        Some([0i64, 6, 12].iter().flat_map(|v| v.to_le_bytes()).collect()),
    );
    fs::write(source.join("time/.zattrs"),r#"{"_ARRAY_DIMENSIONS":["time"],"units":"hours since 1959-01-01","calendar":"proleptic_gregorian"}"#).unwrap();
    array(
        &source,
        "temperature",
        &[3, 4, 3],
        &[2, 2, 2],
        "<f4",
        &["time", "longitude", "latitude"],
        None,
    );
    for t in 0..2 {
        for x in 0..2 {
            for y in 0..2 {
                let mut bytes = Vec::new();
                for dt in 0..2 {
                    for dx in 0..2 {
                        for dy in 0..2 {
                            let (ti, xi, yi) = (t * 2 + dt, x * 2 + dx, y * 2 + dy);
                            let value = if ti >= 3 || yi >= 3 || (xi == 1 && yi == 1) {
                                f32::NAN
                            } else {
                                (ti * 100 + xi * 10 + yi) as f32
                            };
                            bytes.extend(value.to_le_bytes());
                        }
                    }
                }
                fs::write(source.join(format!("temperature/{t}.{x}.{y}")), bytes).unwrap();
            }
        }
    }
    let mut r: Request =
        serde_json::from_str(include_str!("../examples/weatherbench2.json")).unwrap();
    let Source::Assets { assets } = &mut r.source else {
        unreachable!()
    };
    assets[0].locator = source.to_str().unwrap().into();
    r.variables = vec![tile_ripper::Variable::Dataset {
        path: "temperature".into(),
    }];
    r.spatial = tile_ripper::Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox: [4., 35., 11., 46.],
    };
    r.sink = tile_ripper::Sink::Directory {
        path: root.join("out").to_str().unwrap().into(),
    };
    let prepared = acquisition::prepare(&r, &mut LayoutCache::default()).unwrap();
    let mut samples = 0;
    prepared
        .execute(|c| {
            let Pixels::Float32(v) = &c.pixels else {
                panic!("native Float32 required")
            };
            for row in 0..c.window.height {
                for col in 0..c.window.width {
                    let pos = c.pixel_center(col, row).unwrap();
                    assert!([5., 10.].contains(&pos[0]));
                    assert!([40., 50.].contains(&pos[1]));
                    let i = row * c.window.width + col;
                    assert_eq!(c.validity().nth(i).unwrap(), !v[i].is_nan());
                    samples += 1;
                }
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(samples, 8);
    let manifest = prepared.fetch().unwrap();
    assert_eq!(manifest["status"], "complete");
    assert_eq!(manifest["outputs"][0]["dtype"], "float32");
    let store =
        Arc::new(zarrs::filesystem::FilesystemStore::new(root.join("out/era5.zarr")).unwrap());
    let ar = zarrs::array::Array::open(store, "/temperature").unwrap();
    let v = ar
        .retrieve_array_subset_elements::<f32>(&ar.subset_all())
        .unwrap();
    assert_eq!(v.len(), 8);
    assert!(v[0].is_nan());
    assert_eq!(v[1], 21.);
    assert_eq!(v[2], 12.);
    assert_eq!(v[7], 122.);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn imerg_sized_native_chunk_still_fits_the_existing_sixteen_mib_profile() {
    use std::io::Write;
    let root = std::env::temp_dir().join(format!("tile-large-native-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".zgroup"), r#"{"zarr_format":2}"#).unwrap();
    fs::write(root.join(".zattrs"), "{}").unwrap();
    for (name, count) in [("latitude", 900), ("longitude", 1800)] {
        array(
            &root,
            name,
            &[count],
            &[count],
            "<f8",
            &[name],
            Some(
                (0..count)
                    .flat_map(|i| (i as f64 * 0.01).to_le_bytes())
                    .collect(),
            ),
        );
    }
    array(
        &root,
        "time",
        &[1],
        &[1],
        "<i8",
        &["time"],
        Some(0i64.to_le_bytes().to_vec()),
    );
    fs::write(root.join("time/.zattrs"),r#"{"_ARRAY_DIMENSIONS":["time"],"units":"hours since 1959-01-01","calendar":"proleptic_gregorian"}"#).unwrap();
    array(
        &root,
        "temperature",
        &[1, 900, 1800],
        &[1, 900, 1800],
        "<f4",
        &["time", "latitude", "longitude"],
        None,
    );
    let path = root.join("temperature/.zarray");
    let mut meta: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    meta["compressor"] = serde_json::json!({"id":"gzip","level":1});
    fs::write(path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let mut encoded = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoded
        .write_all(&5f32.to_le_bytes().repeat(900 * 1800))
        .unwrap();
    fs::write(root.join("temperature/0.0.0"), encoded.finish().unwrap()).unwrap();
    let mut request: Request =
        serde_json::from_str(include_str!("../examples/weatherbench2.json")).unwrap();
    let Source::Assets { assets } = &mut request.source else {
        unreachable!()
    };
    assets[0].locator = root.to_string_lossy().into_owned();
    request.variables = vec![tile_ripper::Variable::Dataset {
        path: "temperature".into(),
    }];
    request.time = None;
    request.spatial = tile_ripper::Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox: [-0.005, -0.005, 0.015, 0.015],
    };
    request.budget.buffer_bytes = 16 * 1024 * 1024;
    request.budget.chunk_edge = 32;
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    let mut stream = prepared.into_stream(Default::default()).unwrap();
    let mut values = 0;
    for chunk in stream.by_ref() {
        let chunk = chunk.unwrap();
        assert!(chunk.pixels.as_f32().unwrap().iter().all(|v| *v == 5.));
        values += chunk.pixels.len();
    }
    let report = stream.finish().unwrap();
    assert_eq!(values, 4);
    assert_eq!(report.native_materialized_bytes, Some(6_480_000));
    assert_eq!(report.source_data_requests, Some(1));
    assert!(report.scheduler["reserved_bytes_peak"].as_u64().unwrap() <= 16 * 1024 * 1024);
    fs::remove_dir_all(root).unwrap();
}
