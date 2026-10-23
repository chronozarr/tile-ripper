//! Geographic selection for the qualified rectilinear 100 m SWOT Raster D grid.
//!
//! Membership is evaluated in the original longitude/latitude polygon by
//! inverse-transforming actual stored centers. Forward densification is used
//! for coverage diagnostics, never to prune centers or to round array indices.
//! This adapter uses normal native preparation without changing the HDF5 reader.
use crate::{acquisition, fail, swot_job::Observation, Result, Spatial, Variable};
use geo::{
    Area, BooleanOps, BoundingRect, Coord, Intersects, LineString, Point, Polygon, Validation,
};
use proj4rs::Proj;
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Instant;

pub const POLICY: &str = "swot-wgs84-native-centers-v1";
/// Closed polygon, including hole boundaries, with this longitude/latitude
/// distance tolerance (about 11 micrometres of latitude). No all-touched rule.
pub const COORDINATE_TOLERANCE_DEGREES: f64 = 1e-10;
pub const EDGE_ERROR_METRES: f64 = 0.001;
pub const MAX_GRID_CENTERS: usize = 16_777_216;
const MAX_AXIS: usize = 8192;
const MAX_INPUT_VERTICES: usize = 512;
const MAX_MAPPED_VERTICES: usize = 100_000;
const MAX_CENTER_EDGE_TESTS: usize = 256_000_000;
const GEOMETRY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeWindow {
    pub start: [usize; 2],
    pub count: [usize; 2],
    pub step: [usize; 2],
}
impl NativeWindow {
    pub fn spatial(&self) -> Spatial {
        Spatial::ArraySlice {
            start: self.start,
            count: self.count,
            step: self.step,
            axes: ["/y".into(), "/x".into()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridIdentity {
    pub crs: String,
    pub shape: [usize; 2],
    /// Row/column order; signs describe the actual stored arrays.
    pub direction: [String; 2],
    pub coordinate_sha256: [String; 2],
    pub mapping_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolved {
    pub policy: String,
    pub aoi: Spatial,
    pub grid: GridIdentity,
    pub window: NativeWindow,
    pub included_centers: usize,
    pub selected_coordinate_sha256: [String; 2],
    pub mask_sha256: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&SHA256, bytes).as_ref())
}
fn coordinate_hash(values: &[f64]) -> String {
    let mut ctx = Context::new(&SHA256);
    for value in values {
        ctx.update(&value.to_le_bytes());
    }
    hex(ctx.finish().as_ref())
}
fn shape_bound(shape: [usize; 2]) -> Result<()> {
    if shape.iter().any(|n| !(2..=MAX_AXIS).contains(n))
        || shape[0]
            .checked_mul(shape[1])
            .is_none_or(|n| n > MAX_GRID_CENTERS)
    {
        return fail(
            "AOI inspection requires 2..8192 centers per axis and at most 16777216 source centers",
        );
    }
    Ok(())
}
fn axis(values: &[f64]) -> Result<String> {
    if values.len() < 2 || values.iter().any(|x| !x.is_finite()) {
        return fail("AOI requires finite stored one-dimensional x/y axes");
    }
    let delta = values[1] - values[0];
    if values
        .windows(2)
        .any(|p| ((p[1] - p[0]) - delta).abs() > 1e-6)
        || (delta.abs() - 100.).abs() > 1e-6
    {
        return fail("AOI profile requires monotonic uniform 100 m axes; nonuniform, duplicate and nonlinear grids are unsupported");
    }
    Ok(if delta > 0. {
        "increasing"
    } else {
        "decreasing"
    }
    .into())
}
fn value<'a>(attrs: &'a Value, key: &str) -> &'a Value {
    &attrs[key]["value"]
}

/// Exact canonical source WKT encoding qualified here. Equivalent alternate
/// WKT, custom projections, datum transformations and axis/unit overrides fail.
pub fn canonical_wkt(zone: u32, south: bool) -> String {
    let hemi = if south { "S" } else { "N" };
    let meridian = zone as i32 * 6 - 183;
    let northing = if south { 10000000 } else { 0 };
    let epsg = if south { 32700 } else { 32600 } + zone;
    format!("PROJCS[\"WGS 84 / UTM zone {zone}{hemi}\",GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563,AUTHORITY[\"EPSG\",\"7030\"]],AUTHORITY[\"EPSG\",\"6326\"]],PRIMEM[\"Greenwich\",0,AUTHORITY[\"EPSG\",\"8901\"]],UNIT[\"degree\",0.0174532925199433,AUTHORITY[\"EPSG\",\"9122\"]],AUTHORITY[\"EPSG\",\"4326\"]],PROJECTION[\"Transverse_Mercator\"],PARAMETER[\"latitude_of_origin\",0],PARAMETER[\"central_meridian\",{meridian}],PARAMETER[\"scale_factor\",0.9996],PARAMETER[\"false_easting\",500000],PARAMETER[\"false_northing\",{northing}],UNIT[\"metre\",1,AUTHORITY[\"EPSG\",\"9001\"]],AXIS[\"Easting\",EAST],AXIS[\"Northing\",NORTH],AUTHORITY[\"EPSG\",\"{epsg}\"]]")
}
fn utm(crs: &str) -> Result<(u32, bool)> {
    let code: u32 = crs
        .strip_prefix("EPSG:")
        .ok_or("explicit UTM EPSG CRS required")?
        .parse()?;
    if crs != format!("EPSG:{code}") {
        return fail("noncanonical EPSG CRS");
    }
    match code {
        32601..=32660 => Ok((code - 32600, false)),
        32701..=32760 => Ok((code - 32700, true)),
        _ => fail("only WGS84 UTM CRS is supported for SWOT AOIs"),
    }
}
fn source_crs(definition: &Value) -> Result<String> {
    let mapping = &definition["grid_mapping"];
    let a = &mapping["attributes"];
    let meridian = value(a, "longitude_of_central_meridian")
        .as_f64()
        .ok_or("source lacks central meridian")?;
    let zone = (meridian + 183.) / 6.;
    let northing = value(a, "false_northing")
        .as_f64()
        .ok_or("source lacks false northing")?;
    let south = northing == 10000000.;
    if !(1.0..=60.0).contains(&zone) || zone.fract() != 0. || ![0., 10000000.].contains(&northing) {
        return fail("unsupported source UTM parameters");
    }
    for (key, expected) in [
        ("grid_mapping_name", json!("transverse_mercator")),
        ("geographic_crs_name", json!("WGS 84")),
        ("horizontal_datum_name", json!("WGS_1984")),
        ("false_easting", json!(500000.)),
        ("latitude_of_projection_origin", json!(0.)),
        ("longitude_of_prime_meridian", json!(0.)),
        ("scale_factor_at_central_meridian", json!(0.9996)),
        ("semi_major_axis", json!(6378137.)),
        ("inverse_flattening", json!(298.257223563)),
    ] {
        if *value(a, key) != expected {
            return fail("unsupported or conflicting source CF CRS parameters");
        }
    }
    let zone = zone as u32;
    let wkt = canonical_wkt(zone, south);
    if mapping["path"] != "/crs"
        || value(a, "crs_wkt").as_str() != Some(&wkt)
        || value(a, "spatial_ref").as_str() != Some(&wkt)
    {
        return fail(
            "source CRS must use the qualified canonical WGS84 UTM WKT and matching CF parameters",
        );
    }
    let axes = definition["axes"].as_array().ok_or("source lacks axes")?;
    if axes.len() != 2
        || definition["root_attributes"]["crid"]["value"] != "PGD0"
        || !definition["root_attributes"]["descriptor_string"]["value"]
            .as_str()
            .is_some_and(|s| s.starts_with("100m_"))
    {
        return fail("AOI selection starts with the 100 m SWOT native profile");
    }
    for (axis, (name, standard)) in axes.iter().zip([
        ("/y", "projection_y_coordinate"),
        ("/x", "projection_x_coordinate"),
    ]) {
        let a = &axis["attributes"];
        if axis["path"] != name
            || value(a, "units") != "m"
            || value(a, "standard_name") != standard
            || a.get("scale_factor")
                .is_some_and(|_| value(a, "scale_factor").as_f64() != Some(1.))
            || a.get("add_offset")
                .is_some_and(|_| value(a, "add_offset").as_f64() != Some(0.))
        {
            return fail(
                "AOI requires explicit unpacked projected x/y center coordinates in metres",
            );
        }
    }
    Ok(format!("EPSG:{}", if south { 32700 } else { 32600 } + zone))
}
fn definition(inspection: &Value) -> Result<&Value> {
    inspection["sources"][0]["object_identities"]["datasets"]
        .as_array()
        .and_then(|ds| ds.iter().find(|d| d["path"] == "/wse"))
        .ok_or_else(|| "missing inspected WSE grid".into())
}

pub fn polygon(aoi: &Spatial) -> Result<Polygon<f64>> {
    let coordinates = match aoi {
        Spatial::Bounds {
            crs,
            bbox: [w, s, e, n],
        } if crs == "EPSG:4326" && w < e && s < n => {
            json!([[[w, s], [e, s], [e, n], [w, n], [w, s]]])
        }
        Spatial::Polygon { crs, coordinates } if crs == "EPSG:4326" => coordinates.clone(),
        _ => return fail(
            "AOI must be explicit EPSG:4326 longitude/latitude bounds or a simple GeoJSON Polygon",
        ),
    };
    let rings = coordinates.as_array().ok_or("AOI polygon needs rings")?;
    if rings.is_empty() {
        return fail("AOI polygon has no outer ring");
    }
    let mut count = 0usize;
    let mut parsed = Vec::new();
    for ring in rings {
        let positions = ring.as_array().ok_or("invalid AOI ring")?;
        count += positions.len();
        if positions.len() < 4 || count > MAX_INPUT_VERTICES {
            return fail(
                "AOI rings need at least four closed positions; at most 512 total vertices",
            );
        }
        let mut points = Vec::new();
        for p in positions {
            let p = p
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or("AOI positions must be 2D longitude/latitude")?;
            let x = p[0]
                .as_f64()
                .filter(|x| x.is_finite())
                .ok_or("nonfinite longitude")?;
            let y = p[1]
                .as_f64()
                .filter(|x| x.is_finite())
                .ok_or("nonfinite latitude")?;
            points.push(Coord { x, y });
        }
        if points.first() != points.last() {
            return fail("AOI rings must be explicitly closed");
        }
        parsed.push(LineString::new(points));
    }
    let p = Polygon::new(parsed.remove(0), parsed);
    if !p.is_valid() || p.unsigned_area() <= 0. {
        return fail(
            "invalid AOI topology: simple nonzero outer ring and valid disjoint holes required",
        );
    }
    let b = p.bounding_rect().ok_or("empty AOI")?;
    if b.min().x <= -180.
        || b.max().x >= 180.
        || b.min().y < -80.
        || b.max().y > 80.
        || b.width() > 5.
        || b.height() > 5.
    {
        return fail("AOI domain is longitude strictly inside (-180,180), latitude [-80,80], at most 5 degrees per axis; touching/crossing the antimeridian and polar AOIs are unsupported");
    }
    Ok(p)
}

pub struct Evaluator {
    polygon: Polygon<f64>,
    geographic: Proj,
    native: Proj,
}
impl Evaluator {
    pub fn new(aoi: &Spatial, crs: &str) -> Result<Self> {
        let polygon = polygon(aoi)?;
        let (zone, south) = utm(crs)?;
        let b = polygon.bounding_rect().ok_or("empty AOI")?;
        let center = zone as f64 * 6. - 183.;
        if b.min().x < center - 6.
            || b.max().x > center + 6.
            || (!south && b.min().y < 0.)
            || (south && b.max().y > 0.)
        {
            return fail("AOI is outside the native UTM zone vicinity or hemisphere; split the request explicitly");
        }
        Ok(Self {
            polygon,
            geographic: Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs")?,
            native: Proj::from_proj_string(&format!(
                "+proj=utm +zone={zone} {} +datum=WGS84 +units=m +no_defs",
                if south { "+south" } else { "" }
            ))?,
        })
    }
    pub fn project(&self, longitude: f64, latitude: f64) -> Result<[f64; 2]> {
        let mut p = (longitude.to_radians(), latitude.to_radians(), 0.);
        proj4rs::transform::transform(&self.geographic, &self.native, &mut p)?;
        Ok([p.0, p.1])
    }
    pub fn contains(&self, x: f64, y: f64) -> Result<u8> {
        let mut p = (x, y, 0.);
        proj4rs::transform::transform(&self.native, &self.geographic, &mut p)?;
        let p = Coord {
            x: p.0.to_degrees(),
            y: p.1.to_degrees(),
        };
        if !p.x.is_finite() || !p.y.is_finite() {
            return fail("nonfinite inverse native center transformation");
        }
        if self.polygon.intersects(&Point(p)) {
            return Ok(1);
        }
        // Include closed outer AND hole boundaries within the explicit roundoff
        // band; no pixel-size tolerance and no window-dependent rasterization.
        for ring in std::iter::once(self.polygon.exterior()).chain(self.polygon.interiors()) {
            for edge in ring.0.windows(2) {
                if distance_squared(p, edge[0], edge[1]) <= COORDINATE_TOLERANCE_DEGREES.powi(2) {
                    return Ok(1);
                }
            }
        }
        Ok(0)
    }
    fn projected_polygon(&self) -> Result<Polygon<f64>> {
        let mut total = 0;
        let mut rings = Vec::new();
        for ring in std::iter::once(self.polygon.exterior()).chain(self.polygon.interiors()) {
            let mut points = Vec::new();
            for edge in ring.0.windows(2) {
                self.edge(
                    edge[0],
                    edge[1],
                    self.project(edge[0].x, edge[0].y)?,
                    self.project(edge[1].x, edge[1].y)?,
                    0,
                    &mut points,
                )?;
            }
            points.push(points[0]);
            total += points.len();
            if total > MAX_MAPPED_VERTICES {
                return fail("densified AOI exceeds 100000 vertices; narrow or simplify it");
            }
            rings.push(LineString::new(points));
        }
        let p = Polygon::new(rings.remove(0), rings);
        if !p.is_valid() || p.unsigned_area() <= 0. {
            return fail("transformed AOI has invalid topology");
        }
        Ok(p)
    }
    #[allow(clippy::too_many_arguments)]
    fn edge(
        &self,
        a: Coord<f64>,
        b: Coord<f64>,
        pa: [f64; 2],
        pb: [f64; 2],
        depth: usize,
        out: &mut Vec<Coord<f64>>,
    ) -> Result<()> {
        if depth > 24 || out.len() >= MAX_MAPPED_VERTICES {
            return fail("geographic edge transformation exceeds subdivision limits");
        }
        let mid = Coord {
            x: (a.x + b.x) * 0.5,
            y: (a.y + b.y) * 0.5,
        };
        let pm = self.project(mid.x, mid.y)?;
        let error = ((pm[0] - (pa[0] + pb[0]) * 0.5).powi(2)
            + (pm[1] - (pa[1] + pb[1]) * 0.5).powi(2))
        .sqrt();
        if (a.x - b.x).abs().max((a.y - b.y).abs()) > 0.001 || error > EDGE_ERROR_METRES {
            self.edge(a, mid, pa, pm, depth + 1, out)?;
            self.edge(mid, b, pm, pb, depth + 1, out)?;
        } else {
            out.push(Coord { x: pa[0], y: pa[1] });
        }
        Ok(())
    }
}
fn distance_squared(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let d = dx * dx + dy * dy;
    let t = if d == 0. {
        0.
    } else {
        ((p.x - a.x) * dx + (p.y - a.y) * dy) / d
    }
    .clamp(0., 1.);
    (p.x - a.x - t * dx).powi(2) + (p.y - a.y - t * dy).powi(2)
}

/// Pure grid oracle, also used by independent acceptance tooling. Axes are in
/// source-global row/column order, never window-local affine coordinates.
pub fn resolve_grid(
    aoi: &Spatial,
    crs: &str,
    y: &[f64],
    x: &[f64],
    mapping: &Value,
) -> Result<(Option<Resolved>, Value)> {
    let started = Instant::now();
    shape_bound([y.len(), x.len()])?;
    let direction = [axis(y)?, axis(x)?];
    let evaluator = Evaluator::new(aoi, crs)?;
    let vertices = std::iter::once(evaluator.polygon.exterior())
        .chain(evaluator.polygon.interiors())
        .map(|ring| ring.0.len())
        .sum::<usize>();
    if (y.len() * x.len())
        .checked_mul(vertices)
        .is_none_or(|n| n > MAX_CENTER_EDGE_TESTS)
    {
        return fail("AOI resolution exceeds 256000000 source-center/vertex tests; narrow or simplify the AOI");
    }
    let projected = evaluator.projected_polygon()?;
    let projected_bounds = projected.bounding_rect().ok_or("empty projected AOI")?;
    let x0 = x[0].min(x[x.len() - 1]) - 50.;
    let x1 = x[0].max(x[x.len() - 1]) + 50.;
    let y0 = y[0].min(y[y.len() - 1]) - 50.;
    let y1 = y[0].max(y[y.len() - 1]) + 50.;
    let domain = Polygon::new(
        LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]),
        vec![],
    );
    let overlap = projected.intersection(&domain).unsigned_area();
    let mut first = [usize::MAX; 2];
    let mut last = [0; 2];
    let mut included = 0;
    for (row, yy) in y.iter().enumerate() {
        for (col, xx) in x.iter().enumerate() {
            if evaluator.contains(*xx, *yy)? == 1 {
                included += 1;
                first[0] = first[0].min(row);
                first[1] = first[1].min(col);
                last[0] = last[0].max(row);
                last[1] = last[1].max(col);
            }
        }
    }
    let grid = GridIdentity {
        crs: crs.into(),
        shape: [y.len(), x.len()],
        direction,
        coordinate_sha256: [coordinate_hash(y), coordinate_hash(x)],
        mapping_sha256: hash(&serde_json::to_vec(mapping)?),
    };
    let resolved = if included == 0 {
        None
    } else {
        let window = NativeWindow {
            start: first,
            count: [last[0] - first[0] + 1, last[1] - first[1] + 1],
            step: [1, 1],
        };
        let ys = &y[first[0]..=last[0]];
        let xs = &x[first[1]..=last[1]];
        let (n, digest) = mask_digest(&evaluator, ys, xs)?;
        if n != included {
            return fail("source-global AOI count changed during resolution");
        }
        Some(Resolved {
            policy: POLICY.into(),
            aoi: aoi.clone(),
            grid: grid.clone(),
            window,
            included_centers: included,
            selected_coordinate_sha256: [coordinate_hash(ys), coordinate_hash(xs)],
            mask_sha256: digest,
        })
    };
    let report = json!({"policy":POLICY,"native_crs":crs,"grid":grid,"included_native_centers":included,
        "native_centers_intersect":included>0,"native_grid_domain_intersects_aoi":projected.intersects(&domain),
        "window":resolved.as_ref().map(|r|&r.window),"geographic_aoi":aoi,
        "coverage":{"projected_aoi_bounds":[projected_bounds.min().x,projected_bounds.min().y,projected_bounds.max().x,projected_bounds.max().y],
            "projected_aoi_area_m2":projected.unsigned_area(),"native_grid_overlap_area_m2":overlap,
            "fraction_of_projected_aoi_in_grid":overlap/projected.unsigned_area(),
            "area_semantics":"diagnostic projected polygon, adaptive <=0.001 degree segments / <=1 mm midpoint error; grid extent includes half pixels; not usable WSE area"},
        "membership":"inverse transform every source-global stored center into original WGS84 polygon; closed outer and hole boundaries; no all-touched; forward approximation never prunes centers",
        "coordinate_tolerance_degrees":COORDINATE_TOLERANCE_DEGREES,"geometry_working_reservation_bytes":GEOMETRY_BYTES,
        "resolution_seconds":started.elapsed().as_secs_f64(),"quality_accepted_wse":"unknown; coordinates are independent of science and quality"});
    Ok((resolved, report))
}
fn mask_digest(evaluator: &Evaluator, y: &[f64], x: &[f64]) -> Result<(usize, String)> {
    let mut digest = Context::new(&SHA256);
    let mut count = 0;
    let mut row = Vec::with_capacity(x.len());
    for yy in y {
        row.clear();
        for xx in x {
            let b = evaluator.contains(*xx, *yy)?;
            count += usize::from(b);
            row.push(b);
        }
        digest.update(&row);
    }
    Ok((count, hex(digest.finish().as_ref())))
}
impl Resolved {
    pub(crate) fn validate_request(&self, entry: &Observation) -> Result<()> {
        polygon(&self.aoi)?;
        utm(&self.grid.crs)?;
        shape_bound(self.grid.shape)?;
        if self.policy != POLICY
            || serde_json::to_value(&entry.request.spatial)?
                != serde_json::to_value(self.window.spatial())?
            || self.window.step != [1, 1]
            || self.included_centers == 0
            || self.window.count[0]
                .checked_mul(self.window.count[1])
                .is_none_or(|n| self.included_centers > n)
            || self
                .window
                .start
                .iter()
                .zip(self.window.count)
                .zip(self.grid.shape)
                .any(|((s, c), n)| c == 0 || s.checked_add(c).is_none_or(|end| end > n))
        {
            return fail("geographic provenance conflicts with explicit resolved native selection");
        }
        let fingerprints = self
            .grid
            .coordinate_sha256
            .iter()
            .chain(&self.selected_coordinate_sha256)
            .chain([&self.grid.mapping_sha256, &self.mask_sha256]);
        if fingerprints.into_iter().any(|s| {
            s.len() != 64
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        }) || self
            .grid
            .direction
            .iter()
            .any(|d| d != "increasing" && d != "decreasing")
        {
            return fail("invalid resolved grid direction or SHA256 fingerprint");
        }
        Ok(())
    }
    pub(crate) fn validate_selected(
        &self,
        inspection: &Value,
        y: &[f64],
        x: &[f64],
    ) -> Result<Value> {
        let started = Instant::now();
        let d = definition(inspection)?;
        if source_crs(d)? != self.grid.crs
            || d["shape"] != json!(self.grid.shape)
            || hash(&serde_json::to_vec(&d["grid_mapping"])?) != self.grid.mapping_sha256
            || [coordinate_hash(y), coordinate_hash(x)] != self.selected_coordinate_sha256
            || [y.len(), x.len()] != self.window.count
        {
            return fail("source grid or selected coordinate identity conflicts with resolved geographic provenance");
        }
        let evaluator = Evaluator::new(&self.aoi, &self.grid.crs)?;
        let (count, sha) = mask_digest(&evaluator, y, x)?;
        if count != self.included_centers || sha != self.mask_sha256 {
            return fail(
                "AOI mask differs from resolved geographic identity; regenerate discover-job",
            );
        }
        Ok(
            json!({"status":"verified_against_pinned_source","policy":POLICY,"included_native_centers":count,
            "mask_sha256":sha,"validation_seconds":started.elapsed().as_secs_f64(),"geometry_working_reservation_bytes":GEOMETRY_BYTES}),
        )
    }
}

