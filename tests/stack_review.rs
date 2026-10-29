//! Independent regressions for the published-observation stack boundary.
//!
//! These fixtures use the two source identity schemas the acquisition writers
//! actually emit, rather than the old stack unit fixture's synthetic S2 schema.
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};
use tile_ripper::stack;
use zarrs::{array::Array, group::Group};
use zarrs_filesystem::FilesystemStore;

const HEIGHT: u64 = 3;
const WIDTH: u64 = 4;
const TRANSFORM: [f64; 6] = [500000., 10., 0., 9100000., 0., -10.];
const DATES: [&str; 3] = [
    "2024-01-03T15:10:00Z",
    "2024-01-08T15:10:00Z",
    "2024-01-13T15:10:00Z",
];

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "tile-ripper-stack-review-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }
    fn input(&self) -> PathBuf {
        self.0.join("input.zarr")
    }
    fn output(&self) -> PathBuf {
        self.0.join("output.zarr")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn metadata(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn edit_json(path: impl AsRef<Path>, edit: impl FnOnce(&mut Value)) {
    let path = path.as_ref();
    let mut document = metadata(path);
    edit(&mut document);
    fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
}

fn group(store: &Arc<FilesystemStore>, path: &str, attrs: Value) {
    let mut group = Group::new_with_metadata(
        store.clone(),
        path,
        serde_json::from_value(json!({"zarr_format": 2})).unwrap(),
    )
    .unwrap();
    *group.attributes_mut() = serde_json::from_value(attrs).unwrap();
    group.store_metadata().unwrap();
}

fn array(
    store: &Arc<FilesystemStore>,
    path: &str,
    shape: &[u64],
    dtype: &str,
    fill: Value,
    attrs: Value,
) -> Array<FilesystemStore> {
    let mut result = Array::new_with_metadata(
        store.clone(),
        path,
        serde_json::from_value(json!({
            "zarr_format": 2,
            "shape": shape,
            "chunks": shape,
            "dtype": dtype,
            "order": "C",
            "filters": null,
            "compressor": null,
            "fill_value": fill
        }))
        .unwrap(),
    )
    .unwrap();
    *result.attributes_mut() = serde_json::from_value(attrs).unwrap();
    result.store_metadata().unwrap();
    result
}

fn add_observation(
    store: &Arc<FilesystemStore>,
    records: &mut Vec<Value>,
    day: usize,
    band: &str,
    earth_search: bool,
) {
    let i = records.len();
    let base = format!("/observations/{i:06}");
    let asset = format!("asset-{i:04}");
    let (scale, offset) = if earth_search {
        (0.0001, -0.1)
    } else {
        (0.0000275, -0.2)
    };
    let mut source = json!({
        "asset_id": asset,
        "item_id": format!("{}-{day}", if earth_search { "S2A_17SPV" } else { "LC09" }),
        "requested_band": band,
        "collection": if earth_search { "sentinel-2-c1-l2a" } else { "landsat-c2-l2" },
        "href": format!("https://example.invalid/{day}/{band}.tif"),
        "datetime": DATES[day],
        "raster_bands": [{"scale": scale, "offset": offset, "unit": "1"}],
        "acquisition": {"platform": if earth_search { "sentinel-2a" } else { "landsat-9" }}
    });
    if earth_search {
        source["acquisition"]["s2:processing_baseline"] = json!("05.10");
        source["object_identity"] = json!({"etag": format!("\"etag-{i}\""), "bytes": 100000 + i});
    } else {
        source["object_version"] = json!(format!("\"etag-{i}\""));
        source["product_version"] = json!("2.0");
    }
    group(
        store,
        &base,
        json!({
            "asset_id": asset,
            "variable": "band:1",
            "datetime": DATES[day],
            "source": source,
            "transform": TRANSFORM,
            "crs": "EPSG:32718",
            "source_nodata": 0.0,
            "aoi_mask_applied": true,
            "time_missing": false
        }),
    );
    let values = array(
        store,
        &format!("{base}/values"),
        &[1, HEIGHT, WIDTH],
        "<u2",
        json!(0),
        json!({
            "_ARRAY_DIMENSIONS": ["time", "y", "x"],
            "grid_mapping": "spatial_ref",
            "ancillary_variables": "valid aoi_mask",
            "source_nodata": 0.0,
            "scale_factor": scale,
            "add_offset": offset,
            "units": "1"
        }),
    );
    let samples: Vec<u16> = (0..HEIGHT * WIDTH)
        .map(|p| (100 * i as u64 + p) as u16)
        .collect();
    values
        .store_array_subset(&values.subset_all(), &samples)
        .unwrap();
    for name in ["valid", "aoi_mask"] {
        let mask = array(
            store,
            &format!("{base}/{name}"),
            &[1, HEIGHT, WIDTH],
            "|u1",
            Value::Null,
            json!({"_ARRAY_DIMENSIONS": ["time", "y", "x"], "grid_mapping": "spatial_ref"}),
        );
        let mut cells = vec![1u8; (HEIGHT * WIDTH) as usize];
        if name == "valid" {
            cells[i % 2] = 0;
        }
        mask.store_array_subset(&mask.subset_all(), &cells).unwrap();
    }
    for (axis, length, origin, step) in [
        ("x", WIDTH, TRANSFORM[0], TRANSFORM[1]),
        ("y", HEIGHT, TRANSFORM[3], TRANSFORM[5]),
    ] {
        let coordinate = array(
            store,
            &format!("{base}/{axis}"),
            &[length],
            "<f8",
            Value::Null,
            json!({"_ARRAY_DIMENSIONS": [axis], "axis": axis.to_ascii_uppercase(), "units": "m"}),
        );
        let centres: Vec<f64> = (0..length)
            .map(|p| origin + (p as f64 + 0.5) * step)
            .collect();
        coordinate
            .store_array_subset(&coordinate.subset_all(), &centres)
            .unwrap();
    }
    let time = array(
        store,
        &format!("{base}/time"),
        &[1],
        "<i8",
        Value::Null,
        json!({
            "_ARRAY_DIMENSIONS": ["time"],
            "standard_name": "time",
            "units": "nanoseconds since 1970-01-01 00:00:00",
            "calendar": "proleptic_gregorian"
        }),
    );
    let timestamp = chrono::DateTime::parse_from_rfc3339(DATES[day])
        .unwrap()
        .timestamp_nanos_opt()
        .unwrap();
    time.store_array_subset(&time.subset_all(), &[timestamp])
        .unwrap();
    let reference = array(
        store,
        &format!("{base}/spatial_ref"),
        &[],
        "<i4",
        Value::Null,
        json!({
            "_ARRAY_DIMENSIONS": [],
            "crs": "EPSG:32718",
            "GeoTransform": "500000 10 0 9100000 0 -10"
        }),
    );
    reference
        .store_array_subset(&reference.subset_all(), &[0i32])
        .unwrap();
    records.push(json!({
        "file": ".",
        "group": base.trim_start_matches('/'),
        "dataset": "values",
        "asset_id": asset,
        "variable": "band:1",
        "datetime": DATES[day],
        "dtype": "uint16",
        "native_dtype": "<u2",
        "nodata": 0.0,
        "native_grid": {"crs": "EPSG:32718", "transform": TRANSFORM},
        "dimensions": {"names": ["time", "y", "x"], "shape": [1, HEIGHT, WIDTH]},
        "aoi_mask_applied": true
    }));
}

fn fixture(input: &Path, days: usize) {
    fs::create_dir(input).unwrap();
    let store = Arc::new(FilesystemStore::new(input).unwrap());
    group(
        &store,
        "/",
        json!({"status": "complete", "title": "Native source observations"}),
    );
    group(&store, "/observations", json!({}));
    let mut records = Vec::new();
    for day in 0..days {
        for band in ["B04", "B08"] {
            add_observation(&store, &mut records, day, band, true);
        }
    }
    fs::write(
        input.join("manifest.json"),
        serde_json::to_vec(&json!({
            "schema_version": 3,
            "document_type": "acquisition_result",
            "status": "complete",
            "format": "zarr_v2",
            "outputs": records
        }))
        .unwrap(),
    )
    .unwrap();
}

fn tree_bytes(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, at: &Path, result: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                result.insert(relative, None);
                walk(root, &path, result);
            } else {
                result.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

fn write(input: &Path, output: &Path) -> tile_ripper::Result<Value> {
    stack::write(input, output, 2, Arc::new(AtomicBool::new(false)))
}

#[test]
fn earth_search_pin_and_processing_metadata_survive_public_stack() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    write(&temp.input(), &temp.output()).unwrap();
    let output = metadata(temp.output().join("stacks/0/zarr.json"));
    let first = &output["attributes"]["observations"][0];
    assert_eq!(first["object_version"], "\"etag-0\"");
    assert_eq!(first["source"]["object_identity"]["bytes"], 100000);
    assert_eq!(
        first["source"]["acquisition"]["s2:processing_baseline"],
        "05.10"
    );
}

#[test]
fn successful_publication_marks_root_and_manifest_complete() {
    let temp = Temp::new();
    fixture(&temp.input(), 1);
    let result = write(&temp.input(), &temp.output()).unwrap();
    assert_eq!(result["status"], "complete");
    assert_eq!(
        metadata(temp.output().join("manifest.json"))["status"],
        "complete"
    );
    assert_eq!(
        metadata(temp.output().join("zarr.json"))["attributes"]["status"],
        "complete"
    );
}

#[test]
fn successful_stack_preserves_every_input_file_byte_and_directory() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let before = tree_bytes(&temp.input());
    write(&temp.input(), &temp.output()).unwrap();
    assert_eq!(tree_bytes(&temp.input()), before);
}

#[test]
fn missing_source_is_an_error_with_group_context_and_never_a_panic() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    edit_json(temp.input().join("observations/000000/.zattrs"), |attrs| {
        attrs.as_object_mut().unwrap().remove("source");
    });
    let result = std::panic::catch_unwind(|| stack::plan(&temp.input()));
    let error = result
        .expect("missing source must never panic")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("observations/000000") && error.contains("source"),
        "{error}"
    );
}

