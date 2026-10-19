//! Geometry-only CRS mapping and GDAL pixel-center masks; raster samples never warp.
use crate::{fail, Layout, Result, Spatial, Window};
use gdal::{
    raster::{rasterize, Buffer},
    spatial_ref::{AxisMappingStrategy, SpatialRef},
    vector::Geometry,
    DriverManager,
};
use serde_json::{json, Value};

pub fn srs(definition: &str) -> Result<SpatialRef> {
    let mut srs = SpatialRef::from_definition(definition)?;
    srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    Ok(srs)
}
pub fn geometry(spatial: &Spatial) -> Result<Geometry> {
    let (crs, coordinates) = match spatial {
        Spatial::Bounds {
            crs,
            bbox: [x0, y0, x1, y1],
        } => (
            crs,
            json!([[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]]),
        ),
        Spatial::Polygon { crs, coordinates } => (crs, coordinates.clone()),
    };
    let rings = coordinates
        .as_array()
        .ok_or("polygon coordinates must be rings")?;
    if rings.is_empty() {
        return fail("polygon has no rings");
    }
    let mut count = 0;
    for ring in rings {
        let points = ring.as_array().ok_or("invalid ring")?;
        if points.len() < 4 || points.first() != points.last() {
            return fail("polygon rings must be closed and have at least four positions");
        }
        for point in points {
            count += 1;
            let p = point.as_array().ok_or("invalid position")?;
            if p.len() != 2 || !p.iter().all(|v| v.as_f64().is_some_and(f64::is_finite)) {
                return fail("polygon must contain finite 2D positions");
            }
        }
    }
    if count > 10_000 {
        return fail("AOI exceeds 10000 input positions");
    }
    let mut g =
        Geometry::from_geojson(&json!({"type":"Polygon","coordinates":coordinates}).to_string())?;
    g.set_spatial_ref(srs(crs)?);
    if !g.is_valid() || g.is_empty() || g.area() <= 0. {
        return fail("AOI polygon is empty or invalid");
    }
    Ok(g)
}
/// Densify source-CRS straight edges before PROJ transforms. Geographic edges
/// use <=0.001 degrees; projected edges <=100 source units. Large/wrapped AOIs
/// are outside the supported STAC workflow, rather than silently approximated.
pub fn transform(spatial: &Spatial, target: &str) -> Result<Geometry> {
    let source = geometry(spatial)?;
    let source_srs = source.spatial_ref().ok_or("AOI has no CRS")?;
    let target_srs = srs(target)?;
    if source_srs == target_srs {
        return Ok(source);
    }
    let mut value: Value = serde_json::from_str(&source.json()?)?;
    let step = if source_srs.is_geographic() {
        0.001
    } else {
        100.
    };
    let rings = value["coordinates"]
        .as_array_mut()
        .ok_or("invalid polygon")?;
    let mut total = 0usize;
    for ring in rings {
        let points = ring.as_array().ok_or("invalid ring")?;
        let mut dense = Vec::new();
        for pair in points.windows(2) {
            let (x0, y0, x1, y1) = (
                pair[0][0].as_f64().unwrap(),
                pair[0][1].as_f64().unwrap(),
                pair[1][0].as_f64().unwrap(),
                pair[1][1].as_f64().unwrap(),
            );
            let n = ((x1 - x0).abs().max((y1 - y0).abs()) / step).ceil().max(1.) as usize;
            total = total.checked_add(n).ok_or("AOI vertex count overflow")?;
            if total > 100_000 {
                return fail("densified AOI exceeds 100000 positions; narrow the AOI");
            }
            for i in 0..n {
                let f = i as f64 / n as f64;
                dense.push(json!([x0 + (x1 - x0) * f, y0 + (y1 - y0) * f]));
            }
        }
        dense.push(points.last().unwrap().clone());
        *ring = Value::Array(dense);
    }
    let mut dense = Geometry::from_geojson(&value.to_string())?;
    dense.set_spatial_ref(source_srs);
    let mapped = dense.transform_to(&target_srs)?;
    if !mapped.is_valid() {
        return fail("AOI transformation produced invalid geometry");
    }
    Ok(mapped)
}
pub fn bounds(g: &Geometry) -> [f64; 4] {
    let e = g.envelope();
    [e.MinX, e.MinY, e.MaxX, e.MaxY]
}
pub fn window_transform(l: &Layout, w: &Window) -> [f64; 6] {
    let mut t = l.transform;
    t[0] += w.x as f64 * t[1];
    t[3] += w.y as f64 * t[5];
    t
}
pub fn mask(g: &Geometry, l: &Layout, w: &Window) -> Result<Buffer<u8>> {
    let mut ds = DriverManager::get_driver_by_name("MEM")?
        .create_with_band_type::<u8, _>("", w.width, w.height, 1)?;
    ds.set_geo_transform(&window_transform(l, w))?;
    ds.set_spatial_ref(&srs(&l.crs)?)?;
    rasterize(&mut ds, &[1], std::slice::from_ref(g), &[1.], None)?;
    Ok(ds
        .rasterband(1)?
        .read_as::<u8>((0, 0), (w.width, w.height), (w.width, w.height), None)?)
}
pub fn native_mask(spatial: &Spatial, l: &Layout) -> Result<Option<Geometry>> {
    if matches!(spatial,Spatial::Bounds { crs,.. } if *crs==l.crs) {
        return Ok(None);
    }
    let g = transform(spatial, &l.crs)?;
    let extent = Spatial::Bounds {
        crs: l.crs.clone(),
        bbox: [
            l.transform[0],
            l.transform[3] + l.height as f64 * l.transform[5],
            l.transform[0] + l.width as f64 * l.transform[1],
            l.transform[3],
        ],
    };
    let grid = geometry(&extent)?;
    let clipped = g
        .intersection(&grid)
        .ok_or("geometry intersection failed; GDAL with GEOS is required")?;
    Ok(Some(clipped))
}
