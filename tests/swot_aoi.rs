#![cfg(feature = "native-hdf5")]
use serde_json::{json, Value};
use tile_ripper::{
    swot_aoi::{self, Evaluator},
    Result, Spatial,
};
fn bounds() -> Spatial {
    Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox: [-69., 42., -68.995, 42.005],
    }
}
#[test]
fn closed_boundary_holes_and_concavity_are_explicit() -> Result<()> {
    let evaluator = Evaluator::new(&bounds(), "EPSG:32619")?;
    for (lon, lat, want) in [
        (-69., 42., 1),
        (-68.995, 42.005, 1),
        (-68.997, 42.002, 1),
        (-69.000001, 42., 0),
        (
            -69. - swot_aoi::COORDINATE_TOLERANCE_DEGREES * 0.5,
            42.002,
            1,
        ),
        (
            -69. - swot_aoi::COORDINATE_TOLERANCE_DEGREES * 2.,
            42.002,
            0,
        ),
        (-68.997, 42.006, 0),
    ] {
        let p = evaluator.project(lon, lat)?;
        assert_eq!(evaluator.contains(p[0], p[1])?, want);
    }
    let polygon = Spatial::Polygon {
        crs: "EPSG:4326".into(),
        coordinates: json!([
            [
                [-69., 42.],
                [-68.995, 42.],
                [-68.995, 42.002],
                [-68.997, 42.002],
                [-68.997, 42.005],
                [-69., 42.005],
                [-69., 42.]
            ],
            [
                [-68.999, 42.001],
                [-68.998, 42.001],
                [-68.998, 42.002],
                [-68.999, 42.002],
                [-68.999, 42.001]
            ]
        ]),
    };
    let evaluator = Evaluator::new(&polygon, "EPSG:32619")?;
    for (lon, lat, want) in [
        (-68.9985, 42.0015, 0),
        (-68.999, 42.0015, 1),
        (-68.996, 42.004, 0),
        (-68.999, 42.004, 1),
    ] {
        let p = evaluator.project(lon, lat)?;
        assert_eq!(evaluator.contains(p[0], p[1])?, want);
    }
    Ok(())
}
#[test]
fn global_indices_follow_stored_axes_and_empty_has_no_window() -> Result<()> {
    let e = Evaluator::new(&bounds(), "EPSG:32619")?;
    let p = e.project(-69., 42.)?;
    let y: Vec<_> = (0..9).map(|i| p[1] - 200. + i as f64 * 100.).collect();
    let x: Vec<_> = (0..9).map(|i| p[0] - 200. + i as f64 * 100.).collect();
    let (a, _) = swot_aoi::resolve_grid(&bounds(), "EPSG:32619", &y, &x, &Value::Null)?;
    let a = a.unwrap();
    assert_eq!(a.window.start, [2, 2]);
    let ry: Vec<_> = y.iter().rev().copied().collect();
    let rx: Vec<_> = x.iter().rev().copied().collect();
    let (b, _) = swot_aoi::resolve_grid(&bounds(), "EPSG:32619", &ry, &rx, &Value::Null)?;
    let b = b.unwrap();
    assert_eq!(a.included_centers, b.included_centers);
    assert_eq!(a.window.count, b.window.count);
    assert_eq!(
        b.window.start,
        [
            y.len() - a.window.start[0] - a.window.count[0],
            x.len() - a.window.start[1] - a.window.count[1]
        ]
    );
    assert_eq!(b.grid.direction, ["decreasing", "decreasing"]);
    let outside = Spatial::Bounds {
        crs: "EPSG:4326".into(),
        bbox: [-69.2, 42., -69.1, 42.1],
    };
    let (empty, report) = swot_aoi::resolve_grid(&outside, "EPSG:32619", &y, &x, &Value::Null)?;
    assert!(empty.is_none());
    assert_eq!(report["included_native_centers"], 0);
    Ok(())
}
#[test]
fn unsupported_geometry_crs_and_nonuniform_axes_fail() -> Result<()> {
    for aoi in [
        json!({"kind":"bounds","crs":"EPSG:4326","bbox":[179.,0.,-179.,1.]}),
        json!({"kind":"bounds","crs":"EPSG:4326","bbox":[179.,0.,180.,1.]}),
        json!({"kind":"bounds","crs":"EPSG:4326","bbox":[-180.,0.,-179.,1.]}),
        json!({"kind":"bounds","crs":"EPSG:4326","bbox":[-69.,81.,-68.,82.]}),
        json!({"kind":"bounds","crs":"EPSG:3857","bbox":[0.,0.,1.,1.]}),
        json!({"kind":"polygon","crs":"EPSG:4326","coordinates":[[[-69.,42.],[-68.,43.],[-68.,42.],[-69.,43.],[-69.,42.]]]}),
        json!({"kind":"polygon","crs":"EPSG:4326","coordinates":[[[-69.,42.],[-68.,42.],[-68.,43.]]]}),
    ] {
        let aoi: Spatial = serde_json::from_value(aoi)?;
        assert!(swot_aoi::polygon(&aoi).is_err());
    }
    assert!(Evaluator::new(&bounds(), "EPSG:32601").is_err());
    assert!(Evaluator::new(&bounds(), "EPSG:3857").is_err());
    assert!(Evaluator::new(&bounds(), "EPSG:32719").is_err());
    for axis in [
        vec![0., 100., 201.],
        vec![0., 0., 100.],
        vec![0., 100., 50.],
        vec![0., f64::NAN, 200.],
    ] {
        assert!(
            swot_aoi::resolve_grid(&bounds(), "EPSG:32619", &axis, &[0., 100.], &Value::Null)
                .is_err()
        );
    }
    Ok(())
}
