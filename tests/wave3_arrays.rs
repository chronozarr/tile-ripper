#![cfg(feature = "native-hdf5")]
use tile_ripper::products::{array_grid::NativeGrid, arrays};
fn grid() -> NativeGrid {
    NativeGrid::Geostationary {
        perspective_point_height: 35786023.,
        semi_major_axis: 6378137.,
        semi_minor_axis: 6356752.31414,
        longitude_of_projection_origin: -75.,
        sweep_angle_axis: "x".into(),
        y_packing: [-5.6000000768108293e-5, 0.12821200489997864],
        x_packing: [5.6000000768108293e-5, -0.10133200138807297],
    }
}
#[test]
fn navigation_preserves_space_and_native_radian_packing() {
    let g = grid();
    assert!(g.longitude_latitude(0., 0.).is_none());
    let NativeGrid::Geostationary {
        y_packing: yp,
        x_packing: xp,
        ..
    } = g.clone()
    else {
        panic!("wrong test grid")
    };
    let center = g
        .longitude_latitude(-yp[1] / yp[0], -xp[1] / xp[0])
        .unwrap();
    assert!((center[0] + 75.).abs() < 1e-12);
    assert!(center[1].abs() < 1e-12);
    // Real-source PROJ equality is recorded by bench/wave3_arrays.py.
    let p = g.longitude_latitude(900., 1200.).unwrap();
    assert!(p[0] < -75. && p[1] > 0.);
    assert!(g.longitude_latitude(f64::NAN, 0.).is_none());
}
#[test]
fn quality_policy_does_not_fold_other_codes_into_good() {
    for v in [-1., 1., 2., 3., 4., 255., f64::NAN] {
        assert!(!arrays::quality_accepts("goes_dqf_good", v).unwrap());
    }
    assert!(arrays::quality_accepts("goes_dqf_good", 0.).unwrap());
    assert!(arrays::quality_accepts("unknown", 0.).is_err());
}
