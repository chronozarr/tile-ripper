//! Golden files are written by h5py/libhdf5, never by the Rust reader.
#[cfg(feature = "native-hdf5")]
use tile_ripper::stream::StreamOptions;
use tile_ripper::{acquisition, cog::LayoutCache, *};
fn request(path: &str) -> Request {
    serde_json::from_value(serde_json::json!({
        "source":{"kind":"assets","assets":[{"id":"sample","locator":path,"format":"hdf5","datetime":null}]},
        "spatial":{"kind":"array_slice","start":[1,2],"count":[3,3],"step":[3,4],"axes":["/science/y","/science/x"]},
        "time":null,"variables":[{"kind":"dataset","path":"/science/signal"}],"grid":{"kind":"native"},
        "sink":{"kind":"callback"},"budget":{"buffer_bytes":4194304,"max_nodes":1024,"chunk_edge":16}
    })).unwrap()
}
fn fixture(name: &str) -> String {
    format!("{}/tests/data/hdf5/{name}.h5", env!("CARGO_MANIFEST_DIR"))
}
#[cfg(feature = "native-hdf5")]
#[test]
fn strided_big_endian_values_coordinates_and_group_identity() -> Result<()> {
    let r = request(&fixture("v1-be"));
    let prepared = acquisition::prepare(&r, &mut LayoutCache::default())?;
    let mut stream = prepared.into_stream(StreamOptions { capacity: 0 })?;
    let mut values = std::collections::BTreeMap::new();
    for chunk in stream.by_ref() {
        let c = chunk?;
        assert_eq!(
            c.variable,
            Variable::Dataset {
                path: "/science/signal".into()
            }
        );
        assert_eq!(c.scale, Some(0.01));
        assert_eq!(c.offset, Some(-2.));
        assert!(c.crs.starts_with("HDF5:stored_axes:"));
        assert_eq!(c.transform[0] + c.transform[1] / 2., c.window.x as f64);
        assert_eq!(c.transform[3] + c.transform[5] / 2., c.window.y as f64);
        let (x, y) = c.coordinates.as_ref().unwrap();
        for (row, cy) in y.iter().enumerate() {
            for (col, cx) in x.iter().enumerate() {
                let source_row = ((cy - 1.) / 2.) as usize;
                let source_col = ((cx - 5.) / 3.) as usize;
                let value = c.pixels.as_u16().unwrap()[row * c.window.width + col];
                assert_eq!(
                    value,
                    if (3..6).contains(&source_row) && (4..8).contains(&source_col) {
                        65535
                    } else {
                        (source_row * 11 + source_col) as u16
                    }
                );
                assert_eq!(c.pixel_center(col, row), Some([*cx, *cy]));
                assert!(values.insert((source_row, source_col), value).is_none());
            }
        }
    }
    let report = stream.finish()?;
    assert_eq!(values.len(), 9);
    assert_eq!(report.decoded_bytes, 18);
    assert!(
        report.scheduler["reserved_bytes_peak"].as_u64().unwrap() <= r.budget.buffer_bytes as u64
    );
    Ok(())
}
#[cfg(feature = "native-hdf5")]
#[test]
fn unsupported_index_and_small_memory_fail_before_delivery() {
    let r = request(&fixture("unsupported-fixed"));
    let error = acquisition::prepare(&r, &mut LayoutCache::default())
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("v1 B-tree"), "{error}");
    let mut r = request(&fixture("v1-be"));
    r.budget.buffer_bytes = 4096;
    assert!(acquisition::prepare(&r, &mut LayoutCache::default()).is_err());
}
#[cfg(feature = "native-hdf5")]
#[test]
fn cancel_finish_and_repeat_prepared_stream() -> Result<()> {
    let r = request(&fixture("v1-be"));
    let p = acquisition::prepare(&r, &mut LayoutCache::default())?;
    let mut stream = p.clone().into_stream(StreamOptions { capacity: 1 })?;
    stream.next().unwrap()?;
    stream.cancel();
    assert!(stream.finish().is_err());
    let report = p.into_stream(StreamOptions { capacity: 0 })?.finish()?;
    assert_eq!(report.decoded_bytes, 18);
    Ok(())
}
#[cfg(not(feature = "native-hdf5"))]
#[test]
fn disabled_feature_fails_explicitly() {
    let r = request(&fixture("v1-be"));
    let error = acquisition::prepare(&r, &mut LayoutCache::default())
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("native-hdf5"), "{error}");
}

#[cfg(feature = "native-hdf5")]
#[test]
fn storage_initialization_keeps_valid_values_without_cf_missing_marker() -> Result<()> {
    for name in ["storage-fill-valid", "distinct-cf-fill"] {
        let mut r = request(&fixture(name));
        r.spatial = Spatial::ArraySlice {
            start: [3, 4],
            count: [3, 4],
            step: [1, 1],
            axes: ["/science/y".into(), "/science/x".into()],
        };
        let p = acquisition::prepare(&r, &mut LayoutCache::default())?;
        let mut stream = p.into_stream(StreamOptions { capacity: 0 })?;
        let c = stream.next().unwrap()?;
        assert_eq!(c.pixels.as_u16().unwrap(), &[7; 12]);
        assert_eq!(c.validity().filter(|v| *v).count(), 12);
        stream.finish()?;
    }
    Ok(())
}
