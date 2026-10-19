use crate::{
    cog::{self, LayoutCache},
    fail,
    model::*,
    zarr, Result,
};
use std::collections::HashSet;

pub trait FormatPlanner {
    fn capabilities(&self) -> Capabilities;
    fn plan_asset(&mut self, asset: &Asset, request: &Request, plan: &mut Plan) -> Result<()>;
}
pub struct CogPlanner<'a> {
    pub cache: &'a mut LayoutCache,
}
impl FormatPlanner for CogPlanner<'_> {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            selection: vec!["native_bounds".into(), "integer_band".into()],
            reads: vec!["library_window".into()],
            decoding: "GDAL GTiff".into(),
            compression: "delegated to installed GDAL; UInt16 parity required".into(),
            materialization: true,
        }
    }
    fn plan_asset(&mut self, asset: &Asset, request: &Request, p: &mut Plan) -> Result<()> {
        for variable in &request.variables {
            let Variable::Band { index } = variable else {
                return fail("COG requires integer band selectors");
            };
            let (layout, cached) = self.cache.resolve(asset, *index)?;
            let mask = crate::spatial::native_mask(&request.spatial, &layout)?;
            let spatial = if let Some(g) = &mask {
                if g.is_empty() || g.area() <= 0. {
                    continue;
                }
                Spatial::Bounds {
                    crs: layout.crs.clone(),
                    bbox: crate::spatial::bounds(g),
                }
            } else {
                request.spatial.clone()
            };
            let Some(window) = cog::window(&layout, &spatial)? else {
                continue;
            };
            if mask.is_some()
                && layout.nodata.is_none_or(|n| {
                    !n.is_finite() || n.fract() != 0. || !(0. ..=65535.).contains(&n)
                })
            {
                return fail(
                    "AOI masking requires representable UInt16 source nodata in this release",
                );
            }
            let selected_tiles = cog::tiles(&layout, &window);
            let payload = selected_tiles
                .iter()
                .try_fold(0_u64, |acc, t| acc.checked_add(t.bytes?));
            p.estimates.selected_compressed_payload_bytes = p
                .estimates
                .selected_compressed_payload_bytes
                .zip(payload)
                .and_then(|(a, b)| a.checked_add(b));
            p.selected.push(Selected { mask_geometry: mask.as_ref().map(|g| g.json().and_then(|s| serde_json::from_str(&s).map_err(|_| gdal::errors::GdalError::BadArgument("invalid geometry JSON".into())))).transpose()?, asset_id: asset.id.clone(), datetime: asset.datetime.clone(), reason: "direct asset; requested band and bounds intersect native grid; time predicate passed".into(), capabilities: self.capabilities(), layout: Some(layout.clone()), window: window.clone() });
            let inspect = push(p, request, &asset.id, vec![], Operation::Inspect { cached })?;
            for y in (window.y..window.y + window.height).step_by(request.budget.chunk_edge) {
                for x in (window.x..window.x + window.width).step_by(request.budget.chunk_edge) {
                    let w = Window {
                        x,
                        y,
                        width: request.budget.chunk_edge.min(window.x + window.width - x),
                        height: request.budget.chunk_edge.min(window.y + window.height - y),
                    };
                    if let Some(g) = &mask {
                        let m = crate::spatial::mask(g, &layout, &w)?;
                        if !m.data().iter().any(|v| *v != 0) {
                            continue;
                        }
                    }
                    let ts = cog::tiles(&layout, &w);
                    let read = push(
                        p,
                        request,
                        &asset.id,
                        vec![inspect],
                        Operation::Read {
                            read: Read::LibraryWindow {
                                window: w,
                                band: *index,
                                tiles: ts,
                            },
                        },
                    )?;
                    let decode = push(
                        p,
                        request,
                        &asset.id,
                        vec![read],
                        Operation::Decode {
                            reader: "GDAL RasterIO (fused retrieval and decode)".into(),
                        },
                    )?;
                    push(
                        p,
                        request,
                        &asset.id,
                        vec![decode],
                        Operation::Write {
                            output: format!("chunk-{read}.tif"),
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}
pub(crate) fn push(
    p: &mut Plan,
    r: &Request,
    asset: &str,
    dependencies: Vec<usize>,
    operation: Operation,
) -> Result<usize> {
    if p.nodes.len() >= r.budget.max_nodes {
        return fail("plan node budget exceeded; use smaller batches or raise max_nodes");
    }
    let id = p.nodes.len();
    p.nodes.push(Node {
        id,
        dependencies,
        asset_id: asset.into(),
        operation,
    });
    Ok(id)
}
fn instant(s: &str) -> Result<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map_err(|_| "time values must be RFC3339 timestamps".into())
}
fn time_matches(asset: &Asset, time: &Option<Time>) -> Result<bool> {
    let Some(time) = time else {
        return Ok(true);
    };
    let date = instant(
        asset
            .datetime
            .as_deref()
            .ok_or("time selection requires asset datetime")?,
    )?;
    match time {
        Time::Interval { start, end } => {
            let (s, e) = (instant(start)?, instant(end)?);
            if s > e {
                return fail("reversed time interval");
            }
            Ok(date >= s && date <= e)
        }
        Time::Instants { values } => {
            let dates = values
                .iter()
                .map(|v| instant(v))
                .collect::<Result<Vec<_>>>()?;
            Ok(dates.contains(&date))
        }
    }
}
pub fn validate(r: &Request) -> Result<&[Asset]> {
    if !matches!(r.grid, Grid::Native) {
        return fail("target grids/resampling are not implemented");
    }
    if !r.quality.is_empty() || !r.operations.is_empty() {
        return fail("quality, mosaicking and compositing operations are not implemented");
    }
    match &r.spatial {
        Spatial::Bounds { bbox, .. } => {
            if !bbox.iter().all(|x| x.is_finite()) || bbox[0] >= bbox[2] || bbox[1] >= bbox[3] {
                return fail("invalid or wrapped bounds");
            }
        }
        Spatial::Polygon { .. } => {
            crate::spatial::geometry(&r.spatial)?;
        }
    }
    if r.budget.chunk_edge == 0 || r.budget.max_nodes == 0 {
        return fail("chunk_edge and max_nodes must be positive");
    }
    let bytes = r
        .budget
        .chunk_edge
        .checked_mul(r.budget.chunk_edge)
        .and_then(|n| {
            n.checked_mul(if matches!(r.spatial, Spatial::Polygon { .. }) {
                4
            } else {
                2
            })
        })
        .ok_or("buffer size overflow")?;
    if bytes > r.budget.buffer_bytes {
        return fail("chunk pixel buffer exceeds buffer_bytes");
    }
    let mut variable_keys = HashSet::new();
    for variable in &r.variables {
        if !variable_keys.insert(serde_json::to_string(variable)?) {
            return fail("duplicate variable selector");
        }
    }
    if let Some(time) = &r.time {
        match time {
            Time::Interval { start, end } => {
                if instant(start)? > instant(end)? {
                    return fail("reversed time interval");
                }
            }
            Time::Instants { values } => {
                for value in values {
                    instant(value)?;
                }
            }
        }
    }
    if r.variables.is_empty() {
        return fail("at least one variable is required");
    }
    let Source::Assets { assets } = &r.source else {
        return fail("catalog discovery is not implemented; supply explicit assets");
    };
    let mut ids = HashSet::new();
    for a in assets {
        if a.id.is_empty()
            || a.id.len() > 128
            || !a
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
            || !ids.insert(&a.id)
        {
            return fail("asset IDs must be unique safe opaque identifiers");
        }
        if a.locator.starts_with("/vsi") || (!cog::remote(&a.locator) && a.locator.contains("://"))
        {
            return fail("unsupported locator scheme");
        }
        time_matches(a, &r.time)?;
    }
    Ok(assets)
}
pub fn plan(request: &Request, cache: &mut LayoutCache) -> Result<Plan> {
    let assets = validate(request)?;
    let mut p=Plan { schema_version:1,selected:vec![],nodes:vec![],estimates:Estimates {
        selected_compressed_payload_bytes:Some(0),remote_requests:None,
        note:"Payload estimate excludes headers, masks, GDAL range rounding and repeated tile reads between output chunks; actual request counts require observation".into(),
    },limitations:vec!["No implicit reprojection or compositing".into(),"buffer_bytes caps the returned UInt16 pixel buffer, not process RSS or GDAL codec/cache allocations".into(),"Saved plan is diagnostic; fetch replans from runtime request and validates local identity".into()] };
    for asset in assets {
        if !time_matches(asset, &request.time)? {
            continue;
        }
        match asset.format {
            Format::Cog => CogPlanner { cache }.plan_asset(asset, request, &mut p)?,
            Format::ZarrV2 => zarr::ZarrPlanner.plan_asset(asset, request, &mut p)?,
        }
    }
    p.selected.retain(|s| {
        p.nodes
            .iter()
            .any(|n| n.asset_id == s.asset_id && matches!(n.operation, Operation::Read { .. }))
    });
    Ok(p)
}
