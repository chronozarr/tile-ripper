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

#[cfg(feature = "native-hdf5")]
#[test]
fn chunked_coordinate_strides_descending_order_and_partial_edges() -> Result<()> {
    let r = request(&fixture("chunked-coordinates"));
    let prepared = acquisition::prepare(&r, &mut LayoutCache::default())?;
    let explanation = prepared.explain();
    let binding = &explanation["sources"][0]["object_identities"]["datasets"][0];
    assert_eq!(binding["root_attributes"]["cycle_number"]["value"], 11.);
    for axis in binding["axes"].as_array().unwrap() {
        assert_eq!(axis["attributes"]["units"]["value"], "m");
        assert_eq!(axis["storage"]["chunks"].as_array().unwrap().len(), 3);
        assert!(
            axis["storage"]["prepare_reserved_bytes_peak"]
                .as_u64()
                .unwrap()
                <= r.budget.buffer_bytes as u64
        );
    }
    let mut stream = prepared.into_stream(StreamOptions { capacity: 0 })?;
    let mut seen = std::collections::BTreeSet::new();
    for c in stream.by_ref() {
        let c = c?;
        let (x, y) = c.coordinates.as_ref().unwrap();
        for (row, cy) in y.iter().enumerate() {
            for (col, cx) in x.iter().enumerate() {
                let sy = ((5000. - cy) / 10.) as usize;
                let sx = ((cx - 1000.) / 5.) as usize;
                assert!([1, 4, 7].contains(&sy));
                assert!([2, 6, 10].contains(&sx));
                assert_eq!(
                    c.pixels.as_u16().unwrap()[row * x.len() + col],
                    (sy * 11 + sx) as u16
                );
                assert!(seen.insert((sy, sx)));
            }
        }
    }
    stream.finish()?;
    assert_eq!(seen.len(), 9);
    Ok(())
}

#[cfg(feature = "native-hdf5")]
#[test]
fn local_coordinate_and_data_mutation_cannot_return_success() -> Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    for mode in ["coordinates-before", "data-during"] {
        let path =
            std::env::temp_dir().join(format!("tile-ripper-hdf5-{}-{mode}.h5", std::process::id()));
        std::fs::copy(fixture("chunked-coordinates"), &path)?;
        let r = request(path.to_str().unwrap());
        let prepared = acquisition::prepare(&r, &mut LayoutCache::default())?;
        let mut stream = prepared.into_stream(StreamOptions { capacity: 0 })?;
        if mode == "data-during" {
            stream.next().unwrap()?;
        }
        let mut f = std::fs::OpenOptions::new().write(true).open(&path)?;
        f.seek(SeekFrom::Start(4096))?;
        f.write_all(&[0x19])?;
        f.sync_all()?;
        assert!(stream.finish().is_err());
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(feature = "native-hdf5")]
#[test]
fn implicit_storage_zero_is_not_cf_nan_in_either_byte_order() -> Result<()> {
    for order in ["le", "be"] {
        for fill in ["None", "7"] {
            let mut r = request(&fixture(&format!("storage-cf-{order}-{fill}")));
            r.spatial = Spatial::ArraySlice {
                start: [0, 0],
                count: [8, 8],
                step: [1, 1],
                axes: ["/y".into(), "/x".into()],
            };
            r.variables = vec![Variable::Dataset {
                path: "/signal".into(),
            }];
            let p = acquisition::prepare(&r, &mut Default::default())?;
            let definition = &p.explain()["sources"][0]["object_identities"]["datasets"][0];
            assert_eq!(
                definition["storage_fill"],
                if fill == "7" { 7. } else { 0. }
            );
            let mut samples = 0;
            let report = p.execute(|c| {
                for (i, v) in c.pixels.as_f32().unwrap().iter().enumerate() {
                    let y = c.window.y + i / c.window.width;
                    let x = c.window.x + i % c.window.width;
                    if y == 0 && x == 1 {
                        assert!(v.is_nan());
                        assert!(!c.validity().nth(i).unwrap());
                    } else {
                        let expected = if y == 0 && x == 0 {
                            0.
                        } else if y == 0 && x == 2 {
                            1.25
                        } else if fill == "7" {
                            7.
                        } else {
                            0.
                        };
                        assert_eq!(*v, expected);
                        assert!(c.validity().nth(i).unwrap());
                    }
                    samples += 1;
                }
                Ok(())
            })?;
            assert_eq!(samples, 64);
            assert_eq!(report.filled_source_chunks, 3);
        }
    }
    Ok(())
}
