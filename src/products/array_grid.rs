//! Concrete native navigation for the separately admitted array profiles.
use crate::{fail, Result, Spatial};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "coordinate_model", rename_all = "snake_case")]
pub enum NativeGrid {
    EaseGridGlobal {
        epsg: u32,
    },
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
            Self::EaseGridGlobal { epsg: 6933 } => ease_inverse(y, x),
            Self::EaseGridGlobal { .. } => None,
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
        if matches!(self, Self::EaseGridGlobal { .. }) {
            return serde_json::json!({"model":self,"dimension_order":["y","x"],"registration":"native pixel centers","axis_units":"metres","directions":["northing decreases with row","easting increases with column"],"native_coordinates":{"edge_origin":[-17367530.4451615,7314540.8306386],"spacing":9008.055210146,"formula":"x=edge_x+(column+0.5)*spacing; y=edge_y-(row+0.5)*spacing","authority":"https://github.com/nsidc/mapxmaps/blob/main/EASE2_M09km.gpd"},"geographic_selection":"inverse EPSG:6933 at original centers; closed WGS84 polygon","unsupported":["EPSG:6931 polar SMAP grids","resampling","reprojection","AM/PM averaging"]});
        }
        serde_json::json!({"model":self,"dimension_order":["y","x"],"registration":"native pixel centers","stored_axes":"original codes and packing retained; navigation applies packing explicitly","geographic_selection":"inverse navigation of each native center, closed WGS84 polygon; space excluded","unsupported":["reprojection","resampling","antimeridian wrapping","other ABI projection parameters"]})
    }
}
/// Exact EPSG:6933 ellipsoidal cylindrical equal-area inverse. The
/// geodetic latitude is solved from authalic q, independently checked with PROJ.
fn ease_inverse(y: f64, x: f64) -> Option<[f64; 2]> {
    let a = 6378137.;
    let e2: f64 = 0.0066943799901413165;
    let e = e2.sqrt();
    let k0 = 30f64.to_radians().cos() / (1. - e2 * 0.25).sqrt();
    let target = 2. * y * k0 / a;
    let q = |lat: f64| {
        let s = lat.sin();
        (1. - e2) * (s / (1. - e2 * s * s) - ((1. - e * s) / (1. + e * s)).ln() / (2. * e))
    };
    let pole = q(std::f64::consts::FRAC_PI_2);
    if !x.is_finite() || !y.is_finite() || target.abs() > pole {
        return None;
    }
    let mut lat = (target / pole).asin();
    for _ in 0..12 {
        let s = lat.sin();
        let derivative = 2. * (1. - e2) * lat.cos() / (1. - e2 * s * s).powi(2);
        if derivative.abs() < 1e-15 {
            break;
        }
        let delta = (q(lat) - target) / derivative;
        lat -= delta;
        if delta.abs() < 1e-14 {
            break;
        }
    }
    let lon = (x / (a * k0)).to_degrees();
    (lon.abs() <= 180.000000001 && lat.is_finite()).then_some([lon, lat.to_degrees()])
}
pub(crate) fn ease_mapping() -> Value {
    let mut attrs = serde_json::json!({});
    for (k, v) in [
        (
            "grid_mapping_name",
            serde_json::json!("lambert_cylindrical_equal_area"),
        ),
        ("standard_parallel", serde_json::json!(30.)),
        ("longitude_of_central_meridian", serde_json::json!(0.)),
        ("false_easting", serde_json::json!(0.)),
        ("false_northing", serde_json::json!(0.)),
        ("semi_major_axis", serde_json::json!(6378137.)),
        ("inverse_flattening", serde_json::json!(298.257223563)),
        ("epsg_code", serde_json::json!("EPSG:6933")),
    ] {
        attrs[k] = serde_json::json!({"value":v});
    }
    attrs["crs_wkt"] = serde_json::json!({"value":include_str!("ease6933.wkt").trim()});
    serde_json::json!({"path":"official_EASE2_M09_definition","origin":"NSIDC official grid specification; not an HDF5 mapping dataset","attributes":attrs})
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
    let separable = if matches!(grid, NativeGrid::EaseGridGlobal { .. }) {
        Some((
            ys.iter()
                .map(|y| grid.longitude_latitude(*y, 0.).map(|v| v[1]))
                .collect::<Vec<_>>(),
            xs.iter()
                .map(|x| grid.longitude_latitude(0., *x).map(|v| v[0]))
                .collect::<Vec<_>>(),
        ))
    } else {
        None
    };
    let mut min = [usize::MAX; 2];
    let mut max = [0; 2];
    for (r, y) in ys.iter().enumerate() {
        for (c, x) in xs.iter().enumerate() {
            let xy = if let Some((lat, lon)) = &separable {
                lat[r].zip(lon[c]).map(|(y, x)| [x, y])
            } else {
                grid.longitude_latitude(*y, *x)
            };
            if inclusion.accepts(xy) {
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