#[test]
fn unknown_scene_identity_cannot_be_used_to_merge_bands() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    edit_json(temp.input().join("observations/000000/.zattrs"), |attrs| {
        attrs["source"].as_object_mut().unwrap().remove("item_id");
    });
    let error = stack::plan(&temp.input()).unwrap_err().to_string();
    assert!(
        error.contains("observations/000000") && error.contains("item_id"),
        "{error}"
    );
}

#[test]
fn conflicting_source_pins_fail_instead_of_choosing_one() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    edit_json(temp.input().join("observations/000000/.zattrs"), |attrs| {
        attrs["source"]["object_version"] = json!("\"different-object\"");
    });
    let error = stack::plan(&temp.input()).unwrap_err().to_string();
    assert!(
        error.contains("observations/000000")
            && (error.contains("identity") || error.contains("pin") || error.contains("version")),
        "{error}"
    );
}

#[test]
fn manifest_datetime_cannot_relabel_pinned_source_samples() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    edit_json(temp.input().join("manifest.json"), |manifest| {
        manifest["outputs"][0]["datetime"] = json!("2024-01-02T15:10:00Z");
    });
    let error = stack::plan(&temp.input()).unwrap_err().to_string();
    assert!(
        error.contains("observations/000000")
            && (error.contains("datetime") || error.contains("time")),
        "{error}"
    );
}

