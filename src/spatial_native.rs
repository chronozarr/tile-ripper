//! Deliberately narrow, pure Rust Sentinel geometry support.
//!
//! Queries use WGS84 longitude/latitude. The only mapping is to WGS84 UTM
//! (EPSG:32601..32660 / 32701..32760), without raster reprojection. See
//! `mask` for the GDAL-compatible pixel-center boundary convention.
use crate::{fail, Layout, Result, Spatial, Window};
use geo::{Area, BooleanOps, BoundingRect, Coord, LineString, MultiPolygon, Polygon, Validation};
use proj4rs::Proj;
use serde_json::{json, Value};

const INPUT_LIMIT: usize = 10_000;
const MAPPED_LIMIT: usize = 100_000;

#[derive(Clone, Debug)]
pub struct Geometry {
    polygons: MultiPolygon<f64>,
    crs: String,
}
impl Geometry {
    pub fn is_empty(&self) -> bool {
        self.polygons.0.is_empty()
    }
    pub fn area(&self) -> f64 {
        self.polygons.unsigned_area()
    }
    pub fn json(&self) -> Result<String> {
        let coordinates: Vec<Value> = self.polygons.0.iter().map(polygon_json).collect();
        Ok(if coordinates.len() == 1 {
            json!({"type":"Polygon", "coordinates":coordinates[0]})
        } else {
            json!({"type":"MultiPolygon", "coordinates":coordinates})
        }
        .to_string())
    }
    /// Planar intersection in the same already-validated CRS. A disjoint or
    /// boundary-only intersection is represented by an empty polygon set.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        if self.crs != other.crs {
            return None;
        }
        let polygons = self.polygons.intersection(&other.polygons);
        (point_count(&polygons) <= MAPPED_LIMIT && polygons.is_valid()).then(|| Self {
            polygons,
            crs: self.crs.clone(),
        })
    }
}
fn polygon_json(p: &Polygon<f64>) -> Value {
    Value::Array(
        std::iter::once(p.exterior())
            .chain(p.interiors())
            .map(|r| Value::Array(r.0.iter().map(|c| json!([c.x, c.y])).collect()))
            .collect(),
    )
}
fn point_count(p: &MultiPolygon<f64>) -> usize {
    p.0.iter()
        .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
        .map(|r| r.0.len())
        .sum()
}
fn utm(crs: &str) -> Option<(u32, bool)> {
    let n = crs.strip_prefix("EPSG:")?.parse::<u32>().ok()?;
    // Accept authority strings, not aliases or alternate axis/unit semantics.
    if crs != format!("EPSG:{n}") {
        return None;
    }
    match n {
        32601..=32660 => Some((n - 32600, false)),
        32701..=32760 => Some((n - 32700, true)),
        _ => None,
    }
}
fn supported_crs(crs: &str) -> Result<()> {
    if crs != "EPSG:4326" && utm(crs).is_none() {
        return fail("native polygon geometry supports only EPSG:4326 longitude/latitude and WGS84 UTM EPSG:32601..32660 / 32701..32760");
    }
    Ok(())
}
fn parse_polygon(value: &Value, count: &mut usize, limit: usize) -> Result<Polygon<f64>> {
    let rings = value
        .as_array()
        .ok_or("polygon coordinates must be rings")?;
    if rings.is_empty() {
        return fail("polygon has no rings");
    }
    let rings: Vec<LineString<f64>> = rings
        .iter()
        .map(|value| -> Result<_> {
            let positions = value.as_array().ok_or("invalid polygon ring")?;
            if positions.len() < 4 {
                return fail("polygon rings must be closed and have at least four positions");
            }
            *count = count
                .checked_add(positions.len())
                .ok_or("AOI vertex count overflow")?;
            if *count > limit {
                return fail(&format!("polygon exceeds {limit} positions"));
            }
            let coords: Vec<Coord<f64>> = positions
                .iter()
                .map(|p| -> Result<_> {
                    let p = p.as_array().ok_or("invalid polygon position")?;
                    if p.len() != 2 {
                        return fail("polygon must contain finite 2D positions");
                    }
                    let x = p[0].as_f64().filter(|v| v.is_finite());
                    let y = p[1].as_f64().filter(|v| v.is_finite());
                    let (Some(x), Some(y)) = (x, y) else {
                        return fail("polygon must contain finite 2D positions");
                    };
                    Ok(Coord { x, y })
                })
                .collect::<Result<_>>()?;
            if coords.first() != coords.last() {
                return fail("polygon rings must be closed and have at least four positions");
            }
            Ok(LineString::new(coords))
        })
        .collect::<Result<_>>()?;
    Ok(Polygon::new(rings[0].clone(), rings[1..].to_vec()))
}
fn parsed(value: &Value, crs: &str, limit: usize) -> Result<Geometry> {
    supported_crs(crs)?;
    let mut count = 0;
    let polygons = match value.get("type").and_then(Value::as_str) {
        Some("Polygon") => vec![parse_polygon(&value["coordinates"], &mut count, limit)?],
        Some("MultiPolygon") => value["coordinates"]
            .as_array()
            .ok_or("invalid multipolygon coordinates")?
            .iter()
            .map(|p| parse_polygon(p, &mut count, limit))
            .collect::<Result<_>>()?,
        _ => return fail("native geometry requires a Polygon or MultiPolygon"),
    };
    let g = Geometry {
        polygons: MultiPolygon::new(polygons),
        crs: crs.into(),
    };
    if g.is_empty() || !g.polygons.is_valid() || g.area() <= 0. || !g.area().is_finite() {
        return fail("polygon is empty or invalid (simple rings with valid non-overlapping holes are required)");
    }
    if crs == "EPSG:4326" {
        geographic_domain(&g)?;
    }
    Ok(g)
}
/// Parse catalog footprints and serialized execution masks. These may contain
/// clipped pieces or the <=100000 positions from a previously densified AOI.
pub fn from_geojson(value: &Value, crs: &str) -> Result<Geometry> {
    parsed(value, crs, MAPPED_LIMIT)
}
pub fn geometry(spatial: &Spatial) -> Result<Geometry> {
    let (crs, coordinates) = match spatial {
        Spatial::ArraySlice { .. } => return fail("array slices have no polygon geometry"),
        Spatial::Bounds {
            crs,
            bbox: [x0, y0, x1, y1],
        } => {
            if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) || x0 >= x1 || y0 >= y1 {
                return fail("bounds must be finite and ordered xmin < xmax, ymin < ymax");
            }
            (
                crs,
                json!([[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]]),
            )
        }
        Spatial::Polygon { crs, coordinates } => (crs, coordinates.clone()),
    };
    parsed(
        &json!({"type":"Polygon","coordinates":coordinates}),
        crs,
        INPUT_LIMIT,
    )
}
fn geographic_domain(g: &Geometry) -> Result<()> {
    let [xmin, ymin, xmax, ymax] = bounds(g);
    if xmin < -180. || xmax > 180. || ymin < -80. || ymax > 80. {
        return fail(
            "native geographic geometry requires longitude [-180,180] and latitude [-80,80]",
        );
    }
    if xmax - xmin > 5. || ymax - ymin > 5. {
        return fail("native geographic geometry supports extents up to 5 degrees and rejects antimeridian/wrapped AOIs");
    }
    Ok(())
}
pub fn transform(spatial: &Spatial, target: &str) -> Result<Geometry> {
    let g = geometry(spatial)?;
    if g.crs == target {
        return Ok(g);
    }
    let Some((zone, south)) = utm(target) else {
        return fail("native transformation supports only WGS84 longitude/latitude to Sentinel WGS84 UTM grids");
    };
    if g.crs != "EPSG:4326" {
        return fail("native transformation requires an EPSG:4326 longitude/latitude AOI; projected-to-geographic and cross-zone transforms are unsupported");
    }
    let [xmin, ymin, xmax, ymax] = bounds(&g);
    let center = zone as f64 * 6. - 183.;
    if xmin < center - 6. || xmax > center + 6. || (!south && ymin < 0.) || (south && ymax > 0.) {
        return fail("AOI is outside the supported target UTM zone vicinity or hemisphere");
    }
    let from = Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs")?;
    // proj4rs UTM is its extended transverse Mercator implementation. No
    // datum grids, EPSG database, system PROJ, or approximation engine is used.
    let to = Proj::from_proj_string(&format!(
        "+proj=utm +zone={zone} {} +datum=WGS84 +units=m +no_defs",
        if south { "+south" } else { "" }
    ))?;
    let mut total = 0usize;
    let mut mapped = Vec::with_capacity(g.polygons.0.len());
    for polygon in &g.polygons.0 {
        let mut rings = Vec::new();
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let mut points = Vec::new();
            for pair in ring.0.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let n = ((b.x - a.x).abs().max((b.y - a.y).abs()) / 0.001)
                    .ceil()
                    .max(1.) as usize;
                total = total.checked_add(n).ok_or("AOI vertex count overflow")?;
                if total >= MAPPED_LIMIT {
                    return fail("densified AOI exceeds 100000 positions; narrow the AOI");
                }
                for i in 0..n {
                    let f = i as f64 / n as f64;
                    let mut p = (
                        (a.x + (b.x - a.x) * f).to_radians(),
                        (a.y + (b.y - a.y) * f).to_radians(),
                        0.,
                    );
                    proj4rs::transform::transform(&from, &to, &mut p)?;
                    if !p.0.is_finite() || !p.1.is_finite() {
                        return fail("native UTM transformation produced a non-finite coordinate");
                    }
                    points.push(Coord { x: p.0, y: p.1 });
                }
            }
            total = total.checked_add(1).ok_or("AOI vertex count overflow")?;
            if total > MAPPED_LIMIT {
                return fail("densified AOI exceeds 100000 positions; narrow the AOI");
            }
            points.push(points[0]);
            rings.push(LineString::new(points));
        }
        mapped.push(Polygon::new(rings[0].clone(), rings[1..].to_vec()));
    }
    let polygons = MultiPolygon::new(mapped);
    if !polygons.is_valid() || polygons.unsigned_area() <= 0. {
        return fail("AOI transformation produced invalid geometry");
    }
    Ok(Geometry {
        polygons,
        crs: target.into(),
    })
}
pub fn bounds(g: &Geometry) -> [f64; 4] {
    g.polygons
        .bounding_rect()
        .map_or([0.; 4], |r| [r.min().x, r.min().y, r.max().x, r.max().y])
}
pub struct Mask(Vec<u8>);
impl Mask {
    pub fn data(&self) -> &[u8] {
        &self.0
    }
}
fn validate_grid(l: &Layout) -> Result<()> {
    let t = l.transform;
    if !t.iter().all(|v| v.is_finite()) || t[1] <= 0. || t[5] >= 0. || t[2] != 0. || t[4] != 0. {
        return fail("native polygon masks require a finite north-up PixelIsArea grid");
    }
    if ![
        1. / t[1],
        1. / t[5],
        -t[0] / t[1],
        -t[3] / t[5],
        t[0] + l.width as f64 * t[1],
        t[3] + l.height as f64 * t[5],
    ]
    .iter()
    .all(|v| v.is_finite())
    {
        return fail("native grid inverse or extent is outside finite coordinate support");
    }
    Ok(())
}
/// Rasterize using GDAL's default polygon scanline convention, without
/// ALL_TOUCHED. In source-global pixel coordinates each scanline is at y + 0.5;
/// nonhorizontal edges cross when min_y <= center_y < max_y. Sorted crossing
/// pairs fill [floor(left_x+0.5), floor(right_x+0.5)), so a center on a vertical
/// edge belongs to the region on its left. GDAL also burns right-to-left
/// horizontal edges on an exact center scanline after normalizing every ring
/// clockwise in map coordinates (including holes). We apply that tie rule,
/// independent of input winding. MultiPolygon pieces are combined by union.
/// Coordinates, crossings, and rounding use the complete source grid before
/// integer window clipping, so inclusion is independent of chunk size. The
/// reference is GDAL rasterizing the full source grid, then cropping it; GDAL
/// per-window inverse affine rounding can differ on exact center boundaries.
/// No epsilon moves a boundary or changes nearby center classifications.
pub fn mask(g: &Geometry, l: &Layout, w: &Window) -> Result<Mask> {
    validate_grid(l)?;
    if g.crs != l.crs {
        return fail("native polygon mask CRS must equal source grid CRS");
    }
    if w.x.checked_add(w.width).is_none_or(|n| n > l.width)
        || w.y.checked_add(w.height).is_none_or(|n| n > l.height)
    {
        return fail("native mask window exceeds source grid");
    }
    let len = w
        .width
        .checked_mul(w.height)
        .ok_or("native mask size overflow")?;
    let mut mask = vec![0; len];
    let t = l.transform;
    for polygon in &g.polygons.0 {
        let rings: Vec<Vec<Coord<f64>>> = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(|r| -> Result<_> {
                let mut points: Vec<_> =
                    r.0.iter()
                        .map(|c| Coord {
                            // Match GDAL's inverse-affine evaluation order. At
                            // exact center ties large UTM offsets can otherwise
                            // round differently by a fraction of a pixel ulp.
                            x: c.x.mul_add(1. / t[1], -t[0] / t[1]),
                            y: c.y.mul_add(1. / t[5], -t[3] / t[5]),
                        })
                        .collect();
                if points.iter().any(|c| !c.x.is_finite() || !c.y.is_finite()) {
                    return fail("native polygon pixel coordinates are non-finite");
                }
                // Negative north-up y scale changes clockwise map winding
                // into positive signed area in pixel coordinates.
                let signed: f64 = points
                    .windows(2)
                    .map(|p| p[0].x * p[1].y - p[1].x * p[0].y)
                    .sum();
                if signed < 0. {
                    points.reverse();
                }
                if !signed.is_finite() {
                    return fail("native polygon exceeds finite pixel-coordinate support");
                }
                Ok(points)
            })
            .collect::<Result<_>>()?;
        let mut crossings = Vec::new();
        for row in 0..w.height {
            let cy = (w.y + row) as f64 + 0.5;
            crossings.clear();
            for ring in &rings {
                for pair in ring.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    if a.y == b.y {
                        if a.y == cy && a.x > b.x {
                            fill_span(&mut mask, w, row, b.x, a.x);
                        }
                    } else {
                        let (a, b) = if a.y < b.y { (a, b) } else { (b, a) };
                        if a.y <= cy && cy < b.y {
                            crossings.push((cy - a.y) * (b.x - a.x) / (b.y - a.y) + a.x);
                        }
                    }
                }
            }
            crossings.sort_by(f64::total_cmp);
            for pair in crossings.chunks_exact(2) {
                fill_span(&mut mask, w, row, pair[0], pair[1]);
            }
        }
    }
    Ok(Mask(mask))
}
fn fill_span(data: &mut [u8], w: &Window, row: usize, left: f64, right: f64) {
    // Round before the integer offset: subtracting an offset from floating
    // vertices or crossings could change a center tie through cancellation.
    let min = w.x as f64;
    let max = (w.x + w.width) as f64;
    let start = (left + 0.5).floor().clamp(min, max) as usize - w.x;
    let end = (right + 0.5).floor().clamp(min, max) as usize - w.x;
    if start < end {
        data[row * w.width + start..row * w.width + end].fill(1);
    }
}
pub fn native_mask(s: &Spatial, l: &Layout) -> Result<Option<Geometry>> {
    // Preserve the existing generic direct-native-bounds acquisition path.
    if matches!(s, Spatial::Bounds {crs, ..} if crs == &l.crs) {
        return Ok(None);
    }
    validate_grid(l)?;
    let g = transform(s, &l.crs)?;
    let t = l.transform;
    let grid = geometry(&Spatial::Bounds {
        crs: l.crs.clone(),
        bbox: [
            t[0],
            t[3] + l.height as f64 * t[5],
            t[0] + l.width as f64 * t[1],
            t[3],
        ],
    })?;
    g.intersection(&grid).map(Some).ok_or_else(|| {
        "native polygon/grid intersection failed or exceeds supported complexity".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layout() -> Layout {
        Layout {
            width: 5,
            height: 5,
            transform: [0., 1., 0., 5., 0., -1.],
            crs: "EPSG:32617".into(),
            variable: crate::Variable::Band { index: 1 },
            block: [16, 16],
            dtype: "UInt16".into(),
            nodata: Some(0.),
            scale: None,
            offset: None,
            tiles: vec![],
        }
    }
    fn full() -> Window {
        Window {
            x: 0,
            y: 0,
            width: 5,
            height: 5,
        }
    }
    fn shape(coordinates: Value) -> Spatial {
        Spatial::Polygon {
            crs: "EPSG:32617".into(),
            coordinates,
        }
    }
    fn centered() -> Value {
        json!([
            [[0.5, 0.5], [4.5, 0.5], [4.5, 4.5], [0.5, 4.5], [0.5, 0.5]],
            [[1.5, 1.5], [3.5, 1.5], [3.5, 3.5], [1.5, 3.5], [1.5, 1.5]]
        ])
    }
    #[test]
    fn exact_center_edges_and_holes_match_independent_gdal_3103_fixture() {
        // Independently produced by rasterio 1.4.4 / GDAL 3.10.3 default
        // rasterize on Affine(1,0,0,0,-1,5), including exact center ties.
        let expected = [
            0, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1,
        ];
        let mut rings = centered();
        for _ in 0..2 {
            let s = shape(rings.clone());
            let g = geometry(&s).unwrap();
            assert_eq!(mask(&g, &layout(), &full()).unwrap().data(), expected);
            for ring in rings.as_array_mut().unwrap() {
                ring.as_array_mut().unwrap().reverse();
            }
        }
    }
    #[test]
    fn partial_windows_and_serialized_multipolygons_preserve_holes() {
        let l = layout();
        let g = geometry(&shape(centered())).unwrap();
        let restored = from_geojson(
            &serde_json::from_str::<Value>(&g.json().unwrap()).unwrap(),
            &l.crs,
        )
        .unwrap();
        let w = Window {
            x: 1,
            y: 1,
            width: 3,
            height: 3,
        };
        assert_eq!(
            mask(&restored, &l, &w).unwrap().data(),
            [1, 0, 0, 1, 0, 0, 1, 1, 1]
        );
        let mp = from_geojson(
            &json!({"type":"MultiPolygon","coordinates":[
                [[[0,0],[1,0],[1,1],[0,1],[0,0]]],
                [[[3,3],[4,3],[4,4],[3,4],[3,3]]]
            ]}),
            &l.crs,
        )
        .unwrap();
        assert_eq!(
            mask(&mp, &l, &full())
                .unwrap()
                .data()
                .iter()
                .map(|v| usize::from(*v))
                .sum::<usize>(),
            2
        );
    }
    #[test]
    fn sentinel_scale_inverse_affine_matches_independent_gdal_center_fixture() {
        // Same independent GDAL 3.10.3 fixture, now at real UTM magnitudes.
        // Inverse affine rounding puts its lowest edge just above center.
        let mut l = layout();
        l.transform = [500000., 10., 0., 4500000., 0., -10.];
        let coordinates = Value::Array(
            centered()
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    Value::Array(
                        r.as_array()
                            .unwrap()
                            .iter()
                            .map(|p| {
                                json!([
                                    500000. + p[0].as_f64().unwrap() * 10.,
                                    4500000. - p[1].as_f64().unwrap() * 10.
                                ])
                            })
                            .collect(),
                    )
                })
                .collect(),
        );
        let g = geometry(&shape(coordinates)).unwrap();
        assert_eq!(
            mask(&g, &l, &full()).unwrap().data(),
            [0, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 1, 1, 1, 1, 0, 0, 0, 0, 0]
        );
    }
    #[test]
    fn mask_inclusion_is_independent_of_chunk_partition_and_large_pixel_indices() {
        // The 60m/high-row case previously differed by 340 pixels between
        // full-source and chunked masks. Include ordinary Sentinel dimensions
        // and fractional UTM origins as well as that larger supported grid.
        for (ox, oy, scale, sx, sy) in [
            (500000., 4500000., 10., 8192., 0.),
            (500000.12345, 4500000.67891, 20., 0., 4096.),
            (579740., 3996380., 60., 0., 1063.),
            (579740., 3996380., 60., 0., 8192.),
        ] {
            let mut l = layout();
            l.width = 192 + sx as usize;
            l.height = 192 + sy as usize;
            l.transform = [ox, scale, 0., oy, 0., -scale];
            for coordinates in [
                json!([[
                    [5.5, 8.5],
                    [175.5, 8.5],
                    [175.5, 175.5],
                    [5.5, 175.5],
                    [5.5, 8.5]
                ]]),
                json!([
                    [
                        [2.5, 2.5],
                        [180.5, 2.5],
                        [180.5, 180.5],
                        [2.5, 180.5],
                        [2.5, 2.5]
                    ],
                    [
                        [20.5, 20.5],
                        [160.5, 20.5],
                        [160.5, 160.5],
                        [20.5, 160.5],
                        [20.5, 20.5]
                    ]
                ]),
            ] {
                let coordinates = Value::Array(
                    coordinates
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| {
                            Value::Array(
                                r.as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|p| {
                                        json!([
                                            ox + (sx + p[0].as_f64().unwrap()) * scale,
                                            oy - (sy + p[1].as_f64().unwrap()) * scale
                                        ])
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                );
                let g = geometry(&shape(coordinates)).unwrap();
                let full = mask(
                    &g,
                    &l,
                    &Window {
                        x: 0,
                        y: 0,
                        width: l.width,
                        height: l.height,
                    },
                )
                .unwrap();
                for edge in [17, 32, 64] {
                    for y in (0..l.height).step_by(edge) {
                        for x in (0..l.width).step_by(edge) {
                            let w = Window {
                                x,
                                y,
                                width: edge.min(l.width - x),
                                height: edge.min(l.height - y),
                            };
                            let part = mask(&g, &l, &w).unwrap();
                            for row in 0..w.height {
                                assert_eq!(
                                    &part.data()[row*w.width..(row+1)*w.width],
                                    &full.data()[(y+row)*l.width+x..(y+row)*l.width+x+w.width],
                                    "origin={ox},{oy} scale={scale} edge={edge} window={w:?} row={row}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn invalid_and_unsupported_geometry_fails_explicitly() {
        for coordinates in [
            json!([[[0, 0], [1, 1], [1, 0], [0, 1], [0, 0]]]),
            json!([[[0, 0], [1, 0], [1, 1], [0, 1]]]),
            json!([[[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 0, 1]]]),
            json!([
                [[0, 0], [5, 0], [5, 5], [0, 5], [0, 0]],
                [[6, 6], [7, 6], [7, 7], [6, 6]]
            ]),
        ] {
            assert!(geometry(&shape(coordinates)).is_err());
        }
        for bbox in [
            [179., 1., 181., 2.],
            [-179., 1., 179., 2.],
            [-80., 81., -79., 82.],
            [-80., 1., -74., 2.],
        ] {
            assert!(geometry(&Spatial::Bounds {
                crs: "EPSG:4326".into(),
                bbox
            })
            .is_err());
        }
        let s = Spatial::Bounds {
            crs: "EPSG:4326".into(),
            bbox: [-80., 35., -79., 36.],
        };
        assert!(transform(&s, "EPSG:3857").is_err());
        assert!(transform(&s, "EPSG:32601").is_err());
        assert!(transform(&s, "EPSG:32717").is_err());
        assert!(transform(&shape(centered()), "EPSG:4326").is_err());
        let mut l = layout();
        l.transform[2] = 0.1;
        assert!(mask(&geometry(&shape(centered())).unwrap(), &l, &full()).is_err());
    }
    #[test]
    fn native_bounds_still_support_other_crs_without_geometry() {
        let mut l = layout();
        l.crs = "EPSG:3857".into();
        let s = Spatial::Bounds {
            crs: l.crs.clone(),
            bbox: [1., 1., 2., 2.],
        };
        assert!(native_mask(&s, &l).unwrap().is_none());
        assert!(geometry(&s).is_err());
    }
    #[test]
    fn source_clip_can_split_a_polygon_and_exclude_disjoint_geometry() {
        let l = layout();
        let s = shape(json!([[
            [-2, -2],
            [1, -2],
            [1, 6],
            [4, 6],
            [4, -2],
            [7, -2],
            [7, 7],
            [-2, 7],
            [-2, -2]
        ]]));
        let g = native_mask(&s, &l).unwrap().unwrap();
        assert_eq!(g.polygons.0.len(), 2);
        let json: Value = serde_json::from_str(&g.json().unwrap()).unwrap();
        let restored = from_geojson(&json, &l.crs).unwrap();
        assert_eq!(
            mask(&g, &l, &full()).unwrap().data(),
            mask(&restored, &l, &full()).unwrap().data()
        );
        let s = shape(json!([[[8, 8], [9, 8], [9, 9], [8, 9], [8, 8]]]));
        assert!(native_mask(&s, &l).unwrap().unwrap().is_empty());
    }
    #[cfg(feature = "gdal")]
    #[test]
    fn native_projection_agrees_with_independent_system_proj_on_densified_rings() {
        for (bbox, target) in [
            ([-78.8, 35.7, -78.6, 35.9], "EPSG:32617"),
            ([151.0, -34.0, 151.2, -33.8], "EPSG:32756"),
            ([3.2, 79.7, 3.3, 79.8], "EPSG:32631"),
            ([5.8, 50.0, 6.2, 50.1], "EPSG:32632"),
        ] {
            let s = Spatial::Bounds {
                crs: "EPSG:4326".into(),
                bbox,
            };
            let native: Value =
                serde_json::from_str(&transform(&s, target).unwrap().json().unwrap()).unwrap();
            let oracle: Value = serde_json::from_str(
                &super::super::gdal_impl::transform(&s, target)
                    .unwrap()
                    .json()
                    .unwrap(),
            )
            .unwrap();
            let a = native["coordinates"][0].as_array().unwrap();
            let b = oracle["coordinates"][0].as_array().unwrap();
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b) {
                for axis in 0..2 {
                    assert!(
                        (a[axis].as_f64().unwrap() - b[axis].as_f64().unwrap()).abs() < 1e-6,
                        "{target}: {a} vs {b}"
                    );
                }
            }
        }
    }
    #[cfg(feature = "gdal")]
    #[test]
    fn center_edges_diagonals_winding_and_holes_match_gdal_rasterizer() {
        let l = layout();
        let mut cases = vec![
            centered(),
            json!([[[0.5, 0.5], [4.5, 0.5], [2.5, 4.5], [0.5, 0.5]]]),
            json!([[
                [0, 0],
                [5, 0],
                [5, 2.5],
                [2.5, 2.5],
                [2.5, 5],
                [0, 5],
                [0, 0]
            ]]),
        ];
        for left in [-0.5, 0., 0.5, 0.500000000001] {
            for right in [3.5, 4., 4.5, 5.5] {
                for bottom in [-0.5, 0., 0.5, 1.5] {
                    for top in [3.5, 4., 4.5, 5.5] {
                        cases.push(json!([[
                            [left, bottom],
                            [right, bottom],
                            [right, top],
                            [left, top],
                            [left, bottom]
                        ]]));
                    }
                }
            }
        }
        for mut coordinates in cases {
            for _ in 0..2 {
                let s = shape(coordinates.clone());
                let a = mask(&geometry(&s).unwrap(), &l, &full()).unwrap();
                let b = super::super::gdal_impl::mask(
                    &super::super::gdal_impl::geometry(&s).unwrap(),
                    &l,
                    &full(),
                )
                .unwrap();
                assert_eq!(a.data(), b.data(), "{coordinates}");
                for ring in coordinates.as_array_mut().unwrap() {
                    ring.as_array_mut().unwrap().reverse();
                }
            }
        }
    }

    #[cfg(feature = "gdal")]
    #[test]
    fn clipping_at_sentinel_scale_preserves_exact_boundary_masks() {
        for (origin_x, origin_y) in [(500000., 4500000.), (500000.12345, 4500000.67891)] {
            let mut l = layout();
            l.transform = [origin_x, 10., 0., origin_y, 0., -10.];
            for coords in [
                centered(),
                json!([[
                    [-0.5, 0.5],
                    [4.5, 0.5],
                    [4.5, 4.5],
                    [-0.5, 4.5],
                    [-0.5, 0.5]
                ]]),
            ] {
                let coordinates = Value::Array(
                    coords
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| {
                            Value::Array(
                                r.as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|p| {
                                        json!([
                                            origin_x + p[0].as_f64().unwrap() * 10.,
                                            origin_y - p[1].as_f64().unwrap() * 10.
                                        ])
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                );
                let s = shape(coordinates);
                let a = mask(&native_mask(&s, &l).unwrap().unwrap(), &l, &full()).unwrap();
                let b = super::super::gdal_impl::mask(
                    &super::super::gdal_impl::native_mask(&s, &l)
                        .unwrap()
                        .unwrap(),
                    &l,
                    &full(),
                )
                .unwrap();
                assert_eq!(a.data(), b.data(), "{origin_x}, {origin_y}: {s:?}");
            }
        }
    }
}
