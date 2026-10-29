use crate::{
    cog::{self, LayoutCache},
    fail,
    model::*,
    zarr, Result,
};
use std::collections::{BTreeMap, HashSet};

/// Product-private native-geometry exception records.  This lives beside the
/// COG planner so the exception is applied per inspected asset, rather than
/// becoming a request flag or a property of a reusable layout.
#[derive(Default)]
pub(crate) struct NativeSelectionPolicy {
    annual_dist_extended_north: BTreeMap<String, AnnualDistExtendedNorth>,
    hls_pc_southern: BTreeMap<String, HlsPcSouthern>,
}
struct HlsPcSouthern {
    product: String,
    observation: std::sync::Arc<crate::products::Observation>,
    band: String,
    asset: crate::products::ProductAsset,
}
struct AnnualDistExtendedNorth {
    candidate: crate::products::dist_ann::ExtendedNorthernUtm,
    observation: std::sync::Arc<crate::products::Observation>,
    band: String,
    asset: crate::products::ProductAsset,
}
impl NativeSelectionPolicy {
    pub(crate) fn is_empty(&self) -> bool {
        self.annual_dist_extended_north.is_empty() && self.hls_pc_southern.is_empty()
    }
    pub(crate) fn admit_hls_pc_southern(
        &mut self,
        asset_id: String,
        product: String,
        observation: std::sync::Arc<crate::products::Observation>,
        band: String,
        asset: crate::products::ProductAsset,
    ) -> Result<()> {
        if crate::products::hls_pc_southern_tile(&product, &observation, &band, &asset)?.is_none() {
            return fail("PC HLS southern selection requires a validated southern HLS source");
        }
        if self
            .hls_pc_southern
            .insert(
                asset_id,
                HlsPcSouthern {
                    product,
                    observation,
                    band,
                    asset,
                },
            )
            .is_some()
        {
            return fail("duplicate PC HLS native selection policy asset");
        }
        Ok(())
    }
    pub(crate) fn admit_annual_dist_extended_north(
        &mut self,
        asset_id: String,
        candidate: crate::products::dist_ann::ExtendedNorthernUtm,
        observation: std::sync::Arc<crate::products::Observation>,
        band: String,
        asset: crate::products::ProductAsset,
    ) -> Result<()> {
        if self
            .annual_dist_extended_north
            .insert(
                asset_id,
                AnnualDistExtendedNorth {
                    candidate,
                    observation,
                    band,
                    asset,
                },
            )
            .is_some()
        {
            return fail("duplicate annual DIST native selection policy asset");
        }
        Ok(())
    }
    fn selection_for(
        &self,
        asset: &Asset,
        layout: &Layout,
        sources: Option<&crate::storage::Sources>,
    ) -> Result<crate::spatial::NativeSelection> {
        if let Some(entry) = self.hls_pc_southern.get(&asset.id) {
            crate::products::routing::validate_signature(&entry.asset.href, &asset.locator)?;
            let tile = crate::products::hls_pc_southern_tile(
                &entry.product,
                &entry.observation,
                &entry.band,
                &entry.asset,
            )?
            .ok_or("missing PC HLS southern tile identity")?;
            let zone = tile[1..3].parse::<u32>()?;
            if layout.crs == format!("EPSG:{}", 32700 + zone) {
                return Ok(crate::spatial::NativeSelection::Standard);
            }
            let sources = sources
                .ok_or("PC HLS southern selection requires inspected native source evidence")?;
            let source = serde_json::json!({"object_version":asset.etag,"object_identities":sources.describe(&asset.id)});
            let profile = crate::products::profile::profile(&entry.product)?;
            let band = profile.band(&entry.band)?;
            let zone = crate::products::hls_pc_extended_northern_grid(
                &entry.product,
                &entry.observation,
                band,
                &entry.asset,
                &source,
                layout,
            )?;
            return Ok(crate::spatial::NativeSelection::ProviderExtendedNorthernUtm { zone });
        }
        let Some(entry) = self.annual_dist_extended_north.get(&asset.id) else {
            return Ok(crate::spatial::NativeSelection::Standard);
        };
        let zone = entry
            .candidate
            .tile
            .get(1..3)
            .ok_or("annual DIST extended-north tile is malformed")?
            .parse::<u32>()?;
        // Ordinary EPSG:327xx annual grids continue through the canonical
        // southern path.  Only a source actually declaring its northern code
        // may be considered for the special convention.
        if layout.crs != format!("EPSG:{}", 32600 + zone) {
            return Ok(crate::spatial::NativeSelection::Standard);
        }
        let sources = sources.ok_or(
            "annual DIST extended-north selection requires native inspected source evidence",
        )?;
        let source = serde_json::json!({
            "object_version":asset.etag.clone(),
            "object_identities":sources.describe(&asset.id),
        });
        let profile = crate::products::profile::profile(crate::products::dist_ann::ID)?;
        let band = profile.band(&entry.band)?;
        // This is intentionally before spatial selection: product identity,
        // retained physical COG evidence, and the native grid must all agree
        // before an EPSG:326xx southern AOI transform is allowed.
        crate::products::dist_ann::validate_physical(
            &entry.observation,
            band,
            &entry.asset,
            &source,
            layout,
        )?;
        crate::products::dist_ann::extended_northern_grid(&entry.candidate, layout)?;
        Ok(crate::spatial::NativeSelection::ProviderExtendedNorthernUtm { zone })
    }
}