#[test]
fn stored_time_coordinate_must_match_the_observation_datetime() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let store = Arc::new(FilesystemStore::new(temp.input()).unwrap());
    let time = Array::open(store, "/observations/000000/time").unwrap();
    time.store_array_subset(&time.subset_all(), &[0i64])
        .unwrap();
    let error = stack::plan(&temp.input()).unwrap_err().to_string();
    assert!(
        error.contains("observations/000000") && error.contains("time"),
        "{error}"
    );
}

#[test]
fn source_asset_identity_must_agree_with_the_output_record() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    edit_json(temp.input().join("observations/000000/.zattrs"), |attrs| {
        attrs["source"]["asset_id"] = json!("different-asset");
    });
    let error = stack::plan(&temp.input()).unwrap_err().to_string();
    assert!(
        error.contains("observations/000000") && error.contains("asset"),
        "{error}"
    );
}

#[test]
fn manifest_dtype_cannot_reinterpret_equal_width_stored_values() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    edit_json(temp.input().join("manifest.json"), |manifest| {
        manifest["outputs"][0]["dtype"] = json!("int16");
    });
    let error = stack::plan(&temp.input()).unwrap_err().to_string();
    assert!(
        error.contains("observations/000000") && error.contains("dtype"),
        "{error}"
    );
}

