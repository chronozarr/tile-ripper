//! Concrete native navigation for the separately admitted array profiles.
use crate::{fail, Result, Spatial};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "coordinate_model", rename_all = "snake_case")]
pub enum NativeGrid {
    NisarUtm16North,
    EaseGridNorth {
        epsg: u32,
    },
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
            Self::NisarUtm16North => {
                let native = proj4rs::Proj::from_proj_string(
                    "+proj=utm +zone=16 +datum=WGS84 +units=m +no_defs",
                )
                .ok()?;
                let geographic =
                    proj4rs::Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs").ok()?;
                let mut p = (x, y, 0.);
                proj4rs::transform::transform(&native, &geographic, &mut p).ok()?;
                (p.0.is_finite() && p.1.is_finite()).then_some([p.0.to_degrees(), p.1.to_degrees()])
            }
            Self::EaseGridNorth { epsg: 6931 } => ease_north_inverse(y, x),
            Self::EaseGridNorth { .. } => None,
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
        if matches!(self, Self::NisarUtm16North) {
            return serde_json::json!({"model":self,"crs":"EPSG:32616","axis_units":"metres","dimension_order":["y","x"],"registration":"original Float64 source pixel centers","geographic_navigation":"qualified UTM inverse of actual stored centers","selection":"WGS84 bounds/simple polygons or explicit native indices","geographic_limits":"existing UTM AOI limits: <=5 degrees per axis, latitude [-80,80], native zone vicinity and hemisphere; inspected 10m/20m axes within easting [300000,800000], northing [3000000,5000000]","unsupported":["other NISAR CRS/grids","complex/off-diagonal covariance","decibel conversion","reprojection"]});
        }
        if matches!(self, Self::EaseGridNorth { .. }) {
            return serde_json::json!({"model":self,"dimension_order":["y","x"],"registration":"native pixel centers","axis_units":"metres","native_coordinates":{"edge_origin":[-9000000.,9000000.],"spacing":9000.,"shape":[2000,2000],"formula":"x=-9000000+(column+0.5)*9000; y=9000000-(row+0.5)*9000","authority":"https://github.com/nsidc/mapxmaps/blob/main/EASE2_N09km.gpd"},"geographic_selection":"inverse ellipsoidal LAEA at actual centers; WGS84 closed polygons; northern hemisphere AOIs; no longitude wrapping","validity":"projection position does not imply source missingness or quality; square-grid corners may navigate south of equator","unsupported":["southern polar product","antimeridian-wrapping AOIs","resampling","AM/PM averaging"]});
        }
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
/// EPSG:6931 polar ellipsoidal LAEA. Authalic latitude is inverted on WGS84;
/// the mathematical projection domain is distinct from product missingness.
fn ease_north_inverse(y: f64, x: f64) -> Option<[f64; 2]> {
    let a = 6378137.;
    let e2: f64 = 0.0066943799901413165;
    let e = e2.sqrt();
    let q = |lat: f64| {
        let s = lat.sin();
        (1. - e2) * (s / (1. - e2 * s * s) - ((1. - e * s) / (1. + e * s)).ln() / (2. * e))
    };
    let pole = q(std::f64::consts::FRAC_PI_2);
    let target = pole - (x * x + y * y) / (a * a);
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
    let lon = if x == 0. && y == 0. {
        0.
    } else {
        x.atan2(-y).to_degrees()
    };
    lat.is_finite().then_some([lon, lat.to_degrees()])
}
pub(crate) fn ease_mapping(north: bool) -> Value {
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
    if north {
        attrs["grid_mapping_name"] = serde_json::json!({"value":"lambert_azimuthal_equal_area"});
        attrs.as_object_mut().unwrap().remove("standard_parallel");
        attrs
            .as_object_mut()
            .unwrap()
            .remove("longitude_of_central_meridian");
        attrs["latitude_of_projection_origin"] = serde_json::json!({"value":90.});
        attrs["longitude_of_projection_origin"] = serde_json::json!({"value":0.});
        attrs["epsg_code"] = serde_json::json!({"value":"EPSG:6931"});
    }
    attrs["crs_wkt"] = serde_json::json!({"value":if north {include_str!("ease6931.wkt").trim()}else{include_str!("ease6933.wkt").trim()}});
    serde_json::json!({"path":if north {"official_EASE2_N09_definition"}else{"official_EASE2_M09_definition"},"origin":"NSIDC official grid specification; not an HDF5 mapping dataset","attributes":attrs})
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
    utm: Option<crate::swot_aoi::Evaluator>,
    tolerance: f64,
}
impl Inclusion {
    pub(crate) fn geometry_bounds(&self) -> Option<geo::Rect<f64>> {
        use geo::BoundingRect;
        self.geometry.as_ref().and_then(|g| g.bounding_rect())
    }
    pub(crate) fn new(aoi: &Spatial) -> Result<Self> {
        let geometry = match aoi {
            Spatial::ArraySlice {
                start, count, step, ..
            } => {
                if count.contains(&0)
                    || step.contains(&0)
                    || count.iter().any(|n| *n > 65536)
                    || (0..2).any(|i| {
                        (count[i] - 1)
                            .checked_mul(step[i])
                            .and_then(|n| n.checked_add(start[i]))
                            .is_none()
                    })
                {
                    return fail("native selection requires positive bounded counts/strides without index overflow");
                }
                None
            }
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
        Ok(Self {
            geometry,
            utm: None,
            tolerance: 0.,
        })
    }
    pub(crate) fn for_grid(aoi: &Spatial, grid: &NativeGrid) -> Result<Self> {
        let mut inclusion = Self::new(aoi)?;
        if matches!(grid, NativeGrid::EaseGridNorth { epsg: 6931 }) {
            inclusion.tolerance = crate::swot_aoi::COORDINATE_TOLERANCE_DEGREES;
        }
        if matches!(grid, NativeGrid::NisarUtm16North) && inclusion.geometry.is_some() {
            inclusion.utm = Some(crate::swot_aoi::Evaluator::new(aoi, "EPSG:32616")?);
        }
        Ok(inclusion)
    }
    pub(crate) fn native_accepts(&self, grid: &NativeGrid, y: f64, x: f64) -> Result<bool> {
        if let Some(utm) = &self.utm {
            Ok(utm.contains(x, y)? != 0)
        } else {
            Ok(self.accepts(grid.longitude_latitude(y, x)))
        }
    }
    pub(crate) fn accepts(&self, xy: Option<[f64; 2]>) -> bool {
        use geo::Intersects;
        match &self.geometry {
            None => true,
            Some(g) => xy.is_some_and(|[x, y]| {
                g.intersects(&geo::Point::new(x, y))
                    || (self.tolerance > 0.
                        && std::iter::once(g.exterior()).chain(g.interiors()).any(|r| {
                            r.0.windows(2).any(|e| {
                                crate::swot_aoi::distance_squared(geo::Coord { x, y }, e[0], e[1])
                                    <= self.tolerance.powi(2)
                            })
                        }))
            }),
        }
    }
}
/// A descriptive selection is not source authorization. Product preparation
/// must validate actual metadata, axes and version before executing this window.
#[derive(Clone, Debug, Serialize)]
pub struct ResolvedSelection {
    pub requested_aoi: Spatial,
    pub native_selection: Spatial,
    pub included_centers: usize,
    pub membership: &'static str,
}
pub fn resolve(
    grid: &NativeGrid,
    ys: &[f64],
    xs: &[f64],
    aoi: &Spatial,
    axes: [String; 2],
) -> Result<ResolvedSelection> {
    let finish = |native_selection, included_centers| ResolvedSelection {
        requested_aoi: aoi.clone(),
        native_selection,
        included_centers,
        membership: if matches!(
            grid,
            NativeGrid::NisarUtm16North | NativeGrid::EaseGridNorth { epsg: 6931 }
        ) {
            "source-global centers in original WGS84 polygon; closed outer/hole boundaries; 1e-10 degree roundoff tolerance"
        } else {
            "source-global visible centers in original closed WGS84 polygon"
        },
    };
    if let Spatial::ArraySlice {
        axes: requested, ..
    } = aoi
    {
        if requested != &axes {
            return fail("native selection axes conflict with product");
        }
        Inclusion::new(aoi)?;
        let Spatial::ArraySlice {
            start, count, step, ..
        } = aoi
        else {
            unreachable!()
        };
        if (0..2).any(|i| start[i] + (count[i] - 1) * step[i] >= [ys.len(), xs.len()][i]) {
            return fail("native selection outside actual source axes");
        }
        return Ok(finish(aoi.clone(), count[0] * count[1]));
    }
    if matches!(grid, NativeGrid::NisarUtm16North) {
        return resolve_nisar(ys, xs, aoi, axes);
    }
    let inclusion = Inclusion::for_grid(aoi, grid)?;
    let vertices = inclusion.geometry.as_ref().map_or(0, |g| {
        g.exterior().0.len() + g.interiors().iter().map(|r| r.0.len()).sum::<usize>()
    });
    if ys
        .len()
        .checked_mul(xs.len())
        .and_then(|n| n.checked_mul(vertices))
        .is_none_or(|n| n > 256_000_000)
    {
        return fail(
            "geographic selection exceeds 256 million native-center/vertex tests; simplify AOI",
        );
    }
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
    let mut included = 0;
    for (r, y) in ys.iter().enumerate() {
        for (c, x) in xs.iter().enumerate() {
            let xy = if let Some((lat, lon)) = &separable {
                lat[r].zip(lon[c]).map(|(y, x)| [x, y])
            } else {
                grid.longitude_latitude(*y, *x)
            };
            if inclusion.accepts(xy) {
                included += 1;
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
    Ok(finish(
        Spatial::ArraySlice {
            start: min,
            count: [max[0] - min[0] + 1, max[1] - min[1] + 1],
            step: [1, 1],
            axes,
        },
        included,
    ))
}

/// Concrete UTM16N accelerator. On each constant-northing row in this
/// qualified central-zone envelope, inverse longitude increases with easting;
/// latitude is maximal at the central meridian and minimal at an endpoint.
/// These extrema conservatively reject rows. Binary search bounds longitude
/// on ACTUAL centers. No forward polygon approximation prunes source centers.
/// Every remaining center is tested by the existing UTM polygon evaluator.
fn resolve_nisar(
    ys: &[f64],
    xs: &[f64],
    aoi: &Spatial,
    axes: [String; 2],
) -> Result<ResolvedSelection> {
    use geo::BoundingRect;
    if ys.len() < 2
        || xs.len() < 2
        || ys.len() > 65536
        || xs.len() > 65536
        || ys
            .iter()
            .any(|v| !v.is_finite() || !(3_000_000. ..=5_000_000.).contains(v))
        || xs
            .iter()
            .any(|v| !v.is_finite() || !(300_000. ..=800_000.).contains(v))
        || ![10., 20.].contains(&(xs[1] - xs[0]))
        || ys.windows(2).any(|w| w[1] - w[0] != -(xs[1] - xs[0]))
        || xs.windows(2).any(|w| w[1] - w[0] != xs[1] - xs[0])
    {
        return fail("NISAR geographic selection requires inspected descending/ascending 10m or 20m center axes in qualified UTM16N envelope");
    }
    let inclusion = Inclusion::for_grid(aoi, &NativeGrid::NisarUtm16North)?;
    let geometry = inclusion
        .geometry
        .as_ref()
        .ok_or("geographic geometry missing")?;
    let bbox = geometry.bounding_rect().ok_or("empty AOI")?;
    let vertices = geometry.exterior().0.len()
        + geometry
            .interiors()
            .iter()
            .map(|r| r.0.len())
            .sum::<usize>();
    let evaluator = inclusion.utm.as_ref().ok_or("missing UTM evaluator")?;
    let tolerance = crate::swot_aoi::COORDINATE_TOLERANCE_DEGREES;
    let west = bbox.min().x - tolerance;
    let east = bbox.max().x + tolerance;
    let south = bbox.min().y - tolerance;
    let north = bbox.max().y + tolerance;
    let mut first = [usize::MAX; 2];
    let mut last = [0; 2];
    let mut included = 0usize;
    let mut tests = 0usize;
    let mut rows = Vec::new(); // <=65536 intervals, within product geometry reservation
    for (row, y) in ys.iter().copied().enumerate() {
        let left = evaluator.longitude_latitude(xs[0], y)?;
        let right = evaluator.longitude_latitude(xs[xs.len() - 1], y)?;
        let peak = evaluator.longitude_latitude(500_000f64.clamp(xs[0], xs[xs.len() - 1]), y)?;
        if left[1].min(right[1]) > north || peak[1] < south || right[0] < west || left[0] > east {
            continue;
        }
        let bound = |longitude: f64, inclusive: bool| -> Result<usize> {
            let (mut lo, mut hi) = (0, xs.len());
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let lon = evaluator.longitude_latitude(xs[mid], y)?[0];
                if lon < longitude || (inclusive && lon == longitude) {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            Ok(lo)
        };
        let begin = bound(west, false)?;
        let end = bound(east, true)?;
        tests = tests
            .checked_add((end - begin) * vertices)
            .ok_or("NISAR geographic work overflow")?;
        if tests > 256_000_000 {
            return fail("geographic selection exceeds 256 million native-center/vertex tests; narrow or simplify AOI");
        }
        rows.push((row, begin, end));
    }
    for (row, begin, end) in rows {
        let y = ys[row];
        for (col, x) in xs.iter().enumerate().take(end).skip(begin) {
            if evaluator.contains(*x, y)? != 0 {
                included += 1;
                first[0] = first[0].min(row);
                first[1] = first[1].min(col);
                last[0] = last[0].max(row);
                last[1] = last[1].max(col);
            }
        }
    }
    if included == 0 {
        return fail(
            "geographic AOI includes zero native centers (science availability not evaluated)",
        );
    }
    Ok(ResolvedSelection {
        requested_aoi: aoi.clone(),
        native_selection: Spatial::ArraySlice { start: first, count: [last[0]-first[0]+1,last[1]-first[1]+1], step: [1,1], axes },
        included_centers: included,
        membership: "source-global centers in original WGS84 polygon; closed outer/hole boundaries; 1e-10 degree roundoff tolerance",
    })
}

/// Native center membership without changing values or source validity.
pub fn native_aoi_mask(
    aoi: &Spatial,
    grid: &NativeGrid,
    ys: &[f64],
    xs: &[f64],
) -> Result<Vec<u8>> {
    let inclusion = Inclusion::for_grid(aoi, grid)?;
    let count = ys
        .len()
        .checked_mul(xs.len())
        .filter(|n| *n <= 1_048_576)
        .ok_or("AOI chunk exceeds one million native centers")?;
    if matches!(aoi, Spatial::ArraySlice { .. }) {
        return Ok(vec![1; count]);
    }
    let mut mask = Vec::with_capacity(count);
    for y in ys {
        for x in xs {
            mask.push(u8::from(inclusion.native_accepts(grid, *y, *x)?));
        }
    }
    Ok(mask)
}
