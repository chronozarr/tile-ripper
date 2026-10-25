//! Concrete native navigation for the separately admitted array profiles.
use crate::{fail, Result, Spatial};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "coordinate_model", rename_all = "snake_case")]
pub enum NativeGrid {
    Geostationary {
        perspective_point_height: f64,
        semi_major_axis: f64,
        semi_minor_axis: f64,
        longitude_of_projection_origin: f64,
        sweep_angle_axis: String,
        /// [scale, offset], applied only for navigation; stored axes stay intact.
        y_packing: [f64; 2],
        x_packing: [f64; 2],
    },
}
fn number(attrs: &Value, key: &str) -> Result<f64> {
    attrs[key]["value"]
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("missing finite grid parameter {key}").into())
}
impl NativeGrid {
    pub(crate) fn goes(d: &Value) -> Result<Self> {
        let a = &d["grid_mapping"]["attributes"];
        let axes = d["axes"].as_array().ok_or("missing stored axes")?;
        if axes.len() != 2
            || a["grid_mapping_name"]["value"] != "geostationary"
            || a["sweep_angle_axis"]["value"] != "x"
            || number(a, "latitude_of_projection_origin")? != 0.
        {
            return fail("GOES requires inspected geostationary sweep-x equatorial projection");
        }
        for axis in axes {
            if axis["attributes"]["units"]["value"] != "rad" {
                return fail("GOES scan axes must have radian units");
            }
        }
        let height = number(a, "perspective_point_height")?;
        let major = number(a, "semi_major_axis")?;
        let minor = number(a, "semi_minor_axis")?;
        let origin = number(a, "longitude_of_projection_origin")?;
        if height != 35786023. || major != 6378137. || minor != 6356752.31414 || origin != -75. {
            return fail("unqualified GOES-19 projection parameters");
        }
        let packing = |i: usize| -> Result<[f64; 2]> {
            let a = &axes[i]["attributes"];
            Ok([number(a, "scale_factor")?, number(a, "add_offset")?])
        };
        let y = packing(0)?;
        let x = packing(1)?;
        if y[0] >= 0. || x[0] <= 0. {
            return fail("GOES axes require north-to-south rows and west-to-east columns");
        }
        Ok(Self::Geostationary {
            perspective_point_height: height,
            semi_major_axis: major,
            semi_minor_axis: minor,
            longitude_of_projection_origin: origin,
            sweep_angle_axis: "x".into(),
            y_packing: y,
            x_packing: x,
        })
    }
    /// Longitude/latitude of an actual source center, or None for space.
    pub fn longitude_latitude(&self, y: f64, x: f64) -> Option<[f64; 2]> {
        match self {
            Self::Geostationary {
                perspective_point_height,
                semi_major_axis: a,
                semi_minor_axis: b,
                longitude_of_projection_origin: lon0,
                y_packing: yp,
                x_packing: xp,
                ..
            } => {
                let x = x * xp[0] + xp[1];
                let y = y * yp[0] + yp[1];
                let (sx, cx) = x.sin_cos();
                let (sy, cy) = y.sin_cos();
                let h = perspective_point_height + a;
                let ratio = (a / b).powi(2);
                let aa = sx * sx + cx * cx * (cy * cy + ratio * sy * sy);
                let bb = -2. * h * cx * cy;
                let cc = h * h - a * a;
                let disc = bb * bb - 4. * aa * cc;
                if disc < 0. {
                    return None;
                }
                let r = (-bb - disc.sqrt()) / (2. * aa);
                let east = -r * sx;
                let north = r * cx * sy;
                let near = h - r * cx * cy;
                let lon = lon0 - east.atan2(near).to_degrees();
                let lat = (ratio * north / near.hypot(east)).atan().to_degrees();
                (lon.is_finite() && lat.is_finite()).then_some([lon, lat])
            }
        }
    }
    pub fn describe(&self) -> Value {
        serde_json::json!({"model":self,"dimension_order":["y","x"],"registration":"native pixel centers","stored_axes":"original codes and packing retained; navigation applies packing explicitly","geographic_selection":"inverse navigation of each native center, closed WGS84 polygon; space excluded","unsupported":["reprojection","resampling","antimeridian wrapping","other ABI projection parameters"]})
    }
}
fn polygon(coordinates: &Value) -> Result<geo::Polygon<f64>> {
    use geo::Validation;
    let rings = coordinates.as_array().ok_or("AOI requires polygon rings")?;
    if rings.is_empty() || rings.len() > 17 {
        return fail("AOI polygon requires 1..17 rings");
    }
    let mut out = Vec::new();
    let mut vertices = 0;
    for ring in rings {
        let points = ring.as_array().ok_or("AOI ring requires positions")?;
        vertices += points.len();
        if points.len() < 4 || vertices > 256 {
            return fail("AOI polygon exceeds 256 vertices or has an incomplete ring");
        }
        let mut positions = Vec::new();
        for p in points {
            let pair = p
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or("AOI position requires longitude/latitude")?;
            let x = pair[0].as_f64().ok_or("invalid longitude")?;
            let y = pair[1].as_f64().ok_or("invalid latitude")?;
            if !x.is_finite()
                || !y.is_finite()
                || !(-180. ..=180.).contains(&x)
                || !(-90. ..=90.).contains(&y)
            {
                return fail("AOI position outside WGS84 domain");
            }
            positions.push(geo::Coord { x, y });
        }
        if positions.first() != positions.last()
            || positions.windows(2).any(|p| (p[0].x - p[1].x).abs() > 180.)
        {
            return fail("AOI requires closed rings without antimeridian wrapping");
        }
        out.push(geo::LineString::new(positions));
    }
    let poly = geo::Polygon::new(out.remove(0), out);
    if !poly.is_valid() {
        return fail("invalid AOI polygon");
    }
    Ok(poly)
}
pub(crate) struct Inclusion {
    geometry: Option<geo::Polygon<f64>>,
}
impl Inclusion {
    pub(crate) fn new(aoi: &Spatial) -> Result<Self> {
        let geometry = match aoi {
            Spatial::ArraySlice { .. } => None,
            Spatial::Bounds { crs, bbox }
                if crs == "EPSG:4326"
                    && bbox.iter().all(|v| v.is_finite())
                    && bbox[0] >= -180.
                    && bbox[2] <= 180.
                    && bbox[1] >= -90.
                    && bbox[3] <= 90.
                    && bbox[0] < bbox[2]
                    && bbox[1] < bbox[3] =>
            {
                Some(polygon(&serde_json::json!([[
                    [bbox[0], bbox[1]],
                    [bbox[2], bbox[1]],
                    [bbox[2], bbox[3]],
                    [bbox[0], bbox[3]],
                    [bbox[0], bbox[1]]
                ]]))?)
            }
            Spatial::Polygon { crs, coordinates } if crs == "EPSG:4326" => {
                Some(polygon(coordinates)?)
            }
            _ => {
                return fail(
                    "array product requires a finite WGS84 AOI or explicit native array slice",
                )
            }
        };
        Ok(Self { geometry })
    }
    pub(crate) fn accepts(&self, xy: Option<[f64; 2]>) -> bool {
        use geo::Intersects;
        match &self.geometry {
            None => true,
            Some(g) => xy.is_some_and(|[x, y]| g.intersects(&geo::Point::new(x, y))),
        }
    }
}
pub(crate) fn resolve(
    grid: &NativeGrid,
    ys: &[f64],
    xs: &[f64],
    aoi: &Spatial,
    axes: [String; 2],
) -> Result<Spatial> {
    if let Spatial::ArraySlice {
        axes: requested, ..
    } = aoi
    {
        if requested != &axes {
            return fail("native selection axes conflict with product");
        }
        return Ok(aoi.clone());
    }
    let inclusion = Inclusion::new(aoi)?;
    let mut min = [usize::MAX; 2];
    let mut max = [0; 2];
    for (r, y) in ys.iter().enumerate() {
        for (c, x) in xs.iter().enumerate() {
            if inclusion.accepts(grid.longitude_latitude(*y, *x)) {
                min[0] = min[0].min(r);
                min[1] = min[1].min(c);
                max[0] = max[0].max(r);
                max[1] = max[1].max(c);
            }
        }
    }
    if min[0] == usize::MAX {
        return fail("geographic AOI includes no visible native centers");
    }
    Ok(Spatial::ArraySlice {
        start: min,
        count: [max[0] - min[0] + 1, max[1] - min[1] + 1],
        step: [1, 1],
        axes,
    })
}
