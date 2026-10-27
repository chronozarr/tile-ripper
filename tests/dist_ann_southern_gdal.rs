//! GDAL already supports extended projection math; retain its generic behavior.
#![cfg(feature = "gdal")]

#[test]
fn generic_gdal_transformation_keeps_its_existing_southern_northern_zone_math() {
    use tile_ripper::{spatial, Spatial};
    // This intentionally spans >5 degrees: the generic GDAL API did not have
    // the native adapter's geographic-domain or hemisphere policy restrictions.
    let geometry = spatial::transform(
        &Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [-64., -15., -58., -9.],
        },
        "EPSG:32620",
    )
    .unwrap();
    let extent = spatial::bounds(&geometry);
    assert!(extent.iter().all(|value| value.is_finite()));
    assert!(extent[1] < extent[3] && extent[3] < 0.);
}