pub trait FormatPlanner {
    fn capabilities(&self) -> Capabilities;
    fn plan_asset(&mut self, asset: &Asset, request: &Request, plan: &mut Plan) -> Result<()>;
}
pub struct CogPlanner<'a> {
    pub cache: &'a mut LayoutCache,
}
impl FormatPlanner for CogPlanner<'_> {
    fn capabilities(&self) -> Capabilities {
        cog_capabilities()
    }
    fn plan_asset(&mut self, asset: &Asset, request: &Request, p: &mut Plan) -> Result<()> {
        plan_cog_asset(
            asset,
            request,
            p,
            &NativeSelectionPolicy::default(),
            None,
            |band| {
                let inspection = std::time::Instant::now();
                let layout = self.cache.resolve(asset, band);
                crate::diagnostics::profile_stage(
                    "cog_layout_inspection",
                    inspection.elapsed().as_secs_f64(),
                );
                layout
            },
        )
    }
}
fn cog_capabilities() -> Capabilities {
    Capabilities {
        selection: vec!["native_bounds".into(), "integer_band".into()],
        reads: vec!["library_window".into()],
        decoding: if crate::cog::native_selected() {
            "tiff-reader 0.9 storage-domain UInt8/UInt16/Int16/Float32"
        } else {
            "GDAL GTiff"
        }
        .into(),
        compression: if crate::cog::native_selected() {
            "uncompressed/LZW/Deflate; integer predictor 1/2, Float32 predictor 1/3; bounded decode"
        } else {
            "delegated to installed GDAL; UInt16 parity required"
        }
        .into(),
        materialization: true,
    }
}
fn plan_cog_asset(
    asset: &Asset,
    request: &Request,
    p: &mut Plan,
    selection_policy: &NativeSelectionPolicy,
    sources: Option<&crate::storage::Sources>,
    mut resolve: impl FnMut(usize) -> Result<(Layout, bool)>,
) -> Result<()> {
    let valid_nodata = |dtype: &str, value: f64| {
        if !value.is_finite() {
            return false;
        }
        match dtype {
            "Byte" => value.fract() == 0. && (0. ..=u8::MAX as f64).contains(&value),
            "UInt16" => value.fract() == 0. && (0. ..=u16::MAX as f64).contains(&value),
            "Int16" => value.fract() == 0. && (i16::MIN as f64..=i16::MAX as f64).contains(&value),
            "Float32" => (value as f32) as f64 == value,
            _ => false,
        }
    };
    for variable in &request.variables {
        let Variable::Band { index } = variable else {
            return fail("COG requires integer band selectors");
        };
        let (layout, cached) = resolve(*index)?;
        let selection = selection_policy.selection_for(asset, &layout, sources)?;
        // The legacy chunked GeoTIFF writer is deliberately UInt16-only. Reject
        // unsupported source storage while preparing the concrete sink, before
        // `fetch` creates its destination or starts decoding. `Directory`
        // requests are auto-resolved to this sink for COGs, including direct
        // diagnostic planning that has not yet entered acquisition preparation.
        if matches!(
            request.sink,
            Sink::Directory { .. } | Sink::GeoTiffDirectory { .. }
        ) && layout.dtype != "UInt16"
        {
            return fail(
                "GeoTIFF directory output currently supports UInt16 native bands only; use zarr_directory for UInt8, Int16, or Float32 native bands",
            );
        }
        let mask = crate::spatial::native_mask_for_selection(&request.spatial, &layout, selection)?;
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
        if mask.is_some() && !cog::native_selected() && layout.nodata.is_none() {
            return fail("GDAL AOI masking requires representable UInt16 source nodata; native COG supports distinct no-nodata AOI masks");
        }
        if layout
            .nodata
            .is_some_and(|n| !valid_nodata(&layout.dtype, n))
        {
            return fail("source nodata must be exactly representable in the storage type");
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
        p.selected.push(Selected { timestamps: asset.datetime.iter().cloned().collect(), dimensions: None, mask_geometry: mask.as_ref().map(|g| -> Result<serde_json::Value> { Ok(serde_json::from_str(&g.json()?)?) }).transpose()?, asset_id: asset.id.clone(), datetime: asset.datetime.clone(), reason: "direct asset; requested band and bounds intersect native grid; time predicate passed".into(), capabilities: cog_capabilities(), layout: Some(layout.clone()), window: window.clone() });
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
                        reader: if crate::cog::native_selected() {
                            "tiff-reader native window"
                        } else {
                            "GDAL RasterIO (fused retrieval and decode)"
                        }
                        .into(),
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
pub(crate) fn time_matches(asset: &Asset, time: &Option<Time>) -> Result<bool> {
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
pub(crate) fn validate_common(r: &Request) -> Result<()> {
    if !matches!(r.grid, Grid::Native) {
        return fail("target grids/resampling are not implemented");
    }
    if !r.quality.is_empty() || !r.operations.is_empty() {
        return fail("quality, mosaicking and compositing operations are not implemented");
    }
    match &r.spatial {
        Spatial::ArraySlice {
            start,
            count,
            step,
            axes,
        } => {
            if count.contains(&0) || step.contains(&0) || axes.iter().any(|p| !p.starts_with('/')) {
                return fail("array slices require positive counts/steps and absolute axis paths");
            }
            for i in 0..2 {
                count[i]
                    .checked_sub(1)
                    .and_then(|n| n.checked_mul(step[i]))
                    .and_then(|n| n.checked_add(start[i]))
                    .ok_or("array slice overflow")?;
            }
            if !matches!(&r.source, Source::Assets { assets } if assets.iter().all(|a|a.format == Format::Hdf5))
            {
                return fail("array slices require exclusively HDF5 assets");
            }
        }
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
    Ok(())
}
pub fn validate(r: &Request) -> Result<&[Asset]> {
    validate_common(r)?;
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
        if a.format == Format::Cog {
            time_matches(a, &r.time)?;
        }
    }
    Ok(assets)
}
pub fn plan(request: &Request, cache: &mut LayoutCache) -> Result<Plan> {
    let runtime = crate::runtime_config::RuntimeConfig::capture(request)?;
    let _runtime = runtime.enter();
    if !matches!(request.source, Source::Assets { .. })
        || crate::storage::requires_zarr_bindings(request)
        || matches!(&request.source, Source::Assets { assets } if assets.iter().any(|asset| {
            asset.format == Format::Cog
                && (cog::native_selected() || cog::remote(&asset.locator))
        }))
    {
        return Ok(crate::acquisition::prepare(request, cache)?.plan().clone());
    }
    plan_on(request, cache, &crate::storage::Sources::default())
}
pub(crate) fn plan_on(
    request: &Request,
    cache: &mut LayoutCache,
    sources: &crate::storage::Sources,
) -> Result<Plan> {
    plan_on_with_native_selection(request, cache, sources, &NativeSelectionPolicy::default())
}
pub(crate) fn plan_on_with_native_selection(
    request: &Request,
    cache: &mut LayoutCache,
    sources: &crate::storage::Sources,
    selection_policy: &NativeSelectionPolicy,
) -> Result<Plan> {
    let _quiet = crate::diagnostics::QuietNative::enter();
    let started = std::time::Instant::now();
    let assets = validate(request)?;
    let mut p=Plan { schema_version:3,selected:vec![],nodes:vec![],estimates:Estimates {
        selected_compressed_payload_bytes:Some(0),remote_requests:None,
        note:"Payload estimate excludes headers, masks, GDAL range rounding and repeated tile reads between output chunks; actual request counts require observation".into(),
    },limitations:vec!["No implicit reprojection or compositing".into(),"buffer_bytes bounds admitted native sample/chunk payloads, not process RSS or external codec/cache scratch".into(),"Diagnostic JSON is not executable; use prepared runtime bindings or prepare anew from the request".into()] };
    // Inspect only one bounded batch at a time. Each native dataset stays on
    // its inspection thread; layouts and plan construction retain input order.
    let width = crate::storage::worker_count()?;
    let configuration = crate::cog::thread_configuration()?;
    // Also protect legacy direct planning when the caller supplies a new ETag.
    // All invalidation precedes opening any inspection handle.
    for asset in assets
        .iter()
        .filter(|a| a.format == Format::Cog && a.etag.is_some())
    {
        cog::clear_remote_cache(asset)?;
    }
    for batch in assets.chunks(width) {
        let inspection = std::time::Instant::now();
        // Shared URLs are not independent assets for GDAL's URL cache. Preserve
        // serial inspection there instead of racing duplicate header downloads.
        let independent = batch
            .iter()
            .map(|a| &a.locator)
            .collect::<HashSet<_>>()
            .len()
            == batch.len();
        let layouts = std::thread::scope(|scope| -> Result<Vec<Option<Vec<Layout>>>> {
            let mut handles = Vec::new();
            for asset in batch {
                let configuration = &configuration;
                let runtime = crate::runtime_config::active();
                handles.push(
                    if independent
                        // Keep multiband metadata incremental: plan node limits
                        // must apply before inspecting further variable layouts.
                        && request.variables.len() == 1
                        && asset.format == Format::Cog
                        && cog::remote(&asset.locator)
                        && time_matches(asset, &request.time)?
                    {
                        Some(scope.spawn(move || -> Result<Vec<Layout>> {
                            let _runtime = runtime
                                .as_ref()
                                .map(crate::runtime_config::RuntimeConfig::enter);
                            crate::cog::apply_thread_configuration(configuration)?;
                            request
                                .variables
                                .iter()
                                .map(|variable| {
                                    let Variable::Band { index } = variable else {
                                        return fail("COG requires integer band selectors");
                                    };
                                    #[cfg(feature = "native-cog")]
                                    if cog::native_selected() {
                                        return sources.native.inspect(asset, *index);
                                    }
                                    cog::inspect(asset, *index)
                                })
                                .collect()
                        }))
                    } else {
                        None
                    },
                );
            }
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .map(|h| h.join().map_err(|_| "COG inspection worker panicked")?)
                        .transpose()
                })
                .collect()
        })?;
        if layouts.iter().any(Option::is_some) {
            crate::diagnostics::profile_stage(
                "cog_layout_inspection",
                inspection.elapsed().as_secs_f64(),
            );
        }
        for (asset, layouts) in batch.iter().zip(layouts) {
            if asset.format == Format::Cog && !time_matches(asset, &request.time)? {
                continue;
            }
            match asset.format {
                Format::Cog => {
                    if let Some(layouts) = layouts {
                        let mut layouts = layouts.into_iter();
                        plan_cog_asset(
                            asset,
                            request,
                            &mut p,
                            selection_policy,
                            Some(sources),
                            |_| Ok((layouts.next().ok_or("missing inspected layout")?, false)),
                        )?;
                    } else {
                        #[cfg(feature = "native-cog")]
                        if cog::native_selected() {
                            plan_cog_asset(
                                asset,
                                request,
                                &mut p,
                                selection_policy,
                                Some(sources),
                                |band| Ok((sources.native.inspect(asset, band)?, false)),
                            )?;
                            continue;
                        }
                        // Non-native paths never receive a special selection
                        // policy; retain the generic planner behaviour.
                        CogPlanner { cache }.plan_asset(asset, request, &mut p)?;
                    }
                }
                Format::Hdf5 => {
                    #[cfg(feature = "native-hdf5")]
                    sources.hdf5.plan(asset, request, &mut p)?;
                    #[cfg(not(feature = "native-hdf5"))]
                    return fail("build with native-hdf5 to acquire HDF5");
                }
                Format::ZarrV3 | Format::Icechunk => {
                    sources
                        .v3
                        .plan(asset, request, &mut p, sources.get(asset)?)?
                }
                Format::ZarrV2 if cog::remote(&asset.locator) => {
                    crate::zarr_acquire::plan_asset_on(asset, request, &mut p, sources.get(asset)?)?
                }
                Format::ZarrV2 => zarr::ZarrPlanner.plan_asset(asset, request, &mut p)?,
            }
        }
    }
    p.selected.retain(|s| {
        p.nodes
            .iter()
            .any(|n| n.asset_id == s.asset_id && matches!(n.operation, Operation::Read { .. }))
    });
    #[cfg(feature = "native-cog")]
    if cog::native_selected() {
        // Prepared metadata is optional. Preserve enough working headroom for
        // every exact-read operation before freezing the runtime snapshot.
        let mut mandatory_peak = 0;
        for selected in &p.selected {
            let Some(layout) = &selected.layout else {
                continue;
            };
            let nodes: Vec<_> = p
                .nodes
                .iter()
                .filter(|n| {
                    n.asset_id == selected.asset_id
                        && matches!(
                            n.operation,
                            Operation::Read {
                                read: Read::LibraryWindow { .. }
                            }
                        )
                })
                .collect();
            if nodes.is_empty() {
                continue;
            }
            let (retained, working, _) = crate::native_cog::reservation(
                layout,
                &selected.window,
                &nodes,
                0,
                width.min(p.selected.len().max(1)),
            )?;
            let windows = nodes
                .iter()
                .filter_map(|n| match &n.operation {
                    Operation::Read {
                        read: Read::LibraryWindow { window, .. },
                    } => Some(window),
                    _ => None,
                })
                .map(|w| w.width.checked_mul(w.height).and_then(|n| n.checked_mul(4)))
                .collect::<Option<Vec<_>>>()
                .ok_or("native COG window size overflow")?
                .into_iter()
                .max()
                .unwrap_or(0);
            mandatory_peak = mandatory_peak.max(
                retained
                    .checked_add(working)
                    .and_then(|n| n.checked_add(windows))
                    .ok_or("native COG working set overflow")?,
            );
        }
        let transport = sources.transport_reservation()?;
        if mandatory_peak
            .checked_add(transport)
            .is_none_or(|bytes| bytes > request.budget.buffer_bytes)
        {
            return fail("native COG working set and transport sessions exceed buffer_bytes");
        }
        sources.native.trim(
            request
                .budget
                .buffer_bytes
                .saturating_sub(mandatory_peak)
                .saturating_sub(transport),
        );
    }
    crate::diagnostics::profile_stage(
        "native_planning_with_layout",
        started.elapsed().as_secs_f64(),
    );
    Ok(p)
}
