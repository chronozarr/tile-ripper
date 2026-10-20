#![cfg(feature = "gdal")]
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
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

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tile-ripper-stream-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn cog_fixture() -> Fixture {
    let root = root();
    let mut assets = Vec::new();
    for (id, stamp, start, x_origin) in [
        ("a", "2025-01-01T00:00:00Z", 65u16, 0.),
        ("b", "2025-01-02T00:00:00Z", 165u16, 20.),
    ] {
        let source = root.join(format!("{id}.tif"));
        let mut ds = gdal::DriverManager::get_driver_by_name("GTiff")
            .unwrap()
            .create_with_band_type::<u16, _>(&source, 8, 8, 1)
            .unwrap();
        ds.set_geo_transform(&[x_origin, 10., 0., 80., 0., -10.])
            .unwrap();
        ds.set_spatial_ref(&gdal::spatial_ref::SpatialRef::from_epsg(32617).unwrap())
            .unwrap();
        let mut band = ds.rasterband(1).unwrap();
        band.set_no_data_value(Some(0.)).unwrap();
        band.set_scale(0.1).unwrap();
        band.set_offset(-1.).unwrap();
        let values = (0..64)
            .map(|i| {
                if i / 8 >= 4 && i % 8 >= 4 {
                    0
                } else {
                    start + i
                }
            })
            .collect();
        band.write(
            (0, 0),
            (8, 8),
            &mut gdal::raster::Buffer::new((8, 8), values),
        )
        .unwrap();
        assets.push(Asset {
            id: id.into(),
            locator: source.to_string_lossy().into(),
            format: Format::Cog,
            datetime: Some(stamp.into()),
            etag: None,
        });
    }
    Fixture {
        root: root.clone(),
        request: Request {
            source: Source::Assets { assets },
            spatial: Spatial::Bounds {
                crs: "EPSG:32617".into(),
                bbox: [0., 0., 80., 80.],
            },
            time: None,
            variables: vec![Variable::Band { index: 1 }],
            quality: vec![],
            grid: Grid::Native,
            operations: vec![],
            sink: Sink::Callback,
            budget: Budget {
                buffer_bytes: 4096,
                max_nodes: 2000,
                chunk_edge: 2,
            },
        },
    }
}

fn zarr_fixture() -> Fixture {
    let root = root();
    let source = root.join("source.zarr");
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
            vec![1, 1, 1],
            vec!["time", "y", "x"],
        ),
    ] {
        fs::create_dir(source.join(name)).unwrap();
        let meta = serde_json::json!({"zarr_format":2,"shape":shape,"chunks":chunks,"dtype":dtype,"order":"C","filters":null,"compressor":null,"fill_value":0});
        fs::write(
            source.join(name).join(".zarray"),
            serde_json::to_vec(&meta).unwrap(),
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
        let array = Array::open(store.clone(), &format!("/{name}")).unwrap();
        match name {
            "x" => array
                .store_array_subset(
                    &array.subset_all(),
                    (0..8).map(|i| i as f64 * 10. + 5.).collect::<Vec<_>>(),
                )
                .unwrap(),
            "y" => array
                .store_array_subset(
                    &array.subset_all(),
                    (0..8).map(|i| 75. - i as f64 * 10.).collect::<Vec<_>>(),
                )
                .unwrap(),
            "time" => array
                .store_array_subset(
                    &array.subset_all(),
                    &[1735689600i64, 1735776000, 1735948800],
                )
                .unwrap(),
            _ => array
                .store_array_subset(
                    &array.subset_all(),
                    (1..=192).map(|i| i as u16).collect::<Vec<_>>(),
                )
                .unwrap(),
        }
    }
    Fixture {
        root: root.clone(),
        request: Request {
            source: Source::Assets {
                assets: vec![Asset {
                    id: "z".into(),
                    locator: source.to_string_lossy().into(),
                    format: Format::ZarrV2,
                    datetime: None,
                    etag: None,
                }],
            },
            spatial: Spatial::Bounds {
                crs: "EPSG:32617".into(),
                bbox: [0., 0., 80., 80.],
            },
            time: None,
            variables: vec![Variable::Dataset { path: "red".into() }],
            quality: vec![],
            grid: Grid::Native,
            operations: vec![],
            sink: Sink::Callback,
            budget: Budget {
                buffer_bytes: 4096,
                max_nodes: 2000,
                chunk_edge: 2,
            },
        },
    }
}

