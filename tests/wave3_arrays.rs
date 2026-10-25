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

#[test]
fn invalid_native_counts_strides_and_overflow_fail_before_discovery() {
    use serde_json::json;
    use tile_ripper::products::ProductRequest;
    for (count, step, start) in [
        ([1, 1], [0, 1], [0, 0]),
        ([0, 1], [1, 1], [0, 0]),
        ([2, 1], [usize::MAX, 1], [2, 0]),
        ([65537, 1], [1, 1], [0, 0]),
    ] {
        let r:ProductRequest=serde_json::from_value(json!({"product":"goes19-abi-cmipc-c13","start":"2026-10-24T16:00:00Z","end":"2026-10-24T16:05:00Z","bands":["CMI"],"aoi":{"kind":"array_slice","start":start,"count":count,"step":step,"axes":["/y","/x"]}})).unwrap();
        let error = arrays::discover(&r).unwrap_err().to_string();
        assert!(error.contains("native selection"), "{error}");
    }
}
#[test]
fn smap_policy_preserves_flag8_identity_and_excludes_unknown_words() {
    assert!(arrays::quality_accepts("smap_retrieval_recommended", 0.).unwrap());
    assert!(arrays::quality_accepts("smap_retrieval_recommended", 8.).unwrap());
    for code in [1., 7., 9., 15., 16., 65534., 65535.] {
        assert!(!arrays::quality_accepts("smap_retrieval_recommended", code).unwrap());
    }
}
#[test]
fn nisar_native_navigation_matches_independent_proj_reference() {
    let xy = NativeGrid::NisarUtm16North
        .longitude_latitude(4052515., 551885.)
        .unwrap();
    assert!((xy[0] - -86.41976768852962).abs() < 1e-9);
    assert!((xy[1] - 36.616747474338396).abs() < 1e-9);
}