/// Inspect shape, then full stored coordinate axes using the normal bounded
/// native preparation path. Both calls read metadata/coordinates/indexes only.
/// Their versions must match; the final selected five-layer job is admitted by
/// the ordinary job planner. Full-grid plans are never used to stream science.
pub(crate) fn inspect_with(
    entry: &mut Observation,
    aoi: &Spatial,
    mut prepare: impl FnMut(&crate::Request) -> Result<acquisition::Prepared>,
) -> Result<Value> {
    polygon(aoi)?;
    let started = Instant::now();
    let mut request = entry.request.clone();
    request.variables = vec![Variable::Dataset {
        path: "/wse".into(),
    }];
    request.spatial = NativeWindow {
        start: [0, 0],
        count: [1, 1],
        step: [1, 1],
    }
    .spatial();
    let probe = prepare(&request)?;
    let initial = probe.explain();
    crate::swot_job::bounded(&initial, 256 * 1024, "AOI shape inspection")?;
    let d = definition(&initial)?;
    let crs = source_crs(d)?;
    let shape: [usize; 2] = serde_json::from_value(d["shape"].clone())?;
    shape_bound(shape)?;
    let identity = initial["sources"][0]["object_identities"]["identity"].clone();
    drop(probe);
    if let Some(etag) = identity["version"]["etag"].as_str() {
        let crate::Source::Assets { assets } = &mut request.source else {
            unreachable!()
        };
        assets[0].etag = Some(etag.into());
    }
    request.spatial = NativeWindow {
        start: [0, 0],
        count: shape,
        step: [1, 1],
    }
    .spatial();
    let full = prepare(&request)?;
    let inspection = full.explain();
    crate::swot_job::bounded(&inspection, 1024 * 1024, "AOI full-axis inspection")?;
    if inspection["sources"][0]["object_identities"]["identity"] != identity
        || source_crs(definition(&inspection)?)? != crs
    {
        return fail("native source changed between geographic shape and coordinate inspection");
    }
    let inspection_seconds = started.elapsed().as_secs_f64();
    let (resolved, mut report) = full.with_swot_coordinates(|y, x| {
        resolve_grid(aoi, &crs, y, x, &definition(&inspection)?["grid_mapping"])
    })?;
    if let Some(resolved) = resolved {
        entry.request.spatial = resolved.window.spatial();
        entry.geographic_selection = Some(resolved);
    }
    entry.request.source = request.source;
    report["metadata_coordinate_inspection_seconds"] = json!(inspection_seconds);
    report["inspected_source_identity"] = identity;
    report["inspection_transport"] =
        json!({"shape_probe":initial["transport"],"full_axes":inspection["transport"]});
    report["inspection_method"]=json!("serial one-variable shape probe then bounded full-axis native plan; no science-payload execution; final selected five-variable job prepared separately");
    Ok(report)
}