#[test]
fn manifest_shape_cannot_silently_crop_the_observation() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    edit_json(temp.input().join("manifest.json"), |manifest| {
        manifest["outputs"][0]["dimensions"]["shape"] = json!([1, HEIGHT - 1, WIDTH]);
    });
    let error = stack::plan(&temp.input()).unwrap_err().to_string();
    assert!(
        error.contains("observations/000000") && error.contains("shape"),
        "{error}"
    );
}

#[test]
fn coordinate_centres_must_agree_with_manifest_geometry() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let store = Arc::new(FilesystemStore::new(temp.input()).unwrap());
    let x = Array::open(store, "/observations/000000/x").unwrap();
    x.store_array_subset(&x.subset_all(), &[500006f64, 500016., 500026., 500036.])
        .unwrap();
    let error = stack::plan(&temp.input()).unwrap_err().to_string();
    assert!(
        error.contains("observations/000000") && error.contains('x'),
        "{error}"
    );
}

#[test]
fn source_masks_must_be_uint8_on_the_declared_plane() {
    for (name, replacement) in [("valid", json!("|i1")), ("aoi_mask", json!("<u2"))] {
        let temp = Temp::new();
        fixture(&temp.input(), 2);
        edit_json(
            temp.input()
                .join(format!("observations/000000/{name}/.zarray")),
            |array| {
                array["dtype"] = replacement;
            },
        );
        let error = stack::plan(&temp.input()).unwrap_err().to_string();
        assert!(
            error.contains("observations/000000") && error.contains(name),
            "{error}"
        );
    }
}

