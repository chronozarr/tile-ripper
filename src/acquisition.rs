//! One acquisition lifecycle. Inspection documents cannot be deserialized/replayed.
use crate::{
    cog::LayoutCache, fail, model::*, output_transaction::Transaction, stac_workflow, Chunk,
    ExecutionReport, Result,
};
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
    ZarrObservations,
}
fn strategy(runtime: &Runtime, sink: &Sink) -> OutputStrategy {
    if matches!(sink, Sink::Callback) {
        return OutputStrategy::Callback;
    }
    if matches!(sink, Sink::ZarrDirectory { .. }) && cog_runtime(runtime) {
        return OutputStrategy::ZarrObservations;
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
        sources: Box<crate::storage::Sources>,
        planning_seconds: f64,
    },
    Catalog(Box<stac_workflow::Prepared>),
}
fn cog_runtime(runtime: &Runtime) -> bool {
    match runtime {
        Runtime::Catalog(_) => true,
        Runtime::Direct { request, .. } => {
            matches!(&request.source, Source::Assets { assets } if assets.iter().all(|a| a.format == Format::Cog))
        }
    }
}
/// Runtime bindings and source-version checks stay private and nonserializable.
#[derive(Clone)]
pub struct Prepared {
    runtime: std::sync::Arc<Runtime>,
    runtime_config: std::sync::Arc<crate::runtime_config::RuntimeConfig>,
    inspection: std::sync::Arc<Value>,
    output: Sink,
}
impl Prepared {
    /// Product metadata is attached only after physical inspection and profile checks.
    pub(crate) fn with_product_metadata(mut self, sources: Vec<Value>, product: Value) -> Self {
        let mut inspection = self.explain();
        if let Some(existing) = inspection["sources"].as_array_mut() {
            for source in existing {
                if let Some(extra) = sources.iter().find(|v| v["asset_id"] == source["asset_id"]) {
                    if let (Some(dst), Some(src)) = (source.as_object_mut(), extra.as_object()) {
                        dst.extend(src.clone());
                    }
                }
            }
        }
        if let Some(selection_time) = product.get("selection_time") {
            inspection["logical_request"]["time"] = selection_time.clone();
            inspection["logical_request"]["temporal_interpretation"] =
                product["temporal_interpretation"].clone();
        }
        inspection["product"] = product;
        sanitize(&mut inspection);
        self.inspection = std::sync::Arc::new(inspection);
        self
    }
    pub fn plan(&self) -> &Plan {
        match self.runtime.as_ref() {
            Runtime::Direct { plan, .. } => plan,
            Runtime::Catalog(p) => p.plan(),
        }
    }
    pub fn explain(&self) -> Value {
        self.inspection.as_ref().clone()
    }
    pub(crate) fn output_budget(&self) -> Result<Budget> {
        Ok(serde_json::from_value(
            self.inspection["output_budget"].clone(),
        )?)
    }
    pub fn human(&self) -> String {
        let mut s = String::from("Tile-Ripper inspection (not a saved execution plan)\n");
        let logical = &self.inspection["logical_request"];
        let spatial = &logical["spatial"];
        if spatial["kind"] == "bounds" {
            s += &format!(
                "Requested bounds {} in {}\n",
                spatial["bbox"], spatial["crs"]
            );
        } else if spatial["kind"] == "array_slice" {
            s += &format!(
                "Resolved source-global indices: start {}, count {}, step {}\n",
                spatial["start"], spatial["count"], spatial["step"]
            );
        } else {
            s += &format!(
                "Requested Polygon in {} (geometry in --json)\n",
                spatial["crs"]
            );
        }
        let time = &logical["time"];
        if time["kind"] == "interval" {
            s += &format!(
                "Requested interval {} through {} (inclusive)\n",
                time["start"], time["end"]
            );
        } else if let Some(values) = time["values"].as_array() {
            s += &format!("Requested {} explicit timestamp(s)\n", values.len());
        }
        let routing = &self.inspection["product"]["routing"];
        if let Some(provider) = routing["selected"].as_str() {
            s += &format!(
                "Provider: {provider}; decision: {}; credentials required: {}\n{}\n",
                routing["basis"].as_str().unwrap_or("unknown"),
                routing["credentials_required"],
                routing["rationale"].as_str().unwrap_or("")
            );
            s += &format!(
                "Cross-provider equivalence: {}. {}\n",
                routing["equivalence"]["classification"]
                    .as_str()
                    .unwrap_or("unknown"),
                routing["fallback"].as_str().unwrap_or("")
            );
            if let Some(reason) = routing["eligibility"]["decision"].as_str() {
                s += &format!("Coverage decision: {reason}\n");
            }
            if let Some(catalogs) = routing["eligibility"]["catalogs"].as_array() {
                for catalog in catalogs {
                    s += &format!(
                        "  Catalog {}: {}; count {}\n",
                        catalog["provider"].as_str().unwrap_or(""),
                        catalog["status"].as_str().unwrap_or(""),
                        catalog["count"]
                    );
                }
            }
            if let Some(routes) = routing["routes"].as_array() {
                for route in routes {
                    s += &format!(
                        "  Route {}: {}; {}\n",
                        route["provider"].as_str().unwrap_or(""),
                        route["credential_requirement"].as_str().unwrap_or(""),
                        route["availability"].as_str().unwrap_or("")
                    );
                }
            }
        }
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
            if let Some(source) = source.filter(|source| source["provider"].is_string()) {
                s += &format!(
                    "    Pinned {} {} ETag {}\n",
                    source["provider"].as_str().unwrap_or(""),
                    source["canonical_locator"].as_str().unwrap_or(""),
                    source["object_version"]
                );
            }
            if let Some(source) = source.filter(|source| source["temporal_extent"].is_object()) {
                let extent = &source["temporal_extent"];
                s += &format!("    Annual summary {}; actual source coverage {} through {}; time coordinate labels coverage start\n",
                    extent["summary_year"], extent["coverage_start"], extent["coverage_end"]);
            }
            for time in selection.timestamps.iter().take(8) {
                s += &format!("    {time}\n");
            }
            if selection.timestamps.len() > 8 {
                s += &format!(
                    "    … {} more timestamp(s); full list in --json\n",
                    selection.timestamps.len() - 8
                );
            }
        }
        for selection in &self.plan().selected {
            if let Some(layout) = &selection.layout {
                s += &format!(
                    "    Native CRS {}; source window x={} y={} width={} height={}\n",
                    layout.crs,
                    selection.window.x,
                    selection.window.y,
                    selection.window.width,
                    selection.window.height
                );
            }
        }
        s += &format!("Estimated logical output value bytes: {} (encoding, coordinates and metadata excluded).\n", self.inspection["output_estimates"]["native_value_bytes"]);
        s += "Source identities are checked before and after execution; per-object validation is not an atomic dataset snapshot.\n";
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
    pub fn into_stream(
        self,
        options: crate::stream::StreamOptions,
    ) -> Result<crate::stream::ChunkStream> {
        crate::stream::ChunkStream::start(self, options)
    }
    /// Share cancellation with a host application's signal or request lifecycle.
    /// As with every stream, delivered chunks remain provisional until finish.
    pub fn into_stream_cancellable(
        self,
        options: crate::stream::StreamOptions,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<crate::stream::ChunkStream> {
        crate::stream::ChunkStream::start_cancellable(self, options, cancelled)
    }
    pub fn execute(&self, sink: impl FnMut(Chunk) -> Result<()>) -> Result<ExecutionReport> {
        self.execute_cancellable(&Default::default(), sink)
    }
    pub(crate) fn execute_cancellable(
        &self,
        cancelled: &std::sync::atomic::AtomicBool,
        sink: impl FnMut(Chunk) -> Result<()>,
    ) -> Result<ExecutionReport> {
        let _runtime = self.runtime_config.enter();
        let _quiet = crate::diagnostics::QuietNative::enter();
        match self.runtime.as_ref() {
            Runtime::Direct {
                request,
                plan,
                versions,
                sources,
                planning_seconds,
            } => {
                sources.phase("pre_execution_validation");
                versions.validate()?;
                sources.validate_cog_identities(request, &Default::default())?;
                sources.validate_cancellable(Some(cancelled))?;
                sources.phase("execution");
                let mut report = crate::scheduler::execute(
                    request,
                    plan,
                    *planning_seconds,
                    sources,
                    cancelled,
                    sink,
                )
                .map_err(|e| safe_error(request, "execution", e))?;
                sources.phase("post_execution_validation");
                versions.validate()?;
                sources.validate_cog_identities(request, &Default::default())?;
                sources.validate_cancellable(Some(cancelled))?;
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("acquisition cancelled".into());
                }
                report.transport = sources.stats();
                Ok(report)
            }
            Runtime::Catalog(p) => p.execute_cancellable(cancelled, sink),
        }
    }
    #[cfg(feature = "native-hdf5")]
    pub(crate) fn with_swot_coordinates<T>(
        &self,
        f: impl FnOnce(&[f64], &[f64]) -> Result<T>,
    ) -> Result<T> {
        let Runtime::Direct {
            request, sources, ..
        } = self.runtime.as_ref()
        else {
            return fail("SWOT coordinates require direct HDF5 assets");
        };
        let Source::Assets { assets } = &request.source else {
            return fail("missing SWOT source");
        };
        sources.hdf5.with_coordinates(&assets[0].id, f)
    }
    #[cfg(feature = "native-hdf5")]
    pub(crate) fn swot_working_memory(&self) -> Result<Value> {
        let Runtime::Direct {
            request, sources, ..
        } = self.runtime.as_ref()
        else {
            return fail("SWOT aligned inspection requires direct HDF5 assets");
        };
        let Source::Assets { assets } = &request.source else {
            return fail("missing SWOT source");
        };
        Ok(sources.hdf5.aligned_reservation(&assets[0], request)?.2)
    }
    #[cfg(feature = "native-hdf5")]
    pub(crate) fn execute_swot_cancellable(
        &self,
        cancelled: &std::sync::atomic::AtomicBool,
        sink: impl FnMut([Chunk; 5]) -> Result<()>,
    ) -> Result<ExecutionReport> {
        let _runtime = self.runtime_config.enter();
        let _quiet = crate::diagnostics::QuietNative::enter();
        let Runtime::Direct {
            request,
            versions,
            sources,
            planning_seconds,
            ..
        } = self.runtime.as_ref()
        else {
            return fail("SWOT aligned execution requires direct HDF5 assets");
        };
        let Source::Assets { assets } = &request.source else {
            return fail("missing SWOT source");
        };
        sources.phase("pre_execution_validation");
        versions.validate()?;
        sources.validate_cog_identities(request, &Default::default())?;
        sources.validate_cancellable(Some(cancelled))?;
        let mut report = sources
            .hdf5
            .execute_aligned(&assets[0], request, *planning_seconds, cancelled, sink)
            .map_err(|e| safe_error(request, "execution", e))?;
        sources.phase("post_execution_validation");
        versions.validate()?;
        sources.validate_cog_identities(request, &Default::default())?;
        sources.validate_cancellable(Some(cancelled))?;
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return fail("acquisition cancelled");
        }
        report.transport = sources.stats();
        Ok(report)
    }
    /// Native format writers are retained. Only the provenance envelope is unified.
    pub fn fetch(&self) -> Result<Value> {
        let _runtime = self.runtime_config.enter();
        if self.scientific_output() {
            return self.fetch_cancellable(Default::default());
        }
        let _quiet = crate::diagnostics::QuietNative::enter();
        let fetch_started = Instant::now();
        let transaction = Transaction::new(Path::new(destination(&self.output)?))?;
        let staged_root = transaction.stage().to_path_buf();
        let legacy = match self.runtime.as_ref() {
            Runtime::Direct {
                request,
                plan,
                versions,
                sources,
                planning_seconds,
            } => {
                sources.phase("pre_execution_validation");
                versions.validate()?;
                sources.validate_cog_identities(request, &Default::default())?;
                sources.validate()?;
                sources.phase("execution");
                let mut staged = (**request).clone();
                staged.sink = match &request.sink {
                    Sink::Directory { .. } => Sink::Directory {
                        path: staged_root.to_string_lossy().into_owned(),
                    },
                    Sink::GeoTiffDirectory { .. } => Sink::GeoTiffDirectory {
                        path: staged_root.to_string_lossy().into_owned(),
                    },
                    Sink::ZarrDirectory { .. } => Sink::ZarrDirectory {
                        path: staged_root.to_string_lossy().into_owned(),
                    },
                    Sink::Callback => {
                        return fail("fetch requires a directory output; use execute for callbacks")
                    }
                };
                let materialization_prefix = fetch_started.elapsed().as_secs_f64();
                crate::execute::fetch_on(&staged, plan, *planning_seconds, sources)
                    .map_err(|e| safe_error(request, "fetch", e))?;
                sources.phase("post_execution_validation");
                versions
                    .validate()
                    .and_then(|_| sources.validate_cog_identities(request, &Default::default()))
                    .and_then(|_| sources.validate())?;
                fs::read(staged_root.join("manifest.json")).and_then(|bytes| {
                    let mut value: Value = serde_json::from_slice(&bytes)?;
                    value["observed"]["transport"] = sources.stats();
                    if let Some(seconds) = value["observed"]["first_output_seconds"].as_f64() {
                        value["observed"]["first_output_seconds"] =
                            json!(seconds + materialization_prefix);
                    }
                    Ok(serde_json::to_vec(&value)?)
                })
            }
            Runtime::Catalog(p) => {
                let value = p.fetch_to(&staged_root)?;
                serde_json::to_vec(&value).map_err(Into::into)
            }
        };
        (|| -> Result<Value> {
            let stage = Instant::now();
            let mut manifest: Value = serde_json::from_slice(&legacy?)?;
            let outputs = outputs(self.plan(), strategy(&self.runtime, &self.output));
            manifest["schema_version"] = json!(3);
            manifest["document_type"] = json!("acquisition_result");
            manifest["status"] = json!("complete");
            manifest["sources"] = self.inspection["sources"].clone();
            manifest["outputs"] = outputs;
            manifest["inspection"] = self.explain();
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
            let temporary = staged_root.join(".manifest.tmp");
            fs::write(&temporary, bytes)?;
            fs::rename(temporary, staged_root.join("manifest.json"))?;
            crate::diagnostics::profile_stage(
                "common_manifest_write_rename",
                stage.elapsed().as_secs_f64(),
            );
            transaction.publish(&std::sync::atomic::AtomicBool::new(false))?;
            Ok(manifest)
        })()
    }
    /// COG-to-Zarr materialization uses the public provisional chunk stream and
    /// shares this cancellation flag with its producer. Other sinks keep their
    /// existing fetch interface.
    pub fn fetch_cancellable(
        &self,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Value> {
        let _runtime = self.runtime_config.enter();
        if !self.scientific_output() {
            return Err("cancellable fetch requires COG/STAC with a zarr_directory sink".into());
        }
        crate::scientific_output::fetch(self.clone(), destination(&self.output)?, cancelled)
    }
    pub fn scientific_output(&self) -> bool {
        matches!(
            strategy(&self.runtime, &self.output),
            OutputStrategy::ZarrObservations
        )
    }
    /// Evaluate lossless encoding/chunk layouts through the same transactional
    /// COG sink. Ordinary fetch retains its original uncompressed Zarr v2 layout.
    pub fn fetch_scientific_with_options(
        &self,
        options: crate::ScientificOutputOptions,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Value> {
        let _runtime = self.runtime_config.enter();
        if !self.scientific_output() {
            return Err(
                "scientific output options require COG/STAC with a zarr_directory sink".into(),
            );
        }
        crate::scientific_output::fetch_with_options(
            self.clone(),
            destination(&self.output)?,
            cancelled,
            options,
        )
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
    prepare_inner(request, cache, Default::default()).map_err(|e| safe_error(request, "prepare", e))
}
/// Prepare only when Tile-Ripper was compiled without its `gdal` feature.
/// Keep backend environment selection fixed until this prepared request and its
/// streams are dropped. Independent host-application linkage is not inspected.
pub fn prepare_gdal_free(request: &Request, cache: &mut LayoutCache) -> Result<Prepared> {
    crate::cog::require_gdal_free()?;
    if !matches!(request.source, Source::Assets { .. })
        || matches!(&request.source, Source::Assets { assets } if assets.iter().any(|a| a.format == Format::Cog))
    {
        crate::cog::backend()?;
    }
    prepare(request, cache)
}
/// Prepare one native HDF5 object with a caller-owned authenticated session.
#[cfg(feature = "authenticated-http")]
pub fn prepare_authenticated(
    request: &Request,
    cache: &mut LayoutCache,
    transport: std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
) -> Result<Prepared> {
    if !matches!(&request.source, Source::Assets {assets} if assets.len()==1 && assets[0].format == Format::Hdf5 && transport.accepts(&assets[0].locator))
    {
        return fail("authenticated preparation requires its exact configured native HDF5 object");
    }
    let _quiet = crate::diagnostics::QuietNative::enter();
    prepare_inner(
        request,
        cache,
        crate::http_transport::Transport::Authenticated(transport),
    )
    .map_err(|e| safe_error(request, "prepare", e))
}
/// Prepare a strictly bound set of authenticated remote COG objects.
///
/// Every asset must be a remote COG and have exactly one caller-owned session
/// keyed by its asset ID. Each session remains confined to that asset's
/// canonical URL; the native COG reader retains the map for planning, reads,
/// and pre/post source-version validation.
#[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
pub fn prepare_authenticated_cogs(
    request: &Request,
    cache: &mut LayoutCache,
    sessions: std::collections::BTreeMap<
        String,
        std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
    >,
) -> Result<Prepared> {
    prepare_authenticated_cogs_with_native_selection(
        request,
        cache,
        sessions,
        &crate::planner::NativeSelectionPolicy::default(),
    )
}
/// As above, with a crate-private product policy that can authorize one
/// inspected native grid to use a provider-specific coordinate convention.
/// No public request field can opt into this path.
#[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
pub(crate) fn prepare_authenticated_cogs_with_native_selection(
    request: &Request,
    cache: &mut LayoutCache,
    sessions: std::collections::BTreeMap<
        String,
        std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
    >,
    selection_policy: &crate::planner::NativeSelectionPolicy,
) -> Result<Prepared> {
    if !crate::cog::native_selected() {
        return fail("authenticated COG preparation requires the native COG backend");
    }
    validate_authenticated_cog_sessions(request, &sessions)?;
    for session in sessions.values() {
        session.require_fresh()?;
    }
    crate::planner::validate(request)?;
    let sources = crate::storage::Sources::with_cog_sessions(request, sessions)?;
    let _quiet = crate::diagnostics::QuietNative::enter();
    prepare_on_with_native_selection(
        request,
        cache,
        Default::default(),
        Some(sources),
        selection_policy,
    )
    .map_err(|e| safe_error(request, "prepare", e))
}
#[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
fn validate_authenticated_cog_sessions(
    request: &Request,
    sessions: &std::collections::BTreeMap<
        String,
        std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
    >,
) -> Result<usize> {
    let Source::Assets { assets } = &request.source else {
        return fail("authenticated COG preparation requires explicit assets");
    };
    if assets.is_empty()
        || assets
            .iter()
            .any(|asset| asset.format != Format::Cog || !crate::cog::remote(&asset.locator))
        || sessions.len() != assets.len()
        || assets.iter().any(|asset| {
            sessions
                .get(&asset.id)
                .is_none_or(|session| !session.accepts(&asset.locator))
        })
        || sessions
            .keys()
            .any(|id| !assets.iter().any(|asset| &asset.id == id))
    {
        return fail(
            "authenticated COG preparation requires one exact session for every remote COG asset",
        );
    }
    let reservation = sessions
        .len()
        .checked_mul(crate::authenticated_http::WORKING_RESERVATION)
        .ok_or("authenticated COG transport reservation overflow")?;
    if reservation > request.budget.buffer_bytes / 2 {
        return fail("authenticated COG transports exceed half buffer_bytes");
    }
    Ok(reservation)
}
/// Resume verified source bytes; reconstruct all scientific chunks from the
/// beginning. The caller owns a fresh online session and must finish the stream
/// before committing any result. The checkpoint lock lasts as long as Prepared
/// or any of its streams/clones. Selection/policy conflicts require a new directory.
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
pub fn prepare_authenticated_checkpointed(
    request: &Request,
    cache: &mut LayoutCache,
    session: std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
    options: crate::checkpoint::Options,
) -> Result<Prepared> {
    prepare_checkpointed(request, cache, session, options, Value::Null)
}
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
pub(crate) fn prepare_checkpointed(
    request: &Request,
    cache: &mut LayoutCache,
    session: std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
    options: crate::checkpoint::Options,
    quality: Value,
) -> Result<Prepared> {
    if !matches!(&request.source, Source::Assets {assets} if assets.len()==1 && assets[0].format == Format::Hdf5 && session.accepts(&assets[0].locator))
    {
        return fail(
            "checkpoint preparation requires its exact configured authenticated native HDF5 object",
        );
    }
    crate::planner::validate(request)?;
    if !matches!(request.spatial, Spatial::ArraySlice { .. }) {
        return fail("checkpointed native HDF5 requires an explicit array_slice");
    }
    if options
        .reservation()?
        .checked_add(crate::authenticated_http::WORKING_RESERVATION)
        .is_none_or(|n| n > request.budget.buffer_bytes / 2)
    {
        return fail("checkpoint/transport working reservation exceeds half buffer_bytes");
    }
    session.require_fresh()?;
    let store = std::sync::Arc::new(crate::checkpoint::Store::open(
        options,
        crate::checkpoint::scientific_identity(request, quality)?,
        session.checkpoint_policy()?,
    )?);
    prepare_bound_checkpoint(request, cache, session, store)
}
#[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
pub(crate) fn prepare_bound_checkpoint(
    request: &Request,
    cache: &mut LayoutCache,
    session: std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
    store: std::sync::Arc<crate::checkpoint::Store>,
) -> Result<Prepared> {
    if !matches!(&request.source, Source::Assets {assets} if assets.len()==1 && assets[0].format == Format::Hdf5 && session.accepts(&assets[0].locator))
    {
        return fail(
            "checkpoint preparation requires its exact configured authenticated native HDF5 object",
        );
    }
    crate::planner::validate(request)?;
    if !matches!(request.spatial, Spatial::ArraySlice { .. })
        || store
            .reservation()
            .checked_add(crate::authenticated_http::WORKING_RESERVATION)
            .is_none_or(|n| n > request.budget.buffer_bytes / 2)
    {
        return fail("checkpoint/transport working reservation exceeds half buffer_bytes");
    }
    session.claim_checkpoint()?;
    let _quiet = crate::diagnostics::QuietNative::enter();
    prepare_inner(
        request,
        cache,
        crate::http_transport::Transport::Checkpointed { session, store },
    )
    .map_err(|e| safe_error(request, "prepare", e))
}
fn prepare_inner(
    request: &Request,
    cache: &mut LayoutCache,
    transport: crate::http_transport::Transport,
) -> Result<Prepared> {
    prepare_on(request, cache, transport, None)
}
#[cfg(feature = "native-hdf5")]
pub(crate) fn prepare_inspected(
    request: &Request,
    grid: &crate::hdf5::GridInspection,
) -> Result<Prepared> {
    let sources = crate::storage::Sources::from_hdf5(grid.context(request)?);
    prepare_on(
        request,
        &mut LayoutCache::default(),
        Default::default(),
        Some(sources),
    )
    .map_err(|e| safe_error(request, "prepare", e))
}
#[cfg(feature = "native-hdf5")]
pub(crate) fn prepare_array_inspected(
    request: &Request,
    grid: &crate::hdf5::ArrayInspection,
) -> Result<Prepared> {
    let sources = crate::storage::Sources::from_hdf5(grid.context(request)?);
    prepare_on(
        request,
        &mut LayoutCache::default(),
        Default::default(),
        Some(sources),
    )
    .map_err(|e| safe_error(request, "prepare array", e))
}
fn prepare_on(
    request: &Request,
    cache: &mut LayoutCache,
    transport: crate::http_transport::Transport,
    prepared_sources: Option<crate::storage::Sources>,
) -> Result<Prepared> {
    prepare_on_with_native_selection(
        request,
        cache,
        transport,
        prepared_sources,
        &crate::planner::NativeSelectionPolicy::default(),
    )
}
fn prepare_on_with_native_selection(
    request: &Request,
    cache: &mut LayoutCache,
    transport: crate::http_transport::Transport,
    prepared_sources: Option<crate::storage::Sources>,
    selection_policy: &crate::planner::NativeSelectionPolicy,
) -> Result<Prepared> {
    let runtime_config = crate::runtime_config::RuntimeConfig::capture(request)?;
    let _runtime = runtime_config.enter();
    crate::planner::validate_common(request)?;
    let runtime = match &request.source {
        Source::Stac { .. } | Source::Catalog { .. } => {
            Runtime::Catalog(Box::new(stac_workflow::prepare(&as_stac(request)?, cache)?))
        }
        Source::Assets { assets } => {
            crate::planner::validate(request)?;
            if assets.iter().any(|a| a.format.is_zarr())
                && matches!(request.sink, Sink::GeoTiffDirectory { .. })
            {
                return Err("output capability unsupported: choose directory to preserve the native source format".into());
            }
            let mut request = request.clone();
            request.sink = match &request.sink {
                Sink::Directory { path } => {
                    if assets.iter().any(|a| a.format.is_zarr()) {
                        if assets.iter().any(|a| !a.format.is_zarr()) {
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
                #[cfg(all(feature = "native-cog", feature = "authenticated-http"))]
                if let Some(sources) = prepared_sources.as_ref() {
                    sources.resolve_authenticated_cog_identities(&mut request)?;
                } else {
                    stac_workflow::resolve_identities(&mut request, &mut [], &Default::default())?;
                }
                #[cfg(not(all(feature = "native-cog", feature = "authenticated-http")))]
                stac_workflow::resolve_identities(&mut request, &mut [], &Default::default())?;
            }
            let sources = match prepared_sources {
                Some(sources) => sources,
                None => crate::storage::Sources::with_transport(&request, transport)?,
            };
            let plan = crate::planner::plan_on_with_native_selection(
                &request,
                cache,
                &sources,
                selection_policy,
            )?;
            sources.seal()?;
            versions.validate()?;
            versions.add_data(&request, &plan)?;
            Runtime::Direct {
                request: Box::new(request),
                plan,
                versions,
                sources: Box::new(sources),
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
            let sources:Vec<_>=assets.iter().map(|a|json!({"asset_id":a.id,"format":a.format,"reference":if crate::cog::remote(&a.locator){stac_workflow::safe_reference(&a.locator)}else{a.locator.clone()},"datetime":a.datetime,"object_version":a.etag,"local_identity":versions.describe(Path::new(&a.locator)),"object_identities":sources.describe(&a.id),"zarr_v3":sources.v3.describe(&a.id),"version_validation":if crate::cog::remote(&a.locator){"conditional ETag"}else{"local size and modification timestamp"}})).collect();
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
    let mut inspection = json!({"schema_version":3,"document_type":"inspection_only","saved_plan_execution_supported":false,"sources":sources,"discovery":discovery,"planning_seconds":planning,"plan":p,"expected_reads":{"local_library_reads":local,"remote_library_reads":remote,"note":"estimates of library calls; COG HTTP grouping depends on the selected reader; Zarr data keys are exact, metadata/coordinate reads excluded"},"transformations":p.selected.iter().filter(|s|s.mask_geometry.is_some()).map(|s|json!({"asset_id":s.asset_id,"operation":"AOI pixel-center mask; geometry-only CRS mapping as required","raster_resampling":false})).collect::<Vec<_>>(),"unsupported_operations":p.selected.iter().filter(|s|!s.capabilities.materialization).map(|s|json!({"asset_id":s.asset_id,"operation":"materialize","reason":"reader supports inspection only"})).collect::<Vec<_>>(),"observed_planning":{"discovery_seconds":discovery["seconds"],"metadata_and_plan_seconds":planning},"output_strategy":strategy(&runtime,&request.sink),"expected_outputs":outputs(p,strategy(&runtime,&request.sink)),"metadata_cache":p.nodes.iter().filter_map(|n|match n.operation{Operation::Inspect{cached}=>Some(json!({"asset_id":n.asset_id,"cached":cached})),_=>None}).collect::<Vec<_>>()});
    if let Runtime::Catalog(catalog) = &runtime {
        inspection["query"] = catalog.query();
    }
    if let Runtime::Direct { sources, .. } = &runtime {
        inspection["transport"] = sources.stats();
    }
    let native_value_bytes = p.selected.iter().try_fold(0u64, |total, selected| {
        let layout = selected.layout.as_ref()?;
        let width = u64::try_from(selected.window.width).ok()?;
        let height = u64::try_from(selected.window.height).ok()?;
        let times = if selected.dimensions.is_some() {
            selected.timestamps.len().max(1)
        } else {
            1
        };
        let bytes = match layout.dtype.as_str() {
            "Byte" | "UInt8" | "|u1" => 1,
            "UInt16" | "Int16" | "<u2" | "<i2" => 2,
            "Float32" | "UInt32" | "<f4" | "<u4" => 4,
            _ => return None,
        };
        total.checked_add(
            width
                .checked_mul(height)?
                .checked_mul(times as u64)?
                .checked_mul(bytes)?,
        )
    });
    inspection["logical_request"] = json!({"spatial":request.spatial,"time":request.time,"variables":request.variables,"grid":request.grid});
    inspection["output_budget"] = json!(request.budget);
    inspection["output_estimates"] = json!({"native_value_bytes": native_value_bytes, "encoded_file_bytes":null,"note":"estimate of native value payload only; coordinates, metadata, encoding, and filesystem overhead excluded; encoded output size unknown"});
    sanitize(&mut inspection);
    Ok(Prepared {
        runtime: std::sync::Arc::new(runtime),
        runtime_config,
        inspection: std::sync::Arc::new(inspection),
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
    if matches!(mode, OutputStrategy::ZarrObservations) {
        return crate::scientific_output::outputs(p);
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
        let mut record = json!({"asset_id":s.asset_id,"variable":l.variable,"timestamps":s.timestamps,"window":s.window,"native_grid":{"crs":l.crs,"transform":crate::spatial::window_transform(l,&s.window)},"dtype":match l.dtype.as_str() { "Byte" | "UInt8" | "|u1" => "uint8", "UInt16" | "<u2" => "uint16", "Int16" | "<i2" => "int16", "UInt32" | "<u4" => "uint32", "Float32" | "<f4" => "float32", _ => "unsupported" },"native_dtype":l.dtype,"nodata":l.nodata,"scale":l.scale,"offset":l.offset,"aoi_mask_applied":s.mask_geometry.is_some()});
        if zarr {
            record["file"] = json!(format!("{}.zarr", s.asset_id));
            record["validity"] = json!(if matches!(l.dtype.as_str(), "Float32" | "<f4") {
                "NaN is invalid; raw Float32 preserved"
            } else {
                "explicit integer fill"
            });
            record["dimensions"] = json!({"names":if matches!(l.dtype.as_str(), "Float32" | "<f4") {if s.dimensions.as_ref().is_some_and(|d|d.names.iter().any(|v|v=="lat")){vec!["time","lat","lon"]}else{vec!["time","latitude","longitude"]}}else{vec!["time","y","x"]},"shape":[s.timestamps.len(),s.window.height,s.window.width]});
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
pub(crate) fn sanitize(value: &mut Value) {
    match value {
        Value::String(s) if s.contains("://") => *s = sanitize_embedded_urls(s),
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
// Scientific metadata may embed a URL inside XML or a description. Sanitize
// each locator token, preserving the remaining structured/scientific content.
fn sanitize_embedded_urls(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = input[cursor..].find("://") {
        let separator = cursor + relative;
        let start = input[..separator]
            .rfind(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '+' | '-' | '.'))
            .map_or(0, |index| {
                index + input[index..].chars().next().unwrap().len_utf8()
            });
        if start < cursor || start == separator {
            output.push_str(&input[cursor..separator + 3]);
            cursor = separator + 3;
            continue;
        }
        let end = input[separator + 3..]
            .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '<' | '>' | '`'))
            .map_or(input.len(), |index| separator + 3 + index);
        output.push_str(&input[cursor..start]);
        output.push_str(&stac_workflow::safe_reference(&input[start..end]));
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    output
}
pub(crate) fn safe_error(
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
            if matches!(a.format, Format::Cog | Format::Hdf5) {
                v.add(root.into())?;
                continue;
            }
            for file in [".zgroup", ".zattrs", ".zarray", "zarr.json"] {
                v.add(root.join(file))?;
            }
            if a.format == Format::ZarrV3 {
                for axis in [
                    "time",
                    "x",
                    "y",
                    "lat",
                    "lon",
                    "latitude",
                    "longitude",
                    "spatial_ref",
                ] {
                    let dir = root.join(axis);
                    if dir.is_dir() {
                        v.axis(&dir, r.budget.max_nodes)?;
                    }
                }
                let group = crate::metadata_io::json(&root.join("zarr.json"))?;
                for var in &r.variables {
                    if let Variable::Dataset { path } = var {
                        let n = crate::zarr_acquire::name(path)?;
                        let path = root.join(n).join("zarr.json");
                        v.add(path.clone())?;
                        let metadata =
                            if let Some(m) = group["consolidated_metadata"]["metadata"].get(n) {
                                m.clone()
                            } else {
                                crate::metadata_io::json(&path)?
                            };
                        let attrs = &metadata["attributes"];
                        if let Some(mapping) = attrs["grid_mapping"].as_str() {
                            v.add(
                                root.join(crate::zarr_acquire::name(mapping)?)
                                    .join("zarr.json"),
                            )?;
                        }
                    }
                }
                continue;
            }
            let root_attrs = crate::metadata_io::json(&root.join(".zattrs"))?;
            let axes = if root_attrs["tile_ripper_grid"].is_object() {
                ["time", "y", "x"]
            } else if root.join("lat/.zarray").is_file() {
                ["time", "lat", "lon"]
            } else {
                ["time", "latitude", "longitude"]
            };
            for axis in axes {
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
    #[test]
    fn embedded_metadata_urls_preserve_following_scientific_fields() {
        let mut value = serde_json::json!({"xml":"<GDALMetadata><Item name=\"ACCESS\">https://user:password@example.org/a?sig=SECRET#help</Item><Item name=\"ZERO_DOPPLER_START_TIME\">2024-01-10T18:14:05.955189Z</Item></GDALMetadata>"});
        super::sanitize(&mut value);
        let xml = value["xml"].as_str().unwrap();
        assert!(xml.contains("https://example.org/a</Item>"));
        assert!(xml.contains("2024-01-10T18:14:05.955189Z</Item></GDALMetadata>"));
        assert!(!xml.contains("SECRET") && !xml.contains("password"));
        let text = super::sanitize_embedded_urls(
            "ΔH 🌊https://user:password@example.org/a?sig=SECRET then a field",
        );
        assert_eq!(text, "ΔH 🌊https://example.org/a then a field");
    }
    #[cfg(all(unix, feature = "native-cog", feature = "authenticated-http"))]
    #[test]
    fn authenticated_cog_sessions_require_an_exact_complete_map_and_budget() {
        use crate::{Asset, Budget, Format, Grid, Request, Sink, Source, Spatial, Variable};
        use std::{collections::BTreeMap, io::Write, os::unix::fs::OpenOptionsExt, sync::Arc};
        let netrc = std::env::temp_dir().join(format!(
            "tile-ripper-cog-map-{}-{}.netrc",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&netrc)
            .unwrap();
        file.write_all(b"machine urs.earthdata.nasa.gov login fixture password fixture\n")
            .unwrap();
        let url = "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/HLSL30.020/HLS.L30.T10SEG.2026001T000000.v2.0/HLS.L30.T10SEG.2026001T000000.v2.0.B04.tif";
        let request = Request {
            source: Source::Assets {
                assets: vec![Asset {
                    id: "red".into(),
                    locator: url.into(),
                    format: Format::Cog,
                    datetime: None,
                    etag: None,
                }],
            },
            spatial: Spatial::Bounds {
                crs: "EPSG:4326".into(),
                bbox: [-1., -1., 1., 1.],
            },
            time: None,
            variables: vec![Variable::Band { index: 1 }],
            quality: vec![],
            grid: Grid::Native,
            operations: vec![],
            sink: Sink::Callback,
            budget: Budget {
                buffer_bytes: 4 << 20,
                max_nodes: 16,
                chunk_edge: 16,
            },
        };
        let session = || {
            Arc::new(
                crate::authenticated_http::AuthenticatedHttp::earthdata_hls(url, &netrc).unwrap(),
            )
        };
        let exact = BTreeMap::from([("red".into(), session())]);
        assert_eq!(
            super::validate_authenticated_cog_sessions(&request, &exact).unwrap(),
            crate::authenticated_http::WORKING_RESERVATION
        );
        assert!(super::validate_authenticated_cog_sessions(&request, &BTreeMap::new()).is_err());
        let extra = BTreeMap::from([("red".into(), session()), ("extra".into(), session())]);
        assert!(super::validate_authenticated_cog_sessions(&request, &extra).is_err());
        let mut constrained = request.clone();
        constrained.budget.buffer_bytes = crate::authenticated_http::WORKING_RESERVATION;
        assert!(super::validate_authenticated_cog_sessions(&constrained, &exact).is_err());
        std::fs::remove_file(netrc).unwrap();
    }
}
