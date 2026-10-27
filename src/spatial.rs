//! Native grid selection; optional GDAL geometry and projection extension.
use crate::{fail, Layout, Result, Spatial, Window};
/// A product-private exception to the normal UTM hemisphere guard.  It is not
/// part of the request, layout, or observation schemas: callers may only get
/// one after a product-specific inspected-grid gate has accepted an exact
/// provider convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeSelection {
    Standard,
    /// A southern MGRS tile stored in its northern UTM zone with false northing
    /// zero.  The source CRS and coordinates remain untouched.
    ProviderExtendedNorthernUtm {
        zone: u32,
    },
}
#[cfg(feature = "gdal")]
#[path = "spatial_gdal.rs"]
mod gdal_impl;
#[cfg(feature = "gdal")]
pub use gdal_impl::*;
#[cfg(all(feature = "native-cog", any(not(feature = "gdal"), test)))]
#[path = "spatial_native.rs"]
mod native_impl;
#[cfg(all(not(feature = "gdal"), feature = "native-cog"))]
pub use native_impl::*;
pub fn window_transform(l: &Layout, w: &Window) -> [f64; 6] {
    let mut t = l.transform;
    t[0] += w.x as f64 * t[1];
    t[3] += w.y as f64 * t[5];
    t
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

#[cfg(all(not(feature = "gdal"), not(feature = "native-cog")))]
mod native {
    use super::*;
    pub struct Geometry;
    impl Geometry {
        pub fn is_empty(&self) -> bool {
            true
        }
        pub fn area(&self) -> f64 {
            0.
        }
        pub fn json(&self) -> Result<String> {
            fail("polygon geometry requires the gdal feature")
        }
        pub fn intersection(&self, _: &Self) -> Option<Self> {
            None
        }
    }
    pub struct Mask;
    impl Mask {
        pub fn data(&self) -> &[u8] {
            &[]
        }
    }
    pub fn geometry(_: &Spatial) -> Result<Geometry> {
        fail("polygon AOIs require the gdal feature")
    }
    pub fn from_geojson(_: &serde_json::Value, _: &str) -> Result<Geometry> {
        fail("polygon geometry requires native-cog or gdal")
    }
    pub fn transform(_: &Spatial, _: &str) -> Result<Geometry> {
        fail("AOI transformations require native-cog or gdal")
    }
    pub fn bounds(_: &Geometry) -> [f64; 4] {
        unreachable!()
    }
    pub fn mask(_: &Geometry, _: &Layout, _: &Window) -> Result<Mask> {
        fail("polygon masks require the gdal feature")
    }
    pub fn native_mask(s: &Spatial, l: &Layout) -> Result<Option<Geometry>> {
        match s {
            Spatial::Bounds {crs, ..} if crs == &l.crs => Ok(None),
            _ => fail("GDAL-free selection requires bounds in the exact native CRS; AOI transformation and polygon masks require the gdal feature"),
        }
    }
    pub(crate) fn native_mask_for_selection(
        s: &Spatial,
        l: &Layout,
        _: NativeSelection,
    ) -> Result<Option<Geometry>> {
        native_mask(s, l)
    }
}
#[cfg(all(not(feature = "gdal"), not(feature = "native-cog")))]
pub use native::*;

pub(crate) fn validate_crs(crs: &str) -> Result<()> {
    #[cfg(feature = "gdal")]
    {
        gdal::spatial_ref::SpatialRef::from_definition(crs)?;
    }
    #[cfg(not(feature = "gdal"))]
    if !crs
        .strip_prefix("EPSG:")
        .is_some_and(|s| s.parse::<u32>().is_ok_and(|n| n > 0))
    {
        return fail("GDAL-free native grids require an EPSG authority string");
    }
    Ok(())
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