/// Resolve a local or anonymous bounded mirror observation. Authenticated
/// discovery uses the same resolver with the caller's exact-object factory.
/// A zero-center report returns None, never an executable fallback window.
pub fn inspect(mut entry: Observation, aoi: &Spatial) -> Result<(Option<Observation>, Value)> {
    if entry.geographic_selection.is_some() {
        return fail("observation is already resolved; supply a fresh draft");
    }
    let mut cache = crate::cog::LayoutCache::default();
    let report = inspect_with(&mut entry, aoi, |request| {
        acquisition::prepare(request, &mut cache)
    })?;
    Ok((
        entry.geographic_selection.is_some().then_some(entry),
        report,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn definition_fixture() -> Value {
        let mut attrs = json!({});
        for (key, v) in [
            ("grid_mapping_name", json!("transverse_mercator")),
            ("geographic_crs_name", json!("WGS 84")),
            ("horizontal_datum_name", json!("WGS_1984")),
            ("longitude_of_central_meridian", json!(-69.)),
            ("false_easting", json!(500000.)),
            ("false_northing", json!(0.)),
            ("latitude_of_projection_origin", json!(0.)),
            ("longitude_of_prime_meridian", json!(0.)),
            ("scale_factor_at_central_meridian", json!(0.9996)),
            ("semi_major_axis", json!(6378137.)),
            ("inverse_flattening", json!(298.257223563)),
            ("crs_wkt", json!(canonical_wkt(19, false))),
            ("spatial_ref", json!(canonical_wkt(19, false))),
        ] {
            attrs[key] = json!({"value":v});
        }
        json!({"grid_mapping":{"path":"/crs","attributes":attrs},
            "root_attributes":{"crid":{"value":"PGD0"},"descriptor_string":{"value":"100m_UTM19T_N_x_x_x"}},
            "axes":[{"path":"/y","attributes":{"units":{"value":"m"},"standard_name":{"value":"projection_y_coordinate"}}},
                {"path":"/x","attributes":{"units":{"value":"m"},"standard_name":{"value":"projection_x_coordinate"}}}]})
    }
    #[test]
    fn actual_crs_encoding_cf_units_and_axis_packing_must_agree() {
        let d = definition_fixture();
        assert_eq!(source_crs(&d).unwrap(), "EPSG:32619");
        for (key, v) in [
            ("crs_wkt", json!(canonical_wkt(18, false))),
            ("spatial_ref", json!("EPSG:32619")),
            ("false_easting", json!(0.)),
            ("longitude_of_central_meridian", json!(-68.)),
            ("semi_major_axis", json!(6371000.)),
            ("horizontal_datum_name", json!("NAD83")),
            ("false_northing", json!(20.)),
        ] {
            let mut bad = d.clone();
            bad["grid_mapping"]["attributes"][key]["value"] = v;
            assert!(source_crs(&bad).is_err(), "{key}");
        }
        for (key, v) in [
            ("units", json!("km")),
            ("standard_name", json!("latitude")),
            ("scale_factor", json!(2.)),
            ("add_offset", json!(1.)),
        ] {
            let mut bad = d.clone();
            bad["axes"][0]["attributes"][key] = json!({"value":v});
            assert!(source_crs(&bad).is_err(), "{key}");
        }
        let mut bad = d;
        bad["root_attributes"]["crid"]["value"] = json!("PID0");
        assert!(source_crs(&bad).is_err());
    }
}