#[test]
fn stream_preserves_deterministic_order_and_native_provenance() {
    let f = cog_fixture();
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let expected = prepared.execute(|_| Ok(())).unwrap();
    let stream = prepared
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 1 })
        .unwrap();
    let mut actual = Vec::new();
    for item in stream {
        let c = item.unwrap();
        actual.push((
            c.node_id, c.asset_id, c.datetime, c.window, c.nodata, c.scale, c.offset,
        ));
    }
    assert_eq!(actual.len(), expected.chunks);
    assert!(actual.windows(2).all(|w| w[0].0 < w[1].0));
    assert!(actual.iter().all(|c| c.1 == "a" || c.1 == "b"));
    assert!(actual
        .iter()
        .all(|c| c.4 == Some(0.) && c.5 == Some(0.1) && c.6 == Some(-1.)));
}

#[test]
fn cog_selection_reuses_reader_with_one_window_budget_and_cancels_between_windows() {
    let mut f = cog_fixture();
    f.request.budget.buffer_bytes = 8; // exactly one 2x2 UInt16 output
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let mut ids = Vec::new();
    let report = prepared
        .execute(|c| {
            ids.push(c.node_id);
            Ok(())
        })
        .unwrap();
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(report.scheduler["admitted_jobs"], 2);
    assert_eq!(report.scheduler["completed_tasks"], report.chunks);
    assert!(report.scheduler["reserved_bytes_peak"].as_u64().unwrap() <= 8);
    let mut delivered = 0;
    assert!(prepared
        .execute(|_| {
            delivered += 1;
            Err("cancel after one window".into())
        })
        .is_err());
    assert_eq!(delivered, 1);
    let rerun = prepared.execute(|_| Ok(())).unwrap();
    assert_eq!(rerun.chunks, report.chunks);
}

#[test]
fn zarr_stream_delivers_each_selected_source_chunk_once_with_axes_and_values() {
    let f = zarr_fixture();
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let stream = prepared
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 1 })
        .unwrap();
    let mut chunks = 0;
    for item in stream {
        let c = item.unwrap();
        assert_eq!(c.scale, Some(0.1));
        assert_eq!(c.offset, Some(-1.));
        assert_eq!(c.nodata, Some(0.));
        assert!(c.datetime.is_some());
        assert!(c.indices.len() == 1);
        assert_eq!(
            c.pixels.as_u16().unwrap().len(),
            c.window.width * c.window.height
        );
        chunks += 1;
    }
    assert_eq!(chunks, 192);
}

#[test]
fn stream_completion_reports_source_reads_and_scheduler_bound() {
    let f = zarr_fixture();
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let stream = prepared
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 1 })
        .unwrap();
    let report = stream.finish().unwrap();
    assert_eq!(report.source_reads.len(), 192);
    assert!(report.source_reads.iter().all(|r| r.requests == 1));
    assert_eq!(report.scheduler["completed_tasks"], report.chunks);
    assert!(
        report.scheduler["reserved_bytes_peak"].as_u64().unwrap()
            <= report.scheduler["buffer_bytes"].as_u64().unwrap()
    );
}

#[test]
fn zero_capacity_rendezvous_stream_and_invalid_capacity() {
    let f = cog_fixture();
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let invalid = prepared.into_stream(tile_ripper::stream::StreamOptions { capacity: 65 });
    assert!(invalid.is_err());

    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let mut stream = prepared
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 0 })
        .unwrap();
    assert!(stream.next().unwrap().is_ok());
    let report = stream.finish().unwrap();
    assert!(report.chunks > 0);
}

#[test]
fn callback_error_cancels_after_one_delivered_chunk_and_drop_joins() {
    let f = cog_fixture();
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let mut calls = 0;
    assert!(prepared
        .execute(|_| {
            calls += 1;
            Err("consumer stopped".into())
        })
        .is_err());
    assert_eq!(calls, 1);

    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let stream = prepared
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 1 })
        .unwrap();
    let mut iter = stream;
    let first = iter.next().unwrap().unwrap();
    assert!(!first.pixels.is_empty());
    iter.cancel();
    assert!(iter.finish().is_err());
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let stream = prepared
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 1 })
        .unwrap();
    drop(stream);
}