#[test]
fn interleaved_calibration_reports_both_real_chronological_boundaries() {
    let temp = Temp::new();
    fixture(&temp.input(), 3);
    for i in [2, 3] {
        edit_json(
            temp.input()
                .join(format!("observations/{i:06}/values/.zattrs")),
            |attrs| {
                attrs["add_offset"] = json!(0.0);
            },
        );
        edit_json(
            temp.input().join(format!("observations/{i:06}/.zattrs")),
            |attrs| {
                attrs["source"]["raster_bands"][0]["offset"] = json!(0.0);
            },
        );
    }
    let summary = stack::plan(&temp.input()).unwrap();
    let breaks = summary["calibration_breaks"].as_array().unwrap();
    let actual: Vec<_> = breaks
        .iter()
        .map(|change| {
            (
                change["before"]["last"].as_str().unwrap(),
                change["after"]["first"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(actual, [(DATES[0], DATES[1]), (DATES[1], DATES[2])]);
}

#[test]
fn output_inside_input_is_rejected_before_creating_any_directory() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let before = tree_bytes(&temp.input());
    let result = write(&temp.input(), &temp.input().join("derived/output.zarr"));
    assert!(result.is_err(), "output must not mutate the input tree");
    assert_eq!(tree_bytes(&temp.input()), before);
}

#[cfg(unix)]
#[test]
fn symlink_parent_cannot_hide_an_output_inside_the_input() {
    let temp = Temp::new();
    fixture(&temp.input(), 1);
    let before = tree_bytes(&temp.input());
    let alias = temp.0.join("input-alias");
    std::os::unix::fs::symlink(temp.input(), &alias).unwrap();
    let result = write(&temp.input(), &alias.join("derived/output.zarr"));
    assert!(
        result.is_err(),
        "canonical input aliases must remain read only"
    );
    assert_eq!(tree_bytes(&temp.input()), before);
}

#[test]
fn pre_cancelled_write_publishes_nothing_and_preserves_input() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let before = tree_bytes(&temp.input());
    let error = stack::write(
        &temp.input(),
        &temp.output(),
        2,
        Arc::new(AtomicBool::new(true)),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!temp.output().exists());
    assert_eq!(tree_bytes(&temp.input()), before);
}

#[test]
fn explicit_buffer_budget_refuses_work_before_publishing() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let error = stack::write_with_options(
        &temp.input(),
        &temp.output(),
        stack::Options {
            chunk_edge: 2,
            buffer_bytes: 1024 * 1024,
        },
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("buffer") || error.contains("reservation") || error.contains("budget"),
        "{error}"
    );
    assert!(!temp.output().exists());
}

#[test]
fn decoded_source_chunk_size_is_admitted_even_for_a_tiny_selection() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    edit_json(
        temp.input().join("observations/000000/values/.zarray"),
        |array| {
            array["chunks"] = json!([1, 8192, 8192]);
        },
    );
    // An absent all-fill chunk is valid Zarr. Decoding that chunk must still
    // fit the reservation, even though only twelve selected cells are needed.
    fs::remove_file(temp.input().join("observations/000000/values/0.0.0")).unwrap();
    let error = write(&temp.input(), &temp.output())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("buffer") || error.contains("reservation") || error.contains("budget"),
        "{error}"
    );
    assert!(!temp.output().exists());
}

#[test]
fn oversized_encoded_source_file_is_rejected_before_decoding() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let directory = temp.input().join("observations/000000/values");
    edit_json(directory.join(".zarray"), |array| {
        array["compressor"] = json!({"id": "zstd", "level": 1});
    });
    // Sparse extension avoids allocating a giant test buffer. A damaged source
    // must be refused by its encoded-byte admission before the codec sees it.
    fs::OpenOptions::new()
        .write(true)
        .open(directory.join("0.0.0"))
        .unwrap()
        .set_len(stack::DEFAULT_BUFFER_BYTES * 2)
        .unwrap();
    let error = write(&temp.input(), &temp.output())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("observations/000000/values")
            && error.contains("encoded")
            && (error.contains("buffer")
                || error.contains("reservation")
                || error.contains("budget")),
        "{error}"
    );
    assert!(!temp.output().exists());
}

#[test]
fn compressed_frame_cannot_expand_beyond_its_validated_chunk() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let directory = temp.input().join("observations/000000/values");
    edit_json(directory.join(".zarray"), |array| {
        array["compressor"] = json!({"id": "zstd", "level": 1});
    });
    // Metadata admits 24 decoded bytes. The tiny frame actually declares 4096;
    // the fixed-output wrapper must reject it before growable decoding.
    let frame = zstd::bulk::compress(&[0u8; 4096], 1).unwrap();
    fs::write(directory.join("0.0.0"), frame).unwrap();
    let error = write(&temp.input(), &temp.output())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("observations/000000/values") && error.contains("validated chunk size"),
        "{error}"
    );
    assert!(!temp.output().exists());
}

#[test]
fn gzip_cannot_expand_beyond_its_validated_chunk() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let directory = temp.input().join("observations/000000/values");
    edit_json(directory.join(".zarray"), |array| {
        array["compressor"] = json!({"id": "gzip", "level": 1});
    });
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(&[0u8; 4096]).unwrap();
    fs::write(directory.join("0.0.0"), gzip.finish().unwrap()).unwrap();
    let error = write(&temp.input(), &temp.output())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("observations/000000/values") && error.contains("validated chunk size"),
        "{error}"
    );
    assert!(!temp.output().exists());
}

