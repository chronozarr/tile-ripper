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

/// Outer pixel envelope, clipped to source extent; half-open pixel bounds.
pub fn window(layout: &Layout, spatial: &Spatial) -> Result<Option<Window>> {
    let Spatial::Bounds {
        crs,
        bbox: [xmin, ymin, xmax, ymax],
    } = spatial
    else {
        return fail("polygon selection is not implemented");
    };
    if *crs != layout.crs {
        return fail(
            "bounds CRS must equal source authority string; reprojection is not implemented",
        );
    }
    let t = layout.transform;
    let ax = (xmin - t[0]) / t[1];
    let bx = (xmax - t[0]) / t[1];
    let ay = (ymin - t[3]) / t[5];
    let by = (ymax - t[3]) / t[5];
    let x0 = ax.min(bx).floor().clamp(0., layout.width as f64) as usize;
    let x1 = ax.max(bx).ceil().clamp(0., layout.width as f64) as usize;
    let y0 = ay.min(by).floor().clamp(0., layout.height as f64) as usize;
    let y1 = ay.max(by).ceil().clamp(0., layout.height as f64) as usize;
    Ok((x1 > x0 && y1 > y0).then_some(Window {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    }))
}

/// Native rectilinear cell intersection using actual coordinate midpoints.
/// Also preserves boundary selection for regular grids with rounded centers.
pub(crate) fn coordinate_window(
    crs: &str,
    spatial: &Spatial,
    x: &[f64],
    y: &[f64],
) -> Result<Option<Window>> {
    let Spatial::Bounds {
        crs: requested,
        bbox: [xmin, ymin, xmax, ymax],
    } = spatial
    else {
        return fail("native coordinate selection requires bounds");
    };
    if requested != crs {
        return fail("bounds CRS must equal native coordinate CRS");
    }
    fn indices(v: &[f64], lo: f64, hi: f64) -> Option<(usize, usize)> {
        let selected: Vec<_> = v
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let a = if i == 0 {
                    *c - (v[1] - v[0]) / 2.
                } else {
                    v[i - 1] / 2. + c / 2.
                };
                let b = if i + 1 == v.len() {
                    *c + (v[i] - v[i - 1]) / 2.
                } else {
                    c / 2. + v[i + 1] / 2.
                };
                (a.min(b) < hi && a.max(b) > lo).then_some(i)
            })
            .collect();
        Some((*selected.first()?, selected.last()? + 1))
    }
    let Some((x0, x1)) = indices(x, *xmin, *xmax) else {
        return Ok(None);
    };
    let Some((y0, y1)) = indices(y, *ymin, *ymax) else {
        return Ok(None);
    };
    Ok(Some(Window {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }))
}

#[cfg(test)]
mod coordinate_tests {
    use super::*;
    #[test]
    fn rounded_centers_use_actual_midpoints_in_both_directions() {
        let x: Vec<f64> = (0..5)
            .map(|i| ((-80.05 + i as f64 * 0.1) as f32) as f64)
            .collect();
        let y = vec![35.05, 35.15, 35.25];
        let boundary = x[1] / 2. + x[2] / 2.;
        let selection = Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [boundary, 35.01, x[3] / 2. + x[4] / 2., 35.29],
        };
        let w = coordinate_window("EPSG:4326", &selection, &x, &y)
            .unwrap()
            .unwrap();
        assert_eq!((w.x, w.width), (2, 2));
        let reversed: Vec<_> = x.iter().copied().rev().collect();
        let w = coordinate_window("EPSG:4326", &selection, &reversed, &y)
            .unwrap()
            .unwrap();
        assert_eq!((w.x, w.width), (1, 2));
    }
}
