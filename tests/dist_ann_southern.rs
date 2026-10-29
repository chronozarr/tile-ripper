//! Public generic-COG boundaries must survive product-specific southern admission.
#![cfg(all(feature = "native-cog", not(feature = "gdal")))]

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tile_ripper::{acquisition, cog::LayoutCache, *};

fn request(name: &str, spatial: Spatial, sink: Sink) -> Request {
    Request {
        source: Source::Assets {
            assets: vec![Asset {
                id: name.into(),
                locator: Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/data/dist_ann_southern")
                    .join(name)
                    .to_string_lossy()
                    .into(),
                format: Format::Cog,
                datetime: None,
                etag: None,
            }],
        },
        spatial,
        time: None,
        variables: vec![Variable::Band { index: 1 }],
        quality: vec![],
        grid: Grid::Native,
        operations: vec![],
        sink,
        budget: Budget {
            buffer_bytes: 16 * 1024 * 1024,
            max_nodes: 4096,
            chunk_edge: 16,
        },
    }
}
fn geographic() -> Spatial {
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("data/dist_ann_southern/reference.json")).unwrap();
    let bbox = serde_json::from_value(reference["geographic_bbox"].clone()).unwrap();
    Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox,
    }
}
fn output() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "tile-ripper-southern-{}-{}.zarr",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn bare_geographic_cogs_do_not_inherit_the_annual_hemisphere_exception() {
    let extended = request("extended_north.tif", geographic(), Sink::Callback);
    let error = match acquisition::prepare(&extended, &mut LayoutCache::default()) {
        Ok(_) => panic!("a bare COG must not claim annual product admission"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("hemisphere"), "{error}");

    let conventional = request("conventional_south.tif", geographic(), Sink::Callback);
    let prepared = acquisition::prepare(&conventional, &mut LayoutCache::default()).unwrap();
    assert!(!prepared.plan().selected.is_empty());
    let layout = prepared.plan().selected[0].layout.as_ref().unwrap();
    assert_eq!(layout.crs, "EPSG:32720");
    assert_eq!(layout.transform[3], 8900020.);
}

#[test]
fn explicit_native_coordinates_preserve_extended_north_crs_and_negative_axes() {
    // Already-projected native bounds have always been valid. The fix must not
    // reinterpret this frame as EPSG:32720 or add false northing to output.
    let destination = output();
    let request = request(
        "extended_north.tif",
        Spatial::Bounds {
            crs: "EPSG:32620".into(),
            bbox: [799980., -1100940., 800940., -1099980.],
        },
        Sink::ZarrDirectory {
            path: destination.to_string_lossy().into(),
        },
    );
    let prepared = acquisition::prepare(&request, &mut LayoutCache::default()).unwrap();
    prepared.fetch().unwrap();
    let attrs: serde_json::Value = serde_json::from_slice(
        &std::fs::read(destination.join("observations/000000/spatial_ref/.zattrs")).unwrap(),
    )
    .unwrap();
    assert_eq!(attrs["epsg_code"], "EPSG:32620");
    assert_eq!(attrs["false_northing"], 0.);
    assert_eq!(attrs["longitude_of_central_meridian"], -63.);
    let store = std::sync::Arc::new(zarrs::filesystem::FilesystemStore::new(&destination).unwrap());
    let y = zarrs::array::Array::open(store.clone(), "/observations/000000/y").unwrap();
    let coordinates = y
        .retrieve_array_subset::<Vec<f64>>(&y.subset_all())
        .unwrap();
    assert_eq!(
        coordinates,
        (0..32)
            .map(|i| -1099980. - (i as f64 + 0.5) * 30.)
            .collect::<Vec<_>>()
    );
    let values = zarrs::array::Array::open(store, "/observations/000000/values").unwrap();
    let raw = values
        .retrieve_array_subset::<Vec<u8>>(&values.subset_all())
        .unwrap();
    let mut expected: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
    expected[0] = 255;
    assert_eq!(raw, expected);
    std::fs::remove_dir_all(destination).unwrap();
}