fn blosc_values(input: &Path) {
    let directory = input.join("observations/000000/values");
    edit_json(directory.join(".zarray"), |array| {
        array["compressor"] =
            json!({"id": "blosc", "cname": "zstd", "clevel": 1, "shuffle": 1, "blocksize": 0});
    });
    let store = Arc::new(FilesystemStore::new(input).unwrap());
    let values = Array::open(store, "/observations/000000/values").unwrap();
    values
        .store_chunk(
            &[0, 0, 0],
            (0..HEIGHT * WIDTH).map(|n| n as u16).collect::<Vec<_>>(),
        )
        .unwrap();
}

#[test]
fn blosc_header_cannot_advertise_more_than_the_validated_chunk() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    blosc_values(&temp.input());
    let chunk = temp.input().join("observations/000000/values/0.0.0");
    let mut encoded = fs::read(&chunk).unwrap();
    encoded[4..8].copy_from_slice(&4096u32.to_le_bytes());
    fs::write(chunk, encoded).unwrap();
    let error = write(&temp.input(), &temp.output())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("observations/000000/values")
            && error.contains("Blosc header disagrees with validated chunk size/type"),
        "{error}"
    );
    assert!(!temp.output().exists());
}

#[test]
fn admitted_compression_keeps_exact_native_values() {
    for codec in ["zstd", "gzip", "blosc"] {
        let temp = Temp::new();
        fixture(&temp.input(), 2);
        let directory = temp.input().join("observations/000000/values");
        if codec == "blosc" {
            blosc_values(&temp.input());
        } else {
            edit_json(directory.join(".zarray"), |array| {
                array["compressor"] = json!({"id": codec, "level": 1});
            });
            let bytes = fs::read(directory.join("0.0.0")).unwrap();
            let frame = if codec == "zstd" {
                zstd::bulk::compress(&bytes, 1).unwrap()
            } else {
                let mut gzip =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                gzip.write_all(&bytes).unwrap();
                gzip.finish().unwrap()
            };
            fs::write(directory.join("0.0.0"), frame).unwrap();
        }
        write(&temp.input(), &temp.output()).unwrap();
        let store = Arc::new(FilesystemStore::new(temp.output()).unwrap());
        let values = Array::open(store, "/stacks/0/values").unwrap();
        let actual = values
            .retrieve_array_subset::<Vec<u16>>(&values.subset_all())
            .unwrap();
        let expected: Vec<u16> = (0..4)
            .flat_map(|i| (0..HEIGHT * WIDTH).map(move |p| (100 * i + p) as u16))
            .collect();
        assert_eq!(actual, expected, "{codec}");
    }
}

#[test]
fn independent_product_stack_survives_an_aoi_refusal() {
    let temp = Temp::new();
    fixture(&temp.input(), 2);
    let store = Arc::new(FilesystemStore::new(temp.input()).unwrap());
    let aoi = Array::open(store.clone(), "/observations/000002/aoi_mask").unwrap();
    let mut changed = vec![1u8; (HEIGHT * WIDTH) as usize];
    changed[0] = 0;
    aoi.store_array_subset(&aoi.subset_all(), &changed).unwrap();
    let mut manifest = metadata(temp.input().join("manifest.json"));
    let mut records = manifest["outputs"].as_array().unwrap().clone();
    add_observation(&store, &mut records, 0, "SR_B4", false);
    manifest["outputs"] = json!(records);
    fs::write(
        temp.input().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let written = write(&temp.input(), &temp.output()).unwrap();
    assert_eq!(written["summary"]["stacks"].as_array().unwrap().len(), 1);
    assert_eq!(
        written["summary"]["stacks"][0]["collection"],
        "landsat-c2-l2"
    );
    assert_eq!(written["summary"]["refusals"].as_array().unwrap().len(), 1);
    let group = metadata(temp.output().join("stacks/0/zarr.json"));
    assert_eq!(
        group["attributes"]["observations"][0]["source"]["object_version"],
        "\"etag-4\""
    );
}
