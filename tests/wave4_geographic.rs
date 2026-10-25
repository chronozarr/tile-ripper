#![cfg(feature = "native-hdf5")]
use serde_json::{json, Value};
use tile_ripper::{
    products::array_grid::{self, NativeGrid},
    Spatial,
};

#[test]
fn nisar_resolver_matches_frozen_proj_geos_centers_and_masks() {
    let fixture: Value = serde_json::from_str(include_str!("data/wave4/utm-centers.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let ys: Vec<f64> = serde_json::from_value(case["ys"].clone()).unwrap();
        let xs: Vec<f64> = serde_json::from_value(case["xs"].clone()).unwrap();
        let aoi: Spatial = serde_json::from_value(case["aoi"].clone()).unwrap();
        let result = array_grid::resolve(
            &NativeGrid::NisarUtm16North,
            &ys,
            &xs,
            &aoi,
            ["y".into(), "x".into()],
        );
        if case["included"] == 0 {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("zero native centers"),
                "{}",
                case["name"]
            );
            continue;
        }
        let resolved = result.unwrap();
        assert_eq!(
            resolved.included_centers,
            case["included"].as_u64().unwrap() as usize,
            "{}",
            case["name"]
        );
        let Spatial::ArraySlice { start, count, .. } = resolved.native_selection else {
            panic!()
        };
        assert_eq!(json!(start), case["start"], "{}", case["name"]);
        assert_eq!(json!(count), case["count"], "{}", case["name"]);
        let mask = array_grid::native_aoi_mask(
            &aoi,
            &NativeGrid::NisarUtm16North,
            &ys[start[0]..start[0] + count[0]],
            &xs[start[1]..start[1] + count[1]],
        )
        .unwrap();
        assert_eq!(json!(mask), case["mask"], "{}", case["name"]);
    }
}

#[test]
fn nisar_rejects_unqualified_axes_geometry_and_resource_work() {
    let grid = NativeGrid::NisarUtm16North;
    let bounds = Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox: [-87.01, 36., -86.99, 36.01],
    };
    for (ys, xs) in [
        (vec![4_000_000., 3_999_990.], vec![500_000., 500_000.]),
        (vec![4_000_000., 4_000_010.], vec![500_000., 500_010.]),
        (vec![4_000_000., 3_999_990.], vec![500_000., 500_011.]),
        (vec![f64::NAN, 3_999_990.], vec![500_000., 500_010.]),
        (vec![4_000_000., 3_999_990.], vec![200_000., 200_010.]),
    ] {
        assert!(array_grid::resolve(&grid, &ys, &xs, &bounds, ["y".into(), "x".into()]).is_err());
    }
    let ys = vec![4_000_000., 3_999_990.];
    let xs = vec![500_000., 500_010.];
    for bbox in [
        [-90., -1., -89., 1.],
        [-90., 30., -80., 40.],
        [179., 30., -179., 31.],
        [-86., 85., -85., 86.],
    ] {
        let aoi = Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox,
        };
        assert!(array_grid::resolve(&grid, &ys, &xs, &aoi, ["y".into(), "x".into()]).is_err());
    }
    // Large source is bounded by actual axes, then admitted work is capped.
    let ys = (0..65536)
        .map(|i| 4_500_000. - i as f64 * 10.)
        .collect::<Vec<_>>();
    let xs = (0..50000)
        .map(|i| 300_000. + i as f64 * 10.)
        .collect::<Vec<_>>();
    let aoi = Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox: [-89., 35., -85., 40.],
    };
    assert!(
        array_grid::resolve(&grid, &ys, &xs, &aoi, ["y".into(), "x".into()])
            .unwrap_err()
            .to_string()
            .contains("256 million")
    );
}

#[test]
fn native_selection_remains_explicit_and_source_bounded() {
    let aoi = Spatial::ArraySlice {
        start: [1, 0],
        count: [2, 2],
        step: [2, 1],
        axes: ["y".into(), "x".into()],
    };
    let grid = NativeGrid::NisarUtm16North;
    let ys = [4., 3., 2., 1.];
    let xs = [1., 2.];
    let resolved = array_grid::resolve(&grid, &ys, &xs, &aoi, ["y".into(), "x".into()]).unwrap();
    assert_eq!(resolved.included_centers, 4);
    assert_eq!(
        array_grid::native_aoi_mask(&aoi, &grid, &[f64::NAN], &[f64::NAN]).unwrap(),
        vec![1]
    );
    assert!(array_grid::resolve(&grid, &ys[..3], &xs, &aoi, ["y".into(), "x".into()]).is_err());
    assert!(array_grid::resolve(&grid, &ys, &xs, &aoi, ["wrong".into(), "x".into()]).is_err());
}

#[test]
fn polar_laea_matches_proj_and_separates_projection_domain_from_missingness() {
    let grid = NativeGrid::EaseGridNorth { epsg: 6931 };
    let fixture: Value =
        serde_json::from_str(include_str!("data/wave4/polar-navigation.json")).unwrap();
    for p in fixture["cases"].as_array().unwrap() {
        let actual = grid.longitude_latitude(
            p["y"].as_f64().unwrap(),
            p["x"].as_f64().unwrap_or(f64::NAN),
        );
        if p["expected"].is_null() {
            assert!(actual.is_none());
        } else {
            let actual = actual.unwrap();
            for i in 0..2 {
                assert!(
                    (actual[i] - p["expected"][i].as_f64().unwrap()).abs() < 2e-8,
                    "{p:?} {actual:?}"
                );
            }
        }
    }
    // Native square corners have mathematical coordinates south of the equator;
    // their source science validity must come from source fields, never position.
    assert!(grid.longitude_latitude(8_995_500., -8_995_500.).unwrap()[1] < 0.);
    assert!(NativeGrid::EaseGridNorth { epsg: 6933 }
        .longitude_latitude(0., 0.)
        .is_none());
    let center = grid.longitude_latitude(4500., 4500.).unwrap();
    let aoi = Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox: [center[0], center[1], center[0] + 0.01, center[1] + 0.001],
    };
    assert_eq!(
        array_grid::native_aoi_mask(&aoi, &grid, &[4500.], &[4500.]).unwrap(),
        vec![1]
    );
}
