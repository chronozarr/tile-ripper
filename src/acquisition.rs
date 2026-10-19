//! One acquisition lifecycle. Inspection documents cannot be deserialized/replayed.
use crate::{cog::LayoutCache, model::*, stac_workflow, Chunk, ExecutionReport, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum OutputStrategy {
    Callback,
    GeoTiffChunks,
    GeoTiffAssets,
    ZarrGroups,
}
fn strategy(runtime: &Runtime, sink: &Sink) -> OutputStrategy {
    if matches!(sink, Sink::Callback) {
        return OutputStrategy::Callback;
    }
    match runtime {
        Runtime::Catalog(_) => OutputStrategy::GeoTiffAssets,
        Runtime::Direct { request, .. } => {
            if matches!(request.sink, Sink::ZarrDirectory { .. }) {
                OutputStrategy::ZarrGroups
            } else {
                OutputStrategy::GeoTiffChunks
            }
        }
    }
}
enum Runtime {
    Direct {
        request: Box<Request>,
        plan: Plan,
        versions: Versions,
        sources: crate::storage::Sources,
        planning_seconds: f64,
    },
    Catalog(Box<stac_workflow::Prepared>),
}
/// Runtime bindings and source-version checks stay private and nonserializable.
pub struct Prepared {
    runtime: Runtime,
    inspection: Value,
    output: Sink,
}
impl Prepared {
    pub fn plan(&self) -> &Plan {
        match &self.runtime {
            Runtime::Direct { plan, .. } => plan,
            Runtime::Catalog(p) => p.plan(),
        }
    }
    pub fn explain(&self) -> Value {
        self.inspection.clone()
    }
    pub fn human(&self) -> String {
        let mut s = String::from("Tile-Ripper inspection (not a saved execution plan)\n");
        for selection in &self.plan().selected {
            let backend = &selection.capabilities.decoding;
            let variable = selection
                .layout
                .as_ref()
                .map(|l| match &l.variable {
                    Variable::Band { index } => format!("band {index}"),
                    Variable::Dataset { path } => path.clone(),
                    Variable::Named { name } => name.clone(),
                    Variable::Polarization { name } => name.clone(),
                })
                .unwrap_or_default();
            let source = self.inspection["sources"]
                .as_array()
                .and_then(|a| a.iter().find(|v| v["asset_id"] == selection.asset_id));
            let label = source
                .and_then(|v| v["item_id"].as_str())
                .unwrap_or(&selection.asset_id);
            let variable = source
                .and_then(|v| v["requested_band"].as_str())
                .unwrap_or(&variable);
            s += &format!(
                "  {} / {}: {} x {} native pixels; {}; {} timestamp(s)\n",
                label,
                variable,
                selection.window.width,
                selection.window.height,
                backend,
                selection.timestamps.len()
            );
            for time in &selection.timestamps {
                s += &format!("    {time}\n");
            }
        }
        let reads = &self.inspection["expected_reads"];
        s += &format!(
            "Expected data-library reads: {} local, {} remote (estimates; metadata excluded).\n",
            reads["local_library_reads"], reads["remote_library_reads"]
        );
        let bytes = self
            .plan()
            .estimates
            .selected_compressed_payload_bytes
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".into());
        let cache = self.inspection["metadata_cache"].as_array();
        let cached = cache.map_or(0, |v| v.iter().filter(|v| v["cached"] == true).count());
        s+=&format!("Estimated unique encoded data bytes: {bytes}\nMetadata cache: {cached} reused inspections; remaining metadata inspected now.\nNative grids; no resampling, calibration or compositing.\nPhysical tile/chunk selections and native CRS/window details: --json.\n");
        if self.plan().selected.is_empty() {
            s += "Empty selection; fetch writes a completion manifest only.\n";
        }
        for limitation in &self.plan().limitations {
            s += &format!("  Limit: {limitation}\n");
        }
        s
    }
    pub fn execute(&self, sink: impl FnMut(Chunk) -> Result<()>) -> Result<ExecutionReport> {
        let _quiet = crate::diagnostics::QuietNative::enter();
        match &self.runtime {
            Runtime::Direct {
                request,
                plan,
                versions,
                sources,
                planning_seconds,
            } => {
                versions.validate()?;
                sources.validate()?;
                let mut report =
                    crate::execute::execute_on(request, plan, *planning_seconds, sources, sink)
                        .map_err(|e| safe_error(request, "execution", e))?;
                versions.validate()?;
                sources.validate()?;
                report.transport = sources.stats();
                Ok(report)
            }
            Runtime::Catalog(p) => p.execute(sink),
        }
    }
    /// Native format writers are retained. Only the provenance envelope is unified.
    pub fn fetch(&self) -> Result<Value> {
        let _quiet = crate::diagnostics::QuietNative::enter();
        let fetch_started = Instant::now();
        destination(&self.output)?;
        let (root, legacy) = match &self.runtime {
            Runtime::Direct {
                request,
                plan,
                versions,
                sources,
                planning_seconds,
            } => {
                versions.validate()?;
                sources.validate()?;
                let root = destination(&request.sink)?;
                if Path::new(root).exists() {
                    return Err("output directory already exists; choose a new directory".into());
                }
                if let Some(parent) = Path::new(root)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                {
                    fs::create_dir_all(parent)?;
                }
                let materialization_prefix = fetch_started.elapsed().as_secs_f64();
                crate::execute::fetch_on(request, plan, *planning_seconds, sources)
                    .map_err(|e| safe_error(request, "fetch", e))?;
                // A successful native writer owns this new directory; cleanup is safe.
                if let Err(e) = versions.validate().and_then(|_| sources.validate()) {
                    let _ = fs::remove_dir_all(root);
                    return Err(e);
                }
                (
                    root.to_owned(),
                    fs::read(Path::new(root).join("manifest.json")).and_then(|bytes| {
                        let mut value: Value = serde_json::from_slice(&bytes)?;
                        value["observed"]["transport"] = sources.stats();
                        if let Some(seconds) = value["observed"]["first_output_seconds"].as_f64() {
                            value["observed"]["first_output_seconds"] =
                                json!(seconds + materialization_prefix);
                        }
                        Ok(serde_json::to_vec(&value)?)
                    }),
                )
            }
            Runtime::Catalog(p) => {
                let value = p.fetch()?;
                (p.output().to_owned(), Ok(serde_json::to_vec(&value)?))
            }
        };
        let result = (|| -> Result<Value> {
            let stage = Instant::now();
            let mut manifest: Value = serde_json::from_slice(&legacy?)?;
            let outputs = outputs(self.plan(), strategy(&self.runtime, &self.output));
            manifest["schema_version"] = json!(3);
            manifest["document_type"] = json!("acquisition_result");
            manifest["status"] = json!("complete");
            manifest["sources"] = self.inspection["sources"].clone();
            manifest["outputs"] = outputs;
            manifest["inspection"] = self.inspection.clone();
            manifest["processing"] = json!([
                "native stored values; no calibration",
                "native grids and coordinates; no reprojection, interpolation, mosaic or composite",
                "source nodata preserved; explicit pixel-center AOI masks for COG",
                "missing Zarr chunks use native fill; corrupt objects fail"
            ]);
            manifest["limitations"] = json!(self.plan().limitations);
            sanitize(&mut manifest);
            crate::diagnostics::profile_stage(
                "common_manifest_build",
                stage.elapsed().as_secs_f64(),
            );
            let stage = Instant::now();
            let bytes = serde_json::to_vec_pretty(&manifest)?;
            crate::diagnostics::profile_stage(
                "common_manifest_serialization",
                stage.elapsed().as_secs_f64(),
            );
            let stage = Instant::now();
            let temporary = Path::new(&root).join(".manifest.tmp");
            fs::write(&temporary, bytes)?;
            fs::rename(temporary, Path::new(&root).join("manifest.json"))?;
            crate::diagnostics::profile_stage(
                "common_manifest_write_rename",
                stage.elapsed().as_secs_f64(),
            );
            Ok(manifest)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&root);
        }
        result
    }
}
pub fn plan(request: &Request, cache: &mut LayoutCache) -> Result<Value> {
    Ok(prepare(request, cache)?.explain())
}
pub fn fetch(request: &Request, cache: &mut LayoutCache) -> Result<Value> {
    prepare(request, cache)?.fetch()
}
pub fn prepare(request: &Request, cache: &mut LayoutCache) -> Result<Prepared> {
    let _quiet = crate::diagnostics::QuietNative::enter();
    prepare_inner(request, cache).map_err(|e| safe_error(request, "prepare", e))
}
fn prepare_inner(request: &Request, cache: &mut LayoutCache) -> Result<Prepared> {
    crate::planner::validate_common(request)?;
    let runtime = match &request.source {
        Source::Stac { .. } | Source::Catalog { .. } => {
            Runtime::Catalog(Box::new(stac_workflow::prepare(&as_stac(request)?, cache)?))
        }
        Source::Assets { assets } => {
            crate::planner::validate(request)?;
            if (assets.iter().any(|a| a.format == Format::ZarrV2)
                && matches!(request.sink, Sink::GeoTiffDirectory { .. }))
                || (assets.iter().any(|a| a.format == Format::Cog)
                    && matches!(request.sink, Sink::ZarrDirectory { .. }))
            {
                return Err("output capability unsupported: choose directory to preserve the native source format".into());
            }
            let mut request = request.clone();
            request.sink = match &request.sink {
                Sink::Directory { path } => {
                    if assets.iter().any(|a| a.format == Format::ZarrV2) {
                        if assets.iter().any(|a| a.format != Format::ZarrV2) {
                            return Err("mixed formats require separate native requests".into());
                        }
                        Sink::ZarrDirectory { path: path.clone() }
                    } else {
                        Sink::GeoTiffDirectory { path: path.clone() }
                    }
                }
                other => other.clone(),
            };
            let t = Instant::now();
            // Capture relevant metadata before inspection, then add selected data keys.
            let mut versions = Versions::metadata(&request)?;
            if assets
                .iter()
                .any(|a| a.format == Format::Cog && crate::cog::remote(&a.locator))
            {
                stac_workflow::resolve_identities(&mut request, &mut [], &Default::default())?;
            }
            let sources = crate::storage::Sources::new(&request)?;
            let plan = crate::planner::plan_on(&request, cache, &sources)?;
            sources.seal()?;
            versions.validate()?;
            versions.add_data(&request, &plan)?;
            Runtime::Direct {
                request: Box::new(request),
                plan,
                versions,
                sources,
                planning_seconds: t.elapsed().as_secs_f64(),
            }
        }
    };
    let (p, sources, discovery, planning) = match &runtime {
        Runtime::Catalog(p) => (
            p.plan(),
            json!(p
                .sources()
                .iter()
                .map(|source| {
                    let mut v = json!(source);
                    v["format"] = json!("cog");
                    v
                })
                .collect::<Vec<_>>()),
            json!(p.discovery()),
            p.planning_seconds,
        ),
        Runtime::Direct {
            request,
            plan,
            planning_seconds,
            versions,
            sources,
        } => {
            let Source::Assets { assets } = &request.source else {
                unreachable!()
            };
            let sources:Vec<_>=assets.iter().map(|a|json!({"asset_id":a.id,"format":a.format,"reference":if crate::cog::remote(&a.locator){stac_workflow::safe_reference(&a.locator)}else{a.locator.clone()},"datetime":a.datetime,"object_version":a.etag,"local_identity":versions.describe(Path::new(&a.locator)),"object_identities":sources.describe(&a.id),"version_validation":if crate::cog::remote(&a.locator){"conditional ETag"}else{"local size and modification timestamp"}})).collect();
            (
                plan,
                json!(sources),
                json!({"seconds":0.,"kind":"explicit_assets","selected_assets":assets.len()}),
                *planning_seconds,
            )
        }
    };
    let mut local = 0;
    let mut remote = 0;
    for n in &p.nodes {
        if matches!(n.operation, Operation::Read { .. }) {
            let is_remote = match &runtime {
                Runtime::Catalog(_) => true,
                Runtime::Direct { request, .. } => {
                    let Source::Assets { assets } = &request.source else {
                        unreachable!()
                    };
                    assets
                        .iter()
                        .any(|a| a.id == n.asset_id && crate::cog::remote(&a.locator))
                }
            };
            if is_remote {
                remote += 1;
            } else {
                local += 1;
            }
        }
    }
    let mut inspection = json!({"schema_version":3,"document_type":"inspection_only","saved_plan_execution_supported":false,"sources":sources,"discovery":discovery,"planning_seconds":planning,"plan":p,"expected_reads":{"local_library_reads":local,"remote_library_reads":remote,"note":"estimates of library calls; COG HTTP grouping is controlled by GDAL; Zarr data keys are exact, metadata/coordinate reads excluded"},"transformations":p.selected.iter().filter(|s|s.mask_geometry.is_some()).map(|s|json!({"asset_id":s.asset_id,"operation":"AOI pixel-center mask; geometry-only CRS mapping as required","raster_resampling":false})).collect::<Vec<_>>(),"unsupported_operations":p.selected.iter().filter(|s|!s.capabilities.materialization).map(|s|json!({"asset_id":s.asset_id,"operation":"materialize","reason":"reader supports inspection only"})).collect::<Vec<_>>(),"observed_planning":{"discovery_seconds":discovery["seconds"],"metadata_and_plan_seconds":planning},"output_strategy":strategy(&runtime,&request.sink),"expected_outputs":outputs(p,strategy(&runtime,&request.sink)),"metadata_cache":p.nodes.iter().filter_map(|n|match n.operation{Operation::Inspect{cached}=>Some(json!({"asset_id":n.asset_id,"cached":cached})),_=>None}).collect::<Vec<_>>()});
    if let Runtime::Direct { sources, .. } = &runtime {
        inspection["transport"] = sources.stats();
    }
    sanitize(&mut inspection);
    Ok(Prepared {
        runtime,
        inspection,
        output: request.sink.clone(),
    })
}
fn destination(s: &Sink) -> Result<&str> {
    match s {
        Sink::Directory { path }
        | Sink::GeoTiffDirectory { path }
        | Sink::ZarrDirectory { path } => Ok(path),
        Sink::Callback => {
            Err("fetch requires a directory output; use execute for callbacks".into())
        }
    }
}
fn as_stac(r: &Request) -> Result<stac_workflow::StacRequest> {
    let (endpoint, collection, mut limits) = match &r.source {
        Source::Stac {
            endpoint,
            collection,
            limits,
        } => (
            endpoint,
            collection,
            stac_workflow::Limits {
                max_items: limits.max_items,
                page_size: limits.page_size,
                max_assets: limits.max_assets,
                max_output_pixels: limits.max_output_pixels,
                gdal_cache_mb: limits.gdal_cache_mb,
                max_retries: limits.max_retries,
                timeout_seconds: limits.timeout_seconds,
                ..Default::default()
            },
        ),
        Source::Catalog {
            endpoint,
            collection,
        } => (endpoint, collection, Default::default()),
        _ => return Err("not a STAC source".into()),
    };
    let Some(Time::Interval { start, end }) = &r.time else {
        return Err("STAC discovery requires an explicit inclusive time interval".into());
    };
    let bands = r
        .variables
        .iter()
        .map(|v| match v {
            Variable::Named { name } => Ok(name.clone()),
            _ => Err("STAC requires named band selectors (kind: named)".into()),
        })
        .collect::<Result<Vec<_>>>()?;
    limits.buffer_bytes = r.budget.buffer_bytes;
    limits.chunk_edge = r.budget.chunk_edge;
    limits.max_nodes = r.budget.max_nodes;
    if matches!(r.sink, Sink::ZarrDirectory { .. }) {
        return Err("STAC/COG cannot write a Zarr sink; choose directory/native output".into());
    }
    let output = match &r.sink {
        Sink::Callback => "unused-callback".into(),
        s => destination(s)?.into(),
    };
    Ok(stac_workflow::StacRequest {
        endpoint: endpoint.clone(),
        collection: collection.clone(),
        aoi: r.spatial.clone(),
        start: start.clone(),
        end: end.clone(),
        bands,
        output,
        limits,
        provider: Default::default(),
    })
}
/// Explicit migration from the old STAC facade, also accepted by the CLI aliases.
pub fn from_stac(r: stac_workflow::StacRequest) -> Request {
    Request {
        source: Source::Stac {
            endpoint: r.endpoint,
            collection: r.collection,
            limits: CatalogLimits {
                max_items: r.limits.max_items,
                page_size: r.limits.page_size,
                max_assets: r.limits.max_assets,
                max_output_pixels: r.limits.max_output_pixels,
                gdal_cache_mb: r.limits.gdal_cache_mb,
                max_retries: r.limits.max_retries,
                timeout_seconds: r.limits.timeout_seconds,
            },
        },
        spatial: r.aoi,
        time: Some(Time::Interval {
            start: r.start,
            end: r.end,
        }),
        variables: r
            .bands
            .into_iter()
            .map(|name| Variable::Named { name })
            .collect(),
        quality: vec![],
        grid: Grid::Native,
        operations: vec![],
        sink: Sink::Directory { path: r.output },
        budget: Budget {
            buffer_bytes: r.limits.buffer_bytes,
            max_nodes: r.limits.max_nodes,
            chunk_edge: r.limits.chunk_edge,
        },
    }
}
fn outputs(p: &Plan, mode: OutputStrategy) -> Value {
    if matches!(mode, OutputStrategy::Callback) {
        return json!([]);
    }
    let zarr = matches!(mode, OutputStrategy::ZarrGroups);
    let mut out = Vec::new();
    for s in &p.selected {
        if !s.capabilities.materialization {
            continue;
        }
        let Some(l) = &s.layout else {
            continue;
        };
        let mut record = json!({"asset_id":s.asset_id,"variable":l.variable,"timestamps":s.timestamps,"window":s.window,"native_grid":{"crs":l.crs,"transform":crate::spatial::window_transform(l,&s.window)},"dtype":"uint16","native_dtype":l.dtype,"nodata":l.nodata,"scale":l.scale,"offset":l.offset,"aoi_mask_applied":s.mask_geometry.is_some()});
        if zarr {
            record["file"] = json!(format!("{}.zarr", s.asset_id));
            record["dimensions"] = json!({"names":["time","y","x"],"shape":[s.timestamps.len(),s.window.height,s.window.width]});
            if let Variable::Dataset { path } = &l.variable {
                record["dataset"] = json!(path.trim_start_matches('/'));
            }
            out.push(record);
        } else if matches!(mode, OutputStrategy::GeoTiffAssets) {
            record["file"] = json!(format!("{}.tif", s.asset_id));
            record["dimensions"] =
                json!({"names":["y","x"],"shape":[s.window.height,s.window.width]});
            out.push(record);
        } else {
            for n in &p.nodes {
                if n.asset_id != s.asset_id {
                    continue;
                }
                if let Operation::Read {
                    read: Read::LibraryWindow { window, band, .. },
                } = &n.operation
                {
                    if l.variable != (Variable::Band { index: *band }) {
                        continue;
                    }
                    let mut r = record.clone();
                    r["file"] = json!(format!("chunk-{}.tif", n.id));
                    r["window"] = json!(window);
                    r["dimensions"] =
                        json!({"names":["y","x"],"shape":[window.height,window.width]});
                    r["native_grid"]["transform"] =
                        json!(crate::spatial::window_transform(l, window));
                    out.push(r);
                }
            }
        }
    }
    json!(out)
}
fn sanitize(value: &mut Value) {
    match value {
        Value::String(s) if s.contains("://") => *s = stac_workflow::safe_reference(s),
        Value::Array(a) => {
            for v in a {
                sanitize(v);
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                sanitize(v);
            }
        }
        _ => {}
    }
}
fn safe_error(
    r: &Request,
    stage: &str,
    e: Box<dyn std::error::Error + Send + Sync>,
) -> Box<dyn std::error::Error + Send + Sync> {
    let mut message = e.to_string();
    let locators: Vec<&str> = match &r.source {
        Source::Assets { assets } => assets.iter().map(|a| a.locator.as_str()).collect(),
        Source::Stac { endpoint, .. } | Source::Catalog { endpoint, .. } => vec![endpoint],
    };
    for locator in locators {
        message = message.replace(locator, "[source]");
        if let Some((_, query)) = locator.split_once('?') {
            for kv in query.split('&') {
                if let Some((_, value)) = kv.split_once('=') {
                    if !value.is_empty() {
                        message = message.replace(value, "[redacted]");
                    }
                }
            }
        }
    }
    // Native errors sometimes contain resolved URLs not present in the input catalog request.
    for scheme in ["https://", "http://"] {
        while let Some(start) = message.find(scheme) {
            let end = message[start..]
                .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
                .map_or(message.len(), |n| start + n);
            message.replace_range(start..end, "[source]");
        }
    }
    format!("{stage}: {message}").into()
}

#[derive(Clone, PartialEq)]
struct Identity {
    size: u64,
    modified: std::time::SystemTime,
}
#[derive(Default)]
struct Versions {
    files: std::collections::BTreeMap<PathBuf, Option<Identity>>,
}
impl Versions {
    fn describe(&self, root: &Path) -> Vec<Value> {
        self.files.iter().filter_map(|(path,id)|path.strip_prefix(root).ok().map(|key|json!({"key":if key.as_os_str().is_empty(){".".into()}else{key.to_string_lossy().into_owned()},"present":id.is_some(),"bytes":id.as_ref().map(|v|v.size),"modified_unix_ns":id.as_ref().and_then(|v|v.modified.duration_since(std::time::UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string())}))).collect()
    }
    fn add(&mut self, path: PathBuf) -> Result<()> {
        let id = identity(&path)?;
        self.files.insert(path, id);
        Ok(())
    }
    fn metadata(r: &Request) -> Result<Self> {
        let Source::Assets { assets } = &r.source else {
            unreachable!()
        };
        let mut v = Self::default();
        for a in assets {
            if crate::cog::remote(&a.locator) {
                continue;
            }
            let root = Path::new(&a.locator);
            if a.format == Format::Cog {
                v.add(root.into())?;
                continue;
            }
            for file in [".zgroup", ".zattrs", ".zarray", "zarr.json"] {
                v.add(root.join(file))?;
            }
            for axis in ["time", "y", "x"] {
                let dir = root.join(axis);
                if dir.is_dir() {
                    v.axis(&dir, r.budget.max_nodes)?;
                }
            }
            for var in &r.variables {
                if let Variable::Dataset { path } = var {
                    let name = path.trim_start_matches('/');
                    if name == ".." || name.contains('/') || name.contains('\\') {
                        return Err("unsafe dataset path".into());
                    }
                    for file in [".zarray", ".zattrs", "zarr.json"] {
                        v.add(root.join(name).join(file))?;
                    }
                }
            }
        }
        Ok(v)
    }
    fn axis(&mut self, dir: &Path, cap: usize) -> Result<()> {
        self.add(dir.to_path_buf())?;
        for entry in fs::read_dir(dir)? {
            let p = entry?.path();
            if fs::symlink_metadata(&p)?.file_type().is_symlink() {
                return Err("symlink stores are unsupported for prepared snapshots".into());
            }
            if p.is_dir() {
                self.axis(&p, cap)?;
            } else {
                self.add(p)?;
            }
            if self.files.len() > cap {
                return Err("source metadata snapshot exceeds max_nodes".into());
            }
        }
        Ok(())
    }
    fn add_data(&mut self, r: &Request, p: &Plan) -> Result<()> {
        let Source::Assets { assets } = &r.source else {
            unreachable!()
        };
        for n in &p.nodes {
            if let Operation::Read {
                read: Read::ChunkObject { key, .. },
            } = &n.operation
            {
                let a = assets
                    .iter()
                    .find(|a| a.id == n.asset_id)
                    .ok_or("asset binding missing")?;
                if !crate::cog::remote(&a.locator) {
                    self.add(Path::new(&a.locator).join(key))?;
                }
            }
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        for (p, id) in &self.files {
            if identity(p)? != *id {
                return Err("source version conflict: local data or metadata changed since preparation; prepare again".into());
            }
        }
        Ok(())
    }
}
fn identity(p: &Path) -> Result<Option<Identity>> {
    match fs::metadata(p) {
        Ok(m) => Ok(Some(Identity {
            size: m.len(),
            modified: m.modified()?,
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("cannot inspect source identity".into()),
    }
}

/// Catalog-only discovery: no raster metadata opens or pixel retrieval.
pub fn discover(r: &Request) -> Result<Value> {
    let _quiet = crate::diagnostics::QuietNative::enter();
    crate::planner::validate_common(r)?;
    let d = stac_workflow::discover(&as_stac(r)?).map_err(|e| safe_error(r, "discovery", e))?;
    let mut v = json!({"schema_version":3,"document_type":"discovery_only","saved_plan_execution_supported":false,"discovery":d.report,"sources":d.sources(),"raster_metadata_opened":false});
    sanitize(&mut v);
    Ok(v)
}

#[cfg(test)]
mod tests {
    #[test]
    fn nested_provenance_urls_are_sanitized() {
        let mut v = serde_json::json!({"ref":"https://name:password@example.org/a?token=SECRET#fragment","nested":[{"url":"https://x/b?sig=SECRET"}]});
        super::sanitize(&mut v);
        let text = v.to_string();
        assert!(!text.contains("SECRET"));
        assert!(!text.contains("password"));
        assert!(text.contains("https://example.org/a"));
    }
}
