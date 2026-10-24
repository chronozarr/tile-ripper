//! Offline bounded-layout and native-value tests. Independent Python Zarr
//! scientific comparisons are recorded separately by bench/alphaearth_probe.py.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tile_ripper::alphaearth::{self, Request};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn write_json(path: PathBuf, v: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(v).unwrap()).unwrap();
}
fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0x82f63b78 } else { 0 };
        }
    }
    !crc
}
fn value(c: usize, y: usize, x: usize) -> i8 {
    (((c * 7 + y * 3 + x) % 255) as i16 - 127) as i8
}
fn fixture(chunks: &[(usize, usize)]) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "tile-ripper-alphaearth-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let mut group:Value=serde_json::from_str(r#"{"zarr_format":3,"node_type":"group","attributes":{"proj:code":"EPSG:4326","spatial:dimensions":["y","x"],"spatial:transform":[8.983111749910169e-05,0.0,-180.0,0.0,-8.983111749910169e-05,83.68570533713473],"spatial:transform_type":"affine","spatial:bbox":[-180.0,-83.36280346631479,180.2213438735178,83.68570533713473],"spatial:shape":[1859584,4009984],"spatial:registration":"pixel","geoemb:type":"pixel","geoemb:dimensions":64,"geoemb:model":"https://developers.google.com/earth-engine/datasets/catalog/GOOGLE_SATELLITE_EMBEDDING_V1_ANNUAL","geoemb:source_data":"https://source.coop/tge-labs/aef/v1/annual/","geoemb:data_type":"int8","geoemb:gsd":8.983111749910169e-05,"geoemb:quantization":{"method":"signed_square","original_dtype":"float32","quantized_dtype":"int8","formula":"(x / 127.5) ** 2 * sign(x)","valid_range":[-127,127],"nodata":-128},"zarr_conventions":[{"uuid":"f17cb550-5864-4468-aeb7-f3180cfb622f","name":"proj:","description":"Coordinate reference system information for geospatial data","spec_url":"https://github.com/zarr-experimental/geo-proj/blob/v1/README.md","schema_url":"https://raw.githubusercontent.com/zarr-experimental/geo-proj/refs/tags/v1/schema.json"},{"uuid":"689b58e2-cf7b-45e0-9fff-9cfc0883d6b4","name":"spatial:","description":"Spatial coordinate information","spec_url":"https://github.com/zarr-conventions/spatial/blob/v1/README.md","schema_url":"https://raw.githubusercontent.com/zarr-conventions/spatial/refs/tags/v1/schema.json"},{"uuid":"61c12cc5-0e28-4056-999a-480cf3fb7e4c","name":"geoemb:","description":"Geo-embeddings metadata for machine learning embeddings stored in Zarr","spec_url":"https://github.com/geo-embeddings/embeddings-zarr-convention/tree/add-convention"}]}}"#).unwrap();
    group["attributes"]["spatial:shape"] = json!([8192, 8192]);
    group["attributes"]["spatial:transform"] = json!([0.01, 0., -180., 0., -0.01, 80.]);
    group["attributes"]["spatial:bbox"] = json!([-180., -1.92, -98.08, 80.]);
    write_json(root.join("zarr.json"), &group);
    let mut embed:Value=serde_json::from_str(r#"{"zarr_format":3,"node_type":"array","shape":[9,64,1859584,4009984],"data_type":"int8","chunk_grid":{"name":"regular","configuration":{"chunk_shape":[1,64,4096,4096]}},"chunk_key_encoding":{"name":"default","configuration":{"separator":"/"}},"fill_value":-128,"codecs":[{"name":"sharding_indexed","configuration":{"chunk_shape":[1,64,256,256],"codecs":[{"name":"bytes","configuration":{"endian":"little"}},{"name":"zstd","configuration":{"level":3,"checksum":false}}],"index_codecs":[{"name":"bytes","configuration":{"endian":"little"}},{"name":"crc32c"}],"index_location":"end"}}],"attributes":{"_zarrs":{"description":"This array was created with zarrs","repository":"https://github.com/zarrs/zarrs","version":"0.23.0"}},"dimension_names":["time","band","y","x"]}"#).unwrap();
    embed["shape"] = json!([1, 64, 8192, 8192]);
    write_json(root.join("embeddings/zarr.json"), &embed);
    for (name, n, dtype) in [
        ("time", 1, "int32"),
        ("x", 8192, "float64"),
        ("y", 8192, "float64"),
    ] {
        let meta = json!({"zarr_format":3,"node_type":"array","shape":[n],"data_type":dtype,"chunk_grid":{"name":"regular","configuration":{"chunk_shape":[n]}},"chunk_key_encoding":{"name":"default","configuration":{"separator":"/"}},"fill_value":0,"codecs":[{"name":"bytes","configuration":{"endian":"little"}}],"dimension_names":[name],"attributes":{}});
        write_json(root.join(format!("{name}/zarr.json")), &meta);
        let bytes: Vec<u8> = if name == "time" {
            2024i32.to_le_bytes().to_vec()
        } else {
            (0..n)
                .flat_map(|i| {
                    if name == "x" {
                        (-180. + 0.01 * (i as f64 + 0.5)).to_le_bytes()
                    } else {
                        (80. - 0.01 * (i as f64 + 0.5)).to_le_bytes()
                    }
                })
                .collect()
        };
        fs::create_dir_all(root.join(format!("{name}/c"))).unwrap();
        fs::write(root.join(format!("{name}/c/0")), bytes).unwrap();
    }
    let mut shards = std::collections::BTreeMap::<(usize, usize), (Vec<u8>, Vec<u64>)>::new();
    for (y, x) in chunks {
        let values: Vec<u8> = (0..64)
            .flat_map(|c| {
                (0..256).flat_map(move |dy| {
                    (0..256).map(move |dx| value(c, y * 256 + dy, x * 256 + dx) as u8)
                })
            })
            .collect();
        let compressed = zstd::bulk::compress(&values, 3).unwrap();
        let (payload, index) = shards
            .entry((y / 16, x / 16))
            .or_insert_with(|| (vec![], vec![u64::MAX; 512]));
        let entry = ((y % 16) * 16 + x % 16) * 2;
        index[entry] = payload.len() as u64;
        index[entry + 1] = compressed.len() as u64;
        payload.extend(compressed);
    }
    for ((y, x), (mut payload, index)) in shards {
        let index: Vec<u8> = index.iter().flat_map(|v| v.to_le_bytes()).collect();
        payload.extend(&index);
        payload.extend(crc32c(&index).to_le_bytes());
        let path = root.join(format!("embeddings/c/0/0/{y}/{x}"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, payload).unwrap();
    }
    Fixture { root }
}
fn request(f: &Fixture, start: [usize; 2], count: [usize; 2]) -> Request {
    Request {
        locator: f.root.to_str().unwrap().into(),
        time_index: 0,
        start,
        count,
        channels: None,
        network_bytes: 32 * 1024 * 1024,
        network_requests: 128,
        working_bytes: 64 * 1024 * 1024,
    }
}
fn verify(r: Request, expected_chunks: usize) {
    let mut p = alphaearth::prepare(r.clone()).unwrap();
    let stop = AtomicBool::new(false);
    let mut seen = 0;
    while let Some(c) = p.next_chunk(&stop).unwrap() {
        let mut expected = Vec::new();
        for channel in &c.channel_indices {
            for y in c.start[0]..c.start[0] + c.shape[2] {
                for x in c.start[1]..c.start[1] + c.shape[3] {
                    expected.push(value(*channel, y, x));
                }
            }
        }
        assert_eq!(c.values, expected);
        assert_eq!(c.dimension_names, ["time", "band", "y", "x"]);
        assert_eq!(c.year, 2024);
        seen += 1;
    }
    assert_eq!(seen, expected_chunks);
    assert_eq!(p.report()["complete"], true);
    assert!(p.next_chunk(&stop).unwrap().is_none());
}
#[test]
fn exact_int8_one_pixel_small_window_and_channel_order() {
    let f = fixture(&[(0, 0)]);
    verify(request(&f, [3, 5], [1, 1]), 1);
    let mut r = request(&f, [10, 11], [3, 4]);
    r.channels = Some(vec![63, 0, 17]);
    verify(r, 1);
}
#[test]
fn native_chunks_across_logical_and_physical_boundaries() {
    let f = fixture(&[
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
        (15, 15),
        (15, 16),
        (16, 15),
        (16, 16),
    ]);
    verify(request(&f, [255, 255], [2, 2]), 4);
    verify(request(&f, [4095, 4095], [2, 2]), 4);
}
#[test]
fn missing_shard_and_inner_chunk_preserve_fill() {
    let f = fixture(&[(0, 0)]);
    for start in [[300, 300], [5000, 5000]] {
        let mut p = alphaearth::prepare(request(&f, start, [1, 1])).unwrap();
        let c = p.next_chunk(&AtomicBool::new(false)).unwrap().unwrap();
        assert!(!c.present);
        assert_eq!(c.values, vec![-128; 64]);
        assert!(p.next_chunk(&AtomicBool::new(false)).unwrap().is_none());
    }
}
#[test]
fn bounded_errors_cancellation_are_terminal() {
    let f = fixture(&[(0, 0)]);
    let mut r = request(&f, [0, 0], [1, 1]);
    r.working_bytes = 1024;
    assert!(alphaearth::prepare(r).is_err());
    let mut r = request(&f, [0, 0], [1, 1]);
    r.network_bytes = 10;
    assert!(alphaearth::prepare(r).is_err());
    let mut r = request(&f, [0, 0], [1, 1]);
    r.channels = Some(vec![1, 1]);
    assert!(alphaearth::prepare(r).is_err());
    let mut p = alphaearth::prepare(request(&f, [0, 0], [1, 1])).unwrap();
    assert!(p.next_chunk(&AtomicBool::new(true)).is_err());
    assert!(p.next_chunk(&AtomicBool::new(false)).is_err());
}
#[test]
fn changed_axes_metadata_and_absent_shards_invalidate() {
    let f = fixture(&[(0, 0)]);
    let mut p = alphaearth::prepare(request(&f, [0, 0], [1, 1])).unwrap();
    fs::write(f.root.join("x/c/0"), [0; 8]).unwrap();
    assert!(p.next_chunk(&AtomicBool::new(false)).is_err());
    assert!(p.next_chunk(&AtomicBool::new(false)).is_err());
    let f = fixture(&[]);
    let mut p = alphaearth::prepare(request(&f, [0, 0], [1, 1])).unwrap();
    fs::create_dir_all(f.root.join("embeddings/c/0/0/0")).unwrap();
    fs::write(f.root.join("embeddings/c/0/0/0/0"), [1, 2, 3]).unwrap();
    assert!(p.next_chunk(&AtomicBool::new(false)).is_err());
}
#[test]
fn inconsistent_coordinate_and_unqualified_profile_rejected() {
    let f = fixture(&[(0, 0)]);
    let mut bytes = fs::read(f.root.join("x/c/0")).unwrap();
    bytes[..8].copy_from_slice(&1f64.to_le_bytes());
    fs::write(f.root.join("x/c/0"), bytes).unwrap();
    assert!(alphaearth::prepare(request(&f, [0, 0], [1, 1])).is_err());
    let f = fixture(&[]);
    let path = f.root.join("embeddings/zarr.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["dimension_names"] = json!(["time", "embedding", "y", "x"]);
    write_json(path, &m);
    assert!(alphaearth::prepare(request(&f, [0, 0], [1, 1])).is_err());
}
#[test]
fn truncated_corrupt_indices_and_illegal_offsets_rejected() {
    for mode in 0..6 {
        let f = fixture(&[(0, 0)]);
        let path = f.root.join("embeddings/c/0/0/0/0");
        let mut bytes = fs::read(&path).unwrap();
        if mode == 0 {
            bytes.truncate(10);
        } else if mode == 1 {
            let n = bytes.len();
            bytes[n - 1] ^= 1;
        } else {
            let n = bytes.len();
            let start = n - 4100;
            match mode {
                2 => bytes[start..start + 8].copy_from_slice(&u64::MAX.to_le_bytes()),
                3 => bytes[start..start + 8].copy_from_slice(&(start as u64).to_le_bytes()),
                4 => {
                    let pair = bytes[start..start + 16].to_vec();
                    bytes[start + 16..start + 32].copy_from_slice(&pair);
                }
                _ => bytes[start + 8..start + 16]
                    .copy_from_slice(&u64::MAX.saturating_sub(1).to_le_bytes()),
            }
            let crc = crc32c(&bytes[start..n - 4]);
            bytes[n - 4..].copy_from_slice(&crc.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
        assert!(alphaearth::prepare(request(&f, [0, 0], [1, 1])).is_err());
    }
}

#[test]
fn malformed_payload_and_zstd_size_bombs_fail_terminally() {
    for bomb in [false, true] {
        let f = fixture(&[(0, 0)]);
        let path = f.root.join("embeddings/c/0/0/0/0");
        let old = fs::read(&path).unwrap();
        let mut index = old[old.len() - 4100..old.len() - 4].to_vec();
        let mut payload = if bomb {
            zstd::bulk::compress(&vec![0u8; 8 * 1024 * 1024], 3).unwrap()
        } else {
            vec![1, 2, 3]
        };
        index[8..16].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        payload.extend(&index);
        payload.extend(crc32c(&index).to_le_bytes());
        fs::write(path, payload).unwrap();
        let mut p = alphaearth::prepare(request(&f, [0, 0], [1, 1])).unwrap();
        assert!(p.next_chunk(&AtomicBool::new(false)).is_err());
        assert!(p.next_chunk(&AtomicBool::new(false)).is_err());
    }
}

#[test]
fn changed_metadata_prevents_validated_completion() {
    let f = fixture(&[(0, 0)]);
    let mut p = alphaearth::prepare(request(&f, [0, 0], [1, 1])).unwrap();
    assert!(p.next_chunk(&AtomicBool::new(false)).unwrap().is_some());
    let path = f.root.join("zarr.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b' ');
    fs::write(path, bytes).unwrap();
    assert!(p.next_chunk(&AtomicBool::new(false)).is_err());
    assert_eq!(p.report()["complete"], false);
}

#[test]
fn inconsistent_spatial_bbox_rejected() {
    let f = fixture(&[]);
    let path = f.root.join("zarr.json");
    let mut metadata: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    metadata["attributes"]["spatial:bbox"][2] = json!(-90.);
    write_json(path, &metadata);
    assert!(alphaearth::prepare(request(&f, [0, 0], [1, 1])).is_err());
}