#[test]
fn cancelled_callback_can_rerun_the_same_prepared_execution() {
    let f = cog_fixture();
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();

    let mut callbacks = 0;
    assert!(prepared
        .execute(|_| {
            callbacks += 1;
            Err("cancel this attempt".into())
        })
        .is_err());
    assert_eq!(callbacks, 1);

    let mut chunks = Vec::new();
    let report = prepared
        .execute(|c| {
            chunks.push((
                c.node_id,
                c.asset_id.clone(),
                c.datetime.clone(),
                c.variable.clone(),
                c.indices.clone(),
                c.window,
                c.transform,
                c.crs.clone(),
                c.nodata,
                c.scale,
                c.offset,
                c.pixels.as_u16().unwrap().to_vec(),
            ));
            Ok(())
        })
        .unwrap();

    assert_eq!(chunks.len(), report.chunks);
    assert!(chunks.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert!(chunks.iter().any(|chunk| chunk.1 == "a"));
    assert!(chunks.iter().any(|chunk| chunk.1 == "b"));
    assert!(chunks
        .iter()
        .all(|chunk| { chunk.8 == Some(0.) && chunk.9 == Some(0.1) && chunk.10 == Some(-1.) }));

    let control = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let mut control_chunks = Vec::new();
    let control_report = control
        .execute(|c| {
            control_chunks.push((
                c.node_id,
                c.asset_id.clone(),
                c.datetime.clone(),
                c.variable.clone(),
                c.indices.clone(),
                c.window,
                c.transform,
                c.crs.clone(),
                c.nodata,
                c.scale,
                c.offset,
                c.pixels.as_u16().unwrap().to_vec(),
            ));
            Ok(())
        })
        .unwrap();
    assert_eq!(chunks, control_chunks);
    assert_eq!(report.chunks, control_report.chunks);
    assert_eq!(report.decoded_bytes, control_report.decoded_bytes);
    assert_eq!(
        report.decoded_source_chunks,
        control_report.decoded_source_chunks
    );
    assert_eq!(
        report.filled_source_chunks,
        control_report.filled_source_chunks
    );
    assert_eq!(report.non_nodata_pixels, control_report.non_nodata_pixels);
    assert_eq!(report.transport, control_report.transport);
    assert_eq!(
        report.source_data_requests,
        control_report.source_data_requests
    );
    assert_eq!(report.source_data_bytes, control_report.source_data_bytes);
    let reads = |r: &ExecutionReport| {
        r.source_reads
            .iter()
            .map(|v| (v.asset_id.clone(), v.key.clone(), v.requests, v.bytes))
            .collect::<Vec<_>>()
    };
    assert_eq!(reads(&report), reads(&control_report));
}

#[test]
fn source_mutation_is_an_error_from_finish_instead_of_clean_eof() {
    let f = zarr_fixture();
    let prepared = acquisition::prepare(&f.request, &mut LayoutCache::default()).unwrap();
    let stream = prepared
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 1 })
        .unwrap();
    fs::write(f.root.join("source.zarr/.zattrs"), b"changed").unwrap();
    assert!(stream.finish().is_err());
}

#[test]
fn slow_consumer_keeps_scheduler_reservations_bounded() {
    let f = zarr_fixture();
    let stream = acquisition::prepare(&f.request, &mut LayoutCache::default())
        .unwrap()
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 1 })
        .unwrap();
    let mut stream = stream;
    for item in stream.by_ref() {
        item.unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    let report = stream.finish().unwrap();
    assert!(
        report.scheduler["reserved_bytes_peak"].as_u64().unwrap()
            <= report.scheduler["buffer_bytes"].as_u64().unwrap()
    );
}

#[test]
fn mutation_after_delivery_invalidates_all_provisional_results() {
    let f = zarr_fixture();
    let mut stream = acquisition::prepare(&f.request, &mut LayoutCache::default())
        .unwrap()
        .into_stream(tile_ripper::stream::StreamOptions { capacity: 0 })
        .unwrap();
    let first = stream.next().unwrap().unwrap();
    assert!(!first.pixels.is_empty());
    // Backpressure keeps execution in progress after the first delivered plane.
    fs::write(
        f.root.join("source.zarr/.zattrs"),
        b"changed after delivery",
    )
    .unwrap();
    let mut saw_error = false;
    for item in stream.by_ref() {
        saw_error |= item.is_err();
    }
    assert!(saw_error);
    assert!(stream.next().is_none());
    assert!(stream.finish().is_err());
}

#[test]
fn observed_error_cannot_be_lost_before_finish() {
    let f = zarr_fixture();
    let mut stream = acquisition::prepare(&f.request, &mut LayoutCache::default())
        .unwrap()
        .into_stream(Default::default())
        .unwrap();
    stream.cancel();
    assert!(stream.next().unwrap().is_err());
    assert!(stream.next().is_none());
    assert!(stream.finish().is_err());
}

#[test]
fn cancel_after_all_delivered_chunks_still_invalidates_finish() {
    let f = cog_fixture();
    let mut stream = acquisition::prepare(&f.request, &mut LayoutCache::default())
        .unwrap()
        .into_stream(Default::default())
        .unwrap();
    for item in stream.by_ref() {
        item.unwrap();
    }
    stream.cancel();
    assert!(stream.finish().is_err());
}
