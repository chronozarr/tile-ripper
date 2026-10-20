//! Supported public Sentinel-2 discovery → prepared native plan → per-asset outputs.
use crate::{
    cog::LayoutCache, fail, spatial, Asset, Budget, ExecutionReport, Format, Grid, Plan, Request,
    Result, Sink, Source, Spatial, Time, Variable,
};
#[cfg(feature = "gdal")]
use gdal::{raster::RasterCreationOptions, Dataset, DriverManager};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
#[cfg(feature = "gdal")]
use std::{fs, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StacRequest {
    pub endpoint: String,
    pub collection: String,
    pub aoi: Spatial,
    pub start: String,
    pub end: String,
    pub bands: Vec<String>,
    pub output: String,
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    pub limits: Limits,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    EarthSearch,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_items: usize,
    pub page_size: usize,
    pub max_assets: usize,
    pub max_output_pixels: u64,
    pub buffer_bytes: usize,
    pub chunk_edge: usize,
    pub max_nodes: usize,
    pub gdal_cache_mb: usize,
    pub max_retries: u32,
    pub timeout_seconds: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_items: 32,
            page_size: 8,
            max_assets: 64,
            max_output_pixels: 16_000_000,
            buffer_bytes: 4 * 1024 * 1024,
            chunk_edge: 512,
            max_nodes: 100_000,
            gdal_cache_mb: 32,
            max_retries: 2,
            timeout_seconds: 45,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct SourceReference {
    pub asset_id: String,
    pub item_id: String,
    pub collection: String,
    pub asset_key: String,
    pub requested_band: String,
    pub datetime: String,
    pub href: String,
    pub item_reference: Option<String>,
    pub footprint: Value,
    pub raster_bands: Option<Value>,
    pub acquisition: Value,
    pub object_identity: Option<ObjectIdentity>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ObjectIdentity {
    pub etag: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct DiscoveryReport {
    pub seconds: f64,
    pub returned_items: usize,
    pub selected_items: usize,
    pub selected_assets: usize,
    pub attempts: u32,
}
/// Runtime-only bindings are deliberately private and not serializable.
pub struct Discovered {
    request: StacRequest,
    sources: Vec<SourceReference>,
    core: Request,
    geographic_bounds: [f64; 4],
    pub report: DiscoveryReport,
}
/// A diagnostic plan and its original runtime bindings. No deserialization/replay.
pub struct Prepared {
    discovered: Discovered,
    plan: Plan,
    pub planning_seconds: f64,
    sources: crate::storage::Sources,
}
impl Discovered {
    pub fn sources(&self) -> &[SourceReference] {
        &self.sources
    }
    pub fn prepare(mut self, cache: &mut LayoutCache) -> Result<Prepared> {
        configure(&self.request.limits)?;
        let t = Instant::now();
        resolve_identities(&mut self.core, &mut self.sources, &self.request.limits)?;
        let sources = crate::storage::Sources::new(&self.core)?;
        let mut plan = crate::planner::plan_on(&self.core, cache, &sources)?;
        // The STAC sink assembles native chunks into one file per asset.
        for node in &mut plan.nodes {
            if let crate::Operation::Write { output } = &mut node.operation {
                *output = format!("{}.tif", node.asset_id);
            }
        }
        let mut tile_reads = HashSet::new();
        let mut payload = Some(0u64);
        for node in &plan.nodes {
            if let crate::Operation::Read {
                read: crate::Read::LibraryWindow { band, tiles, .. },
            } = &node.operation
            {
                for tile in tiles {
                    if tile.bytes.is_none() {
                        payload = None;
                    }
                    if tile.bytes.is_some_and(|n| n > 0)
                        && tile_reads.insert((node.asset_id.as_str(), *band, tile.x, tile.y))
                    {
                        payload = payload.and_then(|n| n.checked_add(tile.bytes.unwrap_or(0)));
                    }
                }
            }
        }
        plan.estimates.selected_compressed_payload_bytes = payload;
        plan.estimates.remote_requests =
            Some(tile_reads.len() as u64 + 7 * plan.selected.len() as u64);
        plan.estimates.note = "ESTIMATE: one GET per distinct selected non-sparse tile plus five HEADs (identity, plan open, pre/post validation, execution open) and two metadata GETs per asset. GDAL may merge, round, reread, retry, or issue extra metadata reads. Payload excludes metadata and transport overhead; observed counters are separate. Metadata inspections of assets rejected after pixel-center checks are not included.".into();
        if crate::cog::native_selected() {
            plan.estimates.remote_requests = None;
            plan.estimates.note = "Native HTTP totals depend on TIFF metadata pages, optional prepared-page reuse, admitted adjacent-tile groups, cache hits and bounded retries. Immutable-source HEAD checks remain mandatory. Selected compressed payload excludes wrappers, metadata and HTTP overhead; inspect observed transport counters for request and byte totals. Rejected-source metadata inspections are separate.".into();
        }
        let pixels = plan
            .selected
            .iter()
            .try_fold(0u64, |n, s| {
                n.checked_add((s.window.width as u64).checked_mul(s.window.height as u64)?)
            })
            .ok_or("output pixel count overflow")?;
        if pixels > self.request.limits.max_output_pixels {
            return fail("native output envelopes exceed max_output_pixels; narrow the AOI/date or raise the explicit limit");
        }
        Ok(Prepared {
            discovered: self,
            plan,
            planning_seconds: t.elapsed().as_secs_f64(),
            sources,
        })
    }
}
impl Prepared {
    pub(crate) fn output(&self) -> &str {
        &self.discovered.request.output
    }
    pub fn plan(&self) -> &Plan {
        &self.plan
    }
    pub fn sources(&self) -> &[SourceReference] {
        &self.discovered.sources
    }
    pub fn discovery(&self) -> &DiscoveryReport {
        &self.discovered.report
    }
    pub fn explain(&self) -> Value {
        json!({"discovery":self.discovery(),"sources":self.sources(),"query":self.query(),"planning_seconds":self.planning_seconds,"plan":self.plan,"decisions":decisions()})
    }
    /// Geographic query coordinates are longitude, latitude in WGS84 degrees.
    pub fn query(&self) -> Value {
        json!({"aoi":self.discovered.core.spatial,"bounds_wgs84":self.discovered.geographic_bounds,
            "coordinate_order":"longitude, latitude","edge_interpolation":"straight in query CRS; geographic edges densified to <=0.001 degree before UTM mapping"})
    }
    pub fn execute(&self, sink: impl FnMut(crate::Chunk) -> Result<()>) -> Result<ExecutionReport> {
        self.execute_cancellable(&Default::default(), sink)
    }
    pub(crate) fn execute_cancellable(
        &self,
        cancelled: &std::sync::atomic::AtomicBool,
        sink: impl FnMut(crate::Chunk) -> Result<()>,
    ) -> Result<ExecutionReport> {
        configure(&self.discovered.request.limits)?;
        validate_identities(&self.discovered.core, &self.discovered.request.limits)?;
        let report = crate::scheduler::execute(
            &self.discovered.core,
            &self.plan,
            self.planning_seconds,
            &self.sources,
            cancelled,
            sink,
        )?;
        validate_identities(&self.discovered.core, &self.discovered.request.limits)?;
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return fail("acquisition cancelled");
        }
        Ok(report)
    }
    /// One tiled native GeoTIFF per source asset; chunk writes stay bounded.
    #[cfg(feature = "gdal")]
    pub fn fetch(&self) -> Result<Value> {
        let root = Path::new(&self.discovered.request.output);
        if root.exists() {
            return fail("output directory already exists; choose a new directory");
        }
        let parent = root
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        fs::create_dir(root)?;
        let result = (|| -> Result<Value> {
            let mut current: Option<(String, Dataset)> = None;
            let mut written = HashSet::new();
            let report = self.execute(|mut chunk| {
                let selected = self
                    .plan
                    .selected
                    .iter()
                    .find(|s| {
                        s.asset_id == chunk.asset_id
                            && s.layout
                                .as_ref()
                                .is_some_and(|l| l.variable == chunk.variable)
                    })
                    .ok_or("missing planned output")?;
                let layout = selected.layout.as_ref().ok_or("missing grid")?;
                if current.as_ref().is_none_or(|(id, _)| *id != chunk.asset_id) {
                    if let Some((_, mut ds)) = current.take() {
                        ds.flush_cache()?;
                    }
                    let options = RasterCreationOptions::from_iter([
                        "TILED=YES",
                        "COMPRESS=DEFLATE",
                        "PREDICTOR=2",
                        "BLOCKXSIZE=256",
                        "BLOCKYSIZE=256",
                        "NUM_THREADS=1",
                    ]);
                    let mut ds = DriverManager::get_driver_by_name("GTiff")?
                        .create_with_band_type_with_options::<u16, _>(
                            root.join(format!("{}.tif", chunk.asset_id)),
                            selected.window.width,
                            selected.window.height,
                            1,
                            &options,
                        )?;
                    ds.set_geo_transform(&spatial::window_transform(layout, &selected.window))?;
                    ds.set_spatial_ref(&spatial::srs(&layout.crs)?)?;
                    let mut b = ds.rasterband(1)?;
                    b.set_no_data_value(layout.nodata)?;
                    // Fill skipped AOI-only chunks with source nodata, not arbitrary zero.
                    b.fill(layout.nodata.unwrap_or(0.), None)?;
                    if let Some(v) = layout.scale {
                        b.set_scale(v)?;
                    }
                    if let Some(v) = layout.offset {
                        b.set_offset(v)?;
                    }
                    current = Some((chunk.asset_id.clone(), ds));
                    written.insert(chunk.asset_id.clone());
                }
                current.as_mut().unwrap().1.rasterband(1)?.write(
                    (
                        (chunk.window.x - selected.window.x) as isize,
                        (chunk.window.y - selected.window.y) as isize,
                    ),
                    (chunk.window.width, chunk.window.height),
                    &mut gdal::raster::Buffer::new(
                        (chunk.window.width, chunk.window.height),
                        std::mem::take(&mut chunk.pixels).into_u16()?,
                    ),
                )?;
                Ok(())
            })?;
            let t = Instant::now();
            if let Some((_, mut ds)) = current.take() {
                ds.flush_cache()?;
            }
            let outputs:Vec<_>=self.plan.selected.iter().filter(|s|written.contains(&s.asset_id)).map(|s|json!({"file":format!("{}.tif",s.asset_id),"asset_id":s.asset_id,"datetime":s.datetime,"window":s.window,"grid":s.layout,"transform":spatial::window_transform(s.layout.as_ref().unwrap(),&s.window),"aoi_mask_applied":s.mask_geometry.is_some()})).collect();
            let manifest = json!({"schema_version":2,"endpoint":safe_reference(&self.discovered.request.endpoint),"collection":self.discovered.request.collection,"aoi":self.discovered.request.aoi,"time":{"start":self.discovered.request.start,"end":self.discovered.request.end},"sources":self.sources(),"outputs":outputs,"discovery":self.discovery(),"observed":report,"final_flush_seconds":t.elapsed().as_secs_f64(),"decisions":decisions(),"plan":self.plan});
            fs::write(
                root.join("manifest.json"),
                serde_json::to_vec_pretty(&manifest)?,
            )?;
            Ok(manifest)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(root);
        }
        result
    }
}
fn decisions() -> Vec<&'static str> {
    vec!["raw UInt16 samples; no calibration, cloud filtering or compositing","native grids per asset; no raster reprojection or cross-grid assembly","AOI edges densified before geometry-only CRS transformation; GDAL-compatible pixel-center rasterization, ALL_TOUCHED=false","source nodata preserved; outside AOI set to source nodata","source references omit URL query/fragment/credentials; asset key and item ID retain provenance","reader-specific measured read/decode and source retrieval counters appear in the completion report","discovery is complete within the explicit cap; jobs exceeding limits fail rather than truncate"]
}
pub fn safe_reference(href: &str) -> String {
    let clean = href.split(['?', '#']).next().unwrap_or("");
    if let Some((scheme, rest)) = clean.split_once("://") {
        let rest = rest.split_once('@').map_or(rest, |(_, s)| s);
        format!("{scheme}://{rest}")
    } else {
        clean.into()
    }
}
#[cfg(feature = "gdal")]
fn configure(l: &Limits) -> Result<()> {
    for (k, v) in [
        ("GDAL_DISABLE_READDIR_ON_OPEN", "EMPTY_DIR".into()),
        ("GDAL_INGESTED_BYTES_AT_OPEN", "32768".into()),
        ("GDAL_NUM_THREADS", "1".into()),
        ("GDAL_CACHEMAX", l.gdal_cache_mb.to_string()),
        ("GDAL_HTTP_MAX_RETRY", l.max_retries.to_string()),
        ("GDAL_HTTP_RETRY_DELAY", "1".into()),
        ("GDAL_HTTP_TIMEOUT", l.timeout_seconds.to_string()),
        ("CPL_DEBUG", "OFF".into()),
        ("CPL_CURL_VERBOSE", "NO".into()),
    ] {
        gdal::config::set_thread_local_config_option(k, &v)?;
    }
    Ok(())
}
fn band_key(request: &str) -> &str {
    match request {
        "B01" => "coastal",
        "B02" => "blue",
        "B03" => "green",
        "B04" => "red",
        "B05" => "rededge1",
        "B06" => "rededge2",
        "B07" => "rededge3",
        "B08" => "nir",
        "B8A" => "nir08",
        "B09" => "nir09",
        "B11" => "swir16",
        "B12" => "swir22",
        other => other,
    }
}
pub fn discover(r: &StacRequest) -> Result<Discovered> {
    if r.collection != "sentinel-2-c1-l2a" {
        return fail("Earth Search adapter currently supports collection sentinel-2-c1-l2a only");
    }
    if !r.endpoint.starts_with("https://") && !r.endpoint.starts_with("http://127.0.0.1:") {
        return fail("STAC endpoint must use HTTPS (loopback HTTP is allowed for tests)");
    }
    let l = &r.limits;
    if l.max_items == 0
        || l.max_items > 1000
        || l.page_size == 0
        || l.page_size > 100
        || l.max_assets == 0
        || l.max_assets > 1000
        || l.timeout_seconds == 0
        || l.timeout_seconds > 300
        || l.max_retries > 5
        || l.gdal_cache_mb == 0
        || l.gdal_cache_mb > 1024
    {
        return fail("invalid discovery/resource limits (items/assets <=1000, page<=100, retries<=5, timeout<=300s, GDAL cache 1..1024 MiB)");
    }
    let start = chrono::DateTime::parse_from_rfc3339(&r.start)?;
    let end = chrono::DateTime::parse_from_rfc3339(&r.end)?;
    if start > end {
        return fail("reversed date interval");
    }
    if r.bands.is_empty() || r.bands.len() > 13 {
        return fail("request 1..13 distinct Sentinel-2 bands");
    }
    let mut unique = HashSet::new();
    for band in &r.bands {
        if !matches!(
            band_key(band),
            "coastal"
                | "blue"
                | "green"
                | "red"
                | "rededge1"
                | "rededge2"
                | "rededge3"
                | "nir"
                | "nir08"
                | "nir09"
                | "swir16"
                | "swir22"
        ) {
            return fail("unsupported Sentinel-2 reflectance band; use B01..B09, B8A, B11, B12 or Earth Search common names");
        }
        if !unique.insert(band_key(band)) {
            return fail("duplicate band selector");
        }
    }
    let wgs = spatial::transform(&r.aoi, "EPSG:4326")?;
    let b = spatial::bounds(&wgs);
    if b[0] < -180.
        || b[2] > 180.
        || b[1] < -80.
        || b[3] > 80.
        || b[2] - b[0] > 5.
        || b[3] - b[1] > 5.
    {
        return fail("this workflow requires a non-wrapping AOI within +/-80 latitude and <=5 degrees per axis");
    }
    let search: stac::api::Search = serde_json::from_value(
        json!({"collections":[r.collection],"intersects":serde_json::from_str::<Value>(&wgs.json()?)?,"datetime":format!("{}/{}",r.start,r.end),"limit":l.page_size}),
    )?;
    let t = Instant::now();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut attempts = 0;
    let mut collection = loop {
        attempts += 1;
        let builder =
            stac_io::api::ClientBuilder::new().timeout(Duration::from_secs(l.timeout_seconds));
        let result = runtime.block_on(stac_io::api::search_with_client_builder(
            &r.endpoint,
            search.clone(),
            Some(l.max_items + 1),
            builder,
        ));
        match result {
            Ok(c) => break c,
            Err(ref e)
                if attempts <= l.max_retries
                    && matches!(e, stac_io::Error::Reqwest(error) if error.status().is_none_or(|s| s.is_server_error() || s.as_u16()==429 || s.as_u16()==408)) =>
            {
                std::thread::sleep(Duration::from_secs(u64::from(attempts)))
            }
            Err(stac_io::Error::Reqwest(e)) => {
                return Err(format!(
                    "STAC discovery failed after bounded retries: {}",
                    e.without_url()
                )
                .into())
            }
            Err(_) => {
                return fail(
                    "STAC discovery failed after bounded retries (endpoint/credentials omitted)",
                )
            }
        }
    };
    if collection.items.len() > l.max_items {
        return fail("STAC query exceeds max_items; narrow dates/AOI or raise the explicit cap");
    }
    collection
        .items
        .sort_by_key(|v| v.get("id").and_then(Value::as_str).unwrap_or("").to_owned());
    let returned_items = collection.items.len();
    let mut sources = Vec::new();
    let mut assets = Vec::new();
    let mut seen = HashSet::new();
    let mut selected_items = 0;
    for value in collection.items {
        let item: stac::Item = serde_json::from_value(Value::Object(value))?;
        if item.collection.as_deref() != Some(r.collection.as_str()) {
            return fail("STAC returned an item from another collection");
        }
        if !seen.insert(item.id.clone()) {
            continue;
        }
        let datetime = item
            .properties
            .datetime
            .ok_or("STAC item missing acquisition datetime")?;
        if datetime < start || datetime > end {
            continue;
        }
        let footprint = serde_json::to_value(
            item.geometry
                .as_ref()
                .ok_or("STAC item has no footprint geometry")?,
        )?;
        let geometry = spatial::from_geojson(&footprint, "EPSG:4326")?;
        let intersection = wgs
            .intersection(&geometry)
            .ok_or("item geometry intersection failed")?;
        if intersection.is_empty() || intersection.area() <= 0. {
            continue;
        }
        selected_items += 1;
        for requested in &r.bands {
            let key = band_key(requested);
            let a = item
                .assets
                .get(key)
                .ok_or_else(|| format!("item {} missing requested band {}", item.id, requested))?;
            if !a.href.starts_with("https://") && !a.href.starts_with("http://127.0.0.1:") {
                return fail("Earth Search adapter requires public absolute HTTPS COG assets; authenticated/signing providers need a separate adapter");
            }
            if a.href
                .split('?')
                .next()
                .is_none_or(|s| !s.to_lowercase().ends_with(".tif"))
            {
                return fail("requested asset is not a TIFF COG");
            }
            if assets.len() >= l.max_assets {
                return fail("selected assets exceed max_assets");
            }
            let id = format!("asset-{:04}", assets.len());
            let item_ref = item
                .links
                .iter()
                .find(|l| l.rel == "self")
                .map(|l| safe_reference(&l.href));
            let properties = serde_json::to_value(&item.properties)?;
            let acquisition = json!({"platform":properties.get("platform"),"instruments":properties.get("instruments"),"s2:processing_baseline":properties.get("s2:processing_baseline"),"earthsearch:boa_offset_applied":properties.get("earthsearch:boa_offset_applied"),"eo:cloud_cover":properties.get("eo:cloud_cover")});
            sources.push(SourceReference {
                asset_id: id.clone(),
                item_id: item.id.clone(),
                collection: r.collection.clone(),
                asset_key: key.into(),
                requested_band: requested.clone(),
                datetime: datetime.to_rfc3339(),
                href: safe_reference(&a.href),
                item_reference: item_ref,
                footprint: footprint.clone(),
                raster_bands: a.additional_fields.get("raster:bands").cloned(),
                acquisition,
                object_identity: None,
            });
            assets.push(Asset {
                etag: None,
                id,
                locator: a.href.clone(),
                format: Format::Cog,
                datetime: Some(datetime.to_rfc3339()),
            });
        }
    }
    let report = DiscoveryReport {
        seconds: t.elapsed().as_secs_f64(),
        returned_items,
        selected_items,
        selected_assets: assets.len(),
        attempts,
    };
    // Always use a polygon so geographic bounds receive a true AOI mask in native CRS.
    let wgs_json: Value = serde_json::from_str(&wgs.json()?)?;
    let core = Request {
        source: Source::Assets { assets },
        spatial: Spatial::Polygon {
            crs: "EPSG:4326".into(),
            coordinates: wgs_json["coordinates"].clone(),
        },
        time: Some(Time::Interval {
            start: r.start.clone(),
            end: r.end.clone(),
        }),
        variables: vec![Variable::Band { index: 1 }],
        quality: vec![],
        grid: Grid::Native,
        operations: vec![],
        sink: Sink::GeoTiffDirectory {
            path: r.output.clone(),
        },
        budget: Budget {
            buffer_bytes: l.buffer_bytes,
            max_nodes: l.max_nodes,
            chunk_edge: l.chunk_edge,
        },
    };
    crate::planner::validate(&core)?;
    Ok(Discovered {
        request: r.clone(),
        sources,
        core,
        geographic_bounds: b,
        report,
    })
}
pub fn prepare(request: &StacRequest, cache: &mut LayoutCache) -> Result<Prepared> {
    discover(request)?.prepare(cache)
}

/// Recheck pinned COG versions without changing pins or touching GDAL caches.
/// A successful read is still provisional until this final HEAD pass succeeds.
pub(crate) fn validate_identities(core: &Request, limits: &Limits) -> Result<()> {
    remote_identities(core, limits).map(|_| ())
}

pub(crate) fn resolve_identities(
    core: &mut Request,
    sources: &mut [SourceReference],
    limits: &Limits,
) -> Result<()> {
    let identities = remote_identities(core, limits)?;
    let Source::Assets { assets } = &mut core.source else {
        return fail("missing runtime asset bindings");
    };
    for (i, identity) in identities {
        let asset = &mut assets[i];
        crate::cog::clear_remote_cache(asset)?;
        asset.etag = Some(identity.etag.clone());
        if let Some(source) = sources.get_mut(i) {
            source.object_identity = Some(identity);
        }
    }
    Ok(())
}

fn remote_identities(core: &Request, limits: &Limits) -> Result<Vec<(usize, ObjectIdentity)>> {
    let Source::Assets { assets } = &core.source else {
        return fail("missing runtime asset bindings");
    };
    let remote: Vec<_> = assets
        .iter()
        .enumerate()
        .filter(|(_, a)| a.format == crate::Format::Cog && crate::cog::remote(&a.locator))
        .collect();
    if remote.is_empty() {
        return Ok(Vec::new());
    }
    let started = Instant::now();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let client = stac_io::api::ClientBuilder::new()
        .timeout(Duration::from_secs(limits.timeout_seconds))
        .user_agent("tile-ripper/0.1")
        .build()?;
    let width = crate::storage::worker_count()?;
    let result = runtime.block_on(async {
        let mut identities = Vec::with_capacity(remote.len());
        // A bounded batch uses one pooled async client, and never opens GDAL
        // handles concurrently. Conservative global <= per-origin width respects
        // both operation limits even when a batch spans several origins.
        for batch in remote.chunks(width) {
            let mut pending = tokio::task::JoinSet::new();
            for (i, asset) in batch {
                let (i, asset, client) = (*i, (*asset).clone(), client.clone());
                let retries = limits.max_retries;
                pending.spawn(async move {
                    let result = async {
                        let mut attempt = 0;
                        let response = loop {
                            attempt += 1;
                            match client.head(&asset.locator).send().await {
                                Ok(r) if r.status().is_success() => break r,
                                Ok(r) if r.status().is_client_error() && r.status().as_u16() != 429 => {
                                    return fail("asset identity HEAD rejected by provider (credentials omitted)")
                                }
                                _ if attempt <= retries => {
                                    // Async delay lets other admitted HEADs progress.
                                    tokio::time::sleep(Duration::from_secs(u64::from(attempt))).await;
                                }
                                _ => return fail("asset identity HEAD failed after bounded retries (credentials omitted)"),
                            }
                        };
                        let etag = response.headers().get("etag")
                            .and_then(|v| v.to_str().ok())
                            .filter(|v| !v.is_empty() && !v.starts_with("W/"))
                            .ok_or("supported source must expose a strong ETag for conditional extraction")?.to_owned();
                        if asset.etag.as_ref().is_some_and(|expected| *expected != etag) {
                            return fail("source version conflict: requested ETag differs from current source");
                        }
                        let bytes = response.headers().get("content-length")
                            .and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok())
                            .ok_or("source HEAD missing content-length")?;
                        Ok(ObjectIdentity { etag, bytes })
                    }.await;
                    // Keep error ordering deterministic as well as source order.
                    (i, result)
                });
            }
            let mut completed = Vec::with_capacity(batch.len());
            while let Some(result) = pending.join_next().await {
                completed.push(result.map_err(|_| "COG identity worker failed")?);
            }
            completed.sort_by_key(|(i, _)| *i);
            for (i, result) in completed { identities.push((i, result?)); }
        }
        Ok(identities)
    });
    crate::diagnostics::profile_stage("cog_identity_batch", started.elapsed().as_secs_f64());
    result
}

#[cfg(not(feature = "gdal"))]
fn configure(_: &Limits) -> Result<()> {
    Ok(())
}
#[cfg(not(feature = "gdal"))]
impl Prepared {
    pub fn fetch(&self) -> Result<Value> {
        fail("GeoTIFF output requires the gdal feature")
    }
}
