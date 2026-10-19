use gdal::{
    raster::{Buffer, RasterCreationOptions},
    DriverManager,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tile_ripper::{
    cog::{tiles, window, LayoutCache},
    *,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tile-ripper-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&p).unwrap();
    p
}
fn fixture(root: &std::path::Path) -> Request {
    let file = root.join("source.tif");
    let mut ds = DriverManager::get_driver_by_name("GTiff")
        .unwrap()
        .create_with_band_type_with_options::<u16, _>(
            &file,
            64,
            64,
            1,
            &RasterCreationOptions::from_iter([
                "TILED=YES",
                "BLOCKXSIZE=16",
                "BLOCKYSIZE=16",
                "COMPRESS=DEFLATE",
                "PREDICTOR=2",
            ]),
        )
        .unwrap();
    ds.set_geo_transform(&[100., 10., 0., 1000., 0., -10.])
        .unwrap();
    ds.set_spatial_ref(&gdal::spatial_ref::SpatialRef::from_epsg(32617).unwrap())
        .unwrap();
    let mut b = ds.rasterband(1).unwrap();
    b.set_no_data_value(Some(0.)).unwrap();
    b.set_scale(0.0001).unwrap();
    b.set_offset(-0.1).unwrap();
    b.write(
        (0, 0),
        (64, 64),
        &mut Buffer::new((64, 64), (0..4096).map(|v| v as u16).collect()),
    )
    .unwrap();
    drop(ds);
    Request {
        source: Source::Assets {
            assets: vec![Asset {
                etag: None,
                id: "test".into(),
                locator: file.to_str().unwrap().into(),
                format: Format::Cog,
                datetime: Some("2025-06-01T00:00:00Z".into()),
            }],
        },
        spatial: Spatial::Bounds {
            crs: "EPSG:32617".into(),
            bbox: [250., 690., 410., 850.],
        },
        time: None,
        variables: vec![Variable::Band { index: 1 }],
        quality: vec![],
        grid: Grid::Native,
        operations: vec![],
        sink: Sink::Callback,
        budget: Budget {
            buffer_bytes: 512,
            max_nodes: 100,
            chunk_edge: 16,
        },
    }
}
#[test]
fn native_pixels_tiles_georef_cache_and_sink_failure() {
    let root = dir();
    let r = fixture(&root);
    let mut cache = LayoutCache::default();
    let p = plan(&r, &mut cache).unwrap();
    assert_eq!(
        p.selected[0].window,
        Window {
            x: 15,
            y: 15,
            width: 16,
            height: 16
        }
    );
    let Operation::Read {
        read: Read::LibraryWindow { tiles, .. },
    } = &p.nodes[1].operation
    else {
        panic!()
    };
    assert_eq!(tiles.len(), 4);
    assert!(tiles
        .iter()
        .all(|t| t.offset.is_some() && t.bytes.is_some()));
    let warm = plan(&r, &mut cache).unwrap();
    assert!(matches!(
        warm.nodes[0].operation,
        Operation::Inspect { cached: true }
    ));
    let file = root.join("cache.json");
    cache.save(&file).unwrap();
    let mut loaded = LayoutCache::load(&file).unwrap();
    assert!(matches!(
        plan(&r, &mut loaded).unwrap().nodes[0].operation,
        Operation::Inspect { cached: true }
    ));
    let report = execute(&r, &mut cache, |c| {
        assert_eq!(c.transform, [250., 10., 0., 850., 0., -10.]);
        assert_eq!(c.scale, Some(0.0001));
        assert_eq!(c.offset, Some(-0.1));
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(c.pixels[y * 16 + x], ((y + 15) * 64 + x + 15) as u16);
            }
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(report.decoded_bytes, 512);
    assert!(execute(&r, &mut cache, |_| Err("sink cancelled".into())).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn rejects_unsupported_and_over_budget_before_io() {
    let root = dir();
    let mut r = fixture(&root);
    let mut c = LayoutCache::default();
    r.budget.buffer_bytes = 1;
    assert!(plan(&r, &mut c).is_err());
    r.budget.buffer_bytes = 512;
    r.quality.push(QualityFilter::Metadata {
        field: "cloud".into(),
        comparison: Comparison::LessOrEqual,
        value: 20.,
    });
    assert!(plan(&r, &mut c).is_err());
    r.quality.clear();
    r.grid = Grid::Target {
        crs: "EPSG:4326".into(),
        resolution: [0.1, 0.1],
        resampling: "nearest".into(),
    };
    assert!(plan(&r, &mut c).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn empty_time_selection_no_pixels_and_invalid_time_rejected() {
    let root = dir();
    let mut r = fixture(&root);
    let mut c = LayoutCache::default();
    r.time = Some(Time::Interval {
        start: "2026-01-01T00:00:00Z".into(),
        end: "2026-12-31T00:00:00Z".into(),
    });
    assert!(plan(&r, &mut c).unwrap().selected.is_empty());
    r.time = Some(Time::Instants {
        values: vec!["yesterday".into()],
    });
    assert!(plan(&r, &mut c).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn zarr_chunk_objects_and_execution_preflight() {
    let root = dir();
    let mut r = fixture(&root);
    fs::write(root.join(".zarray"),r#"{"zarr_format":2,"shape":[64,64],"chunks":[16,16],"dtype":"<u2","order":"C","compressor":null,"filters":null}"#).unwrap();
    fs::write(
        root.join(".zattrs"),
        r#"{"tile_ripper_grid":{"crs":"EPSG:32617","transform":[100,10,0,1000,0,-10]}}"#,
    )
    .unwrap();
    let Source::Assets { assets } = &mut r.source else {
        panic!()
    };
    assets[0].locator = root.to_str().unwrap().into();
    assets[0].format = Format::ZarrV2;
    r.variables = vec![Variable::Dataset { path: "/".into() }];
    let mut c = LayoutCache::default();
    let p = plan(&r, &mut c).unwrap();
    let keys: Vec<_> = p
        .nodes
        .iter()
        .filter_map(|n| match &n.operation {
            Operation::Read {
                read: Read::ChunkObject { key, .. },
            } => Some(key.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, vec!["0.0", "0.1", "1.0", "1.1"]);
    let mut calls = 0;
    assert!(execute(&r, &mut c, |_| {
        calls += 1;
        Ok(())
    })
    .is_err());
    assert_eq!(calls, 0);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn disjoint_and_fractional_bounds_clip_correctly() {
    let layout = Layout {
        width: 10,
        height: 10,
        transform: [0., 1., 0., 10., 0., -1.],
        crs: "native".into(),
        variable: tile_ripper::Variable::Band { index: 1 },
        block: [4, 4],
        dtype: "UInt16".into(),
        nodata: None,
        scale: None,
        offset: None,
        tiles: vec![],
    };
    assert!(window(
        &layout,
        &Spatial::Bounds {
            crs: "native".into(),
            bbox: [-4., 1., -1., 5.]
        }
    )
    .unwrap()
    .is_none());
    let w = window(
        &layout,
        &Spatial::Bounds {
            crs: "native".into(),
            bbox: [-1., 7.9, 4.1, 11.],
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        w,
        Window {
            x: 0,
            y: 0,
            width: 5,
            height: 3
        }
    );
    assert!(tiles(&layout, &w).is_empty());
}
#[test]
fn output_preserves_metadata_and_never_overwrites() {
    let root = dir();
    let mut r = fixture(&root);
    let out = root.join("out");
    r.sink = Sink::GeoTiffDirectory {
        path: out.to_str().unwrap().into(),
    };
    let mut c = LayoutCache::default();
    fetch(&r, &mut c).unwrap();
    assert!(fetch(&r, &mut c).is_err());
    let ds = gdal::Dataset::open(out.join("chunk-1.tif")).unwrap();
    assert_eq!(ds.geo_transform().unwrap(), [250., 10., 0., 850., 0., -10.]);
    assert_eq!(ds.rasterband(1).unwrap().offset(), Some(-0.1));
    drop(ds);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_variables_node_budget_and_invalid_ids_rejected() {
    let root = dir();
    let mut r = fixture(&root);
    let mut c = LayoutCache::default();
    r.variables.push(Variable::Band { index: 1 });
    assert!(plan(&r, &mut c)
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
    r.variables.pop();
    r.budget.max_nodes = 1;
    assert!(plan(&r, &mut c).is_err());
    r.budget.max_nodes = 100;
    let Source::Assets { assets } = &mut r.source else {
        panic!()
    };
    assets[0].id = "https://secret.invalid/?token=private".into();
    assert!(plan(&r, &mut c).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_local_cache_invalidates_after_source_replacement() {
    let root = dir();
    let r = fixture(&root);
    let mut cache = LayoutCache::default();
    plan(&r, &mut cache).unwrap();
    // A differently sized image at the same locator must invalidate persisted layout.
    let file = root.join("source.tif");
    let mut ds = DriverManager::get_driver_by_name("GTiff")
        .unwrap()
        .create_with_band_type::<u16, _>(&file, 128, 128, 1)
        .unwrap();
    ds.set_geo_transform(&[100., 10., 0., 1000., 0., -10.])
        .unwrap();
    ds.set_spatial_ref(&gdal::spatial_ref::SpatialRef::from_epsg(32617).unwrap())
        .unwrap();
    drop(ds);
    let p = plan(&r, &mut cache).unwrap();
    assert!(matches!(
        p.nodes[0].operation,
        Operation::Inspect { cached: false }
    ));
    assert_eq!(p.selected[0].layout.as_ref().unwrap().width, 128);
    fs::remove_dir_all(root).unwrap();
}
