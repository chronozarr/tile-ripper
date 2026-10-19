use serde_json::json;
use tile_ripper::{
    spatial,
    stac_workflow::{self, Limits, StacRequest},
    Layout, Spatial, Window,
};
#[test]
fn masks_preserve_holes_and_use_native_pixel_centers() {
    let g = spatial::geometry(&Spatial::Polygon {
        crs: "EPSG:32617".into(),
        coordinates: json!([
            [[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]],
            [[3, 3], [3, 7], [7, 7], [7, 3], [3, 3]]
        ]),
    })
    .unwrap();
    let layout = Layout {
        width: 10,
        height: 10,
        transform: [0., 1., 0., 10., 0., -1.],
        crs: "EPSG:32617".into(),
        band: 1,
        block: [16, 16],
        dtype: "UInt16".into(),
        nodata: Some(0.),
        scale: None,
        offset: None,
        tiles: vec![],
    };
    let m = spatial::mask(
        &g,
        &layout,
        &Window {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        },
    )
    .unwrap();
    assert_eq!(m.data().iter().filter(|v| **v == 1).count(), 84);
}
#[test]
fn rejects_invalid_polygon_and_unsupported_provider_contract() {
    assert!(spatial::geometry(&Spatial::Polygon {
        crs: "EPSG:4326".into(),
        coordinates: json!([[[0, 0], [1, 1], [1, 0], [0, 1], [0, 0]]])
    })
    .is_err());
    let r = StacRequest {
        endpoint: "https://example.invalid".into(),
        collection: "other".into(),
        aoi: Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [-79., 35., -78., 36.],
        },
        start: "2025-01-01T00:00:00Z".into(),
        end: "2025-01-02T00:00:00Z".into(),
        bands: vec!["B04".into()],
        output: "unused".into(),
        provider: Default::default(),
        limits: Limits::default(),
    };
    assert!(stac_workflow::discover(&r)
        .unwrap_err_string()
        .contains("sentinel-2-c1-l2a"));
}
trait ErrorString {
    fn unwrap_err_string(self) -> String;
}
impl<T> ErrorString for tile_ripper::Result<T> {
    fn unwrap_err_string(self) -> String {
        match self {
            Ok(_) => panic!("expected error"),
            Err(e) => e.to_string(),
        }
    }
}
#[test]
fn references_strip_runtime_credentials() {
    assert_eq!(
        stac_workflow::safe_reference("https://user:password@host/path.tif?token=secret#frag"),
        "https://host/path.tif"
    );
}

/// The local Python STAC fixture advances its ETag on the second search.
#[test]
#[ignore = "run by bench/stac_workflow.py with its bounded local HTTP fixture"]
fn prepared_versions_do_not_reuse_another_snapshot_cache() {
    gdal::config::set_error_handler(|_, _, _| {});
    let file = std::env::var("TILE_RIPPER_STAC_TEST_REQUEST").unwrap();
    let request: StacRequest = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let mut cache = tile_ripper::cog::LayoutCache::default();
    let older = stac_workflow::prepare(&request, &mut cache).unwrap();
    let newer = stac_workflow::prepare(&request, &mut cache).unwrap();
    assert_ne!(
        older.sources()[0].object_identity.as_ref().unwrap().etag,
        newer.sources()[0].object_identity.as_ref().unwrap().etag
    );
    newer.execute(|_| Ok(())).unwrap();
    assert!(older.execute(|_| Ok(())).is_err());
}
