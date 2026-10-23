//! Bounded, serial acquisition of explicit SWOT Raster D PGD0 observations.
//!
//! Requests are inputs, never saved executable plans. Each source gets a fresh,
//! exact-object caller-owned session. No collection discovery or grid alignment.
#[cfg(feature = "authenticated-http")]
use crate::acquisition;
use crate::{
    cog::LayoutCache, fail, scientific_output::swot as output, swot, Asset, Format, Grid, Request,
    Result, Sink, Source, Spatial, Variable,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fmt,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_INSPECTION_BYTES: usize = 256 * 1024;
const MAX_RECEIPT_BYTES: usize = 512 * 1024;
const MAX_RESULT_BYTES: usize = 3 * 1024 * 1024;
/// Covers at most four bounded inspections/receipts, JSON trees/serialization,
/// output metadata, coverage rectangles and job/checkpoint-root bookkeeping.
pub const BOOKKEEPING_RESERVATION: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    SwotRasterDPgd0,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub job_schema: u32,
    pub profile: Profile,
    pub observations: Vec<Observation>,
    pub limits: Limits,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub id: String,
    pub request: Request,
    pub quality_policy: swot::Policy,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub memory_bytes: usize,
    pub output_chunk_edge: usize,
    pub max_output_objects: usize,
}

/// Count serialized bytes without first allocating a potentially oversized copy.
pub(crate) fn bounded(value: &impl Serialize, cap: usize, component: &str) -> Result<usize> {
    struct Counter {
        bytes: usize,
        cap: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.cap)
                .ok_or_else(|| std::io::Error::other("serialized size bound"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, cap };
    serde_json::to_writer(&mut counter, value)
        .map_err(|_| format!("{component} exceeds its bounded metadata allowance ({cap} bytes)"))?;
    Ok(counter.bytes)
}
fn identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
impl Observation {
    pub fn asset(&self) -> Result<&Asset> {
        match &self.request.source {
            Source::Assets { assets } if assets.len() == 1 && assets[0].format == Format::Hdf5 => {
                Ok(&assets[0])
            }
            _ => fail("job observations require one explicit native HDF5 source"),
        }
    }
    fn normalized_request(&self) -> Request {
        let mut r = self.request.clone();
        r.variables = swot::PATHS
            .iter()
            .map(|p| Variable::Dataset { path: (*p).into() })
            .collect();
        r
    }
}
impl Manifest {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return fail("job manifest exceeds 64 KiB input bound");
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|_| "invalid SWOT job manifest; see docs/swot-job/workflow.md")?;
        manifest.validate()?;
        Ok(manifest)
    }
    pub fn validate(&self) -> Result<()> {
        bounded(self, MAX_MANIFEST_BYTES, "job manifest")?;
        if self.job_schema != 1 || !(1..=4).contains(&self.observations.len()) {
            return fail("job_schema 1 requires one to four observations");
        }
        if !(1..=1024).contains(&self.limits.output_chunk_edge)
            || self.limits.max_output_objects == 0
        {
            return fail("output_chunk_edge must be 1..1024 and max_output_objects positive");
        }
        let writer = output::reservation(self.limits.output_chunk_edge)?;
        let mut ids = HashSet::new();
        let mut sources = HashSet::new();
        let mut objects = 5usize;
        for entry in &self.observations {
            if !identifier(&entry.id) || !ids.insert(&entry.id) {
                return fail("observation IDs must be unique 1..64 character ASCII letters/digits/underscore/hyphen");
            }
            let asset = entry.asset()?;
            if !identifier(&asset.id) {
                return fail("source asset ID must be a bounded ASCII identifier");
            }
            let canonical = if crate::cog::remote(&asset.locator) {
                let u = reqwest::Url::parse(&asset.locator)
                    .map_err(|_| "invalid canonical source URL")?;
                if !u.username().is_empty()
                    || u.password().is_some()
                    || u.query().is_some()
                    || u.fragment().is_some()
                    || u.as_str() != asset.locator
                    || u.path().ends_with('/')
                    || !(u.scheme() == "https"
                        || u.scheme() == "http"
                            && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
                {
                    return fail("job sources require canonical unsigned HTTPS object URLs (HTTP only for loopback fixtures); no credentials, queries or fragments");
                }
                u.to_string()
            } else {
                std::fs::canonicalize(&asset.locator)
                    .map_err(|_| "cannot resolve local job source")?
                    .to_string_lossy()
                    .into_owned()
            };
            if !sources.insert(canonical) {
                return fail("repeated canonical source in job; combine its selection explicitly or use a separate job; observations are never silently merged");
            }
            if asset.etag.as_ref().is_some_and(|v| {
                v.len() > 512
                    || !v.starts_with('"')
                    || !v.ends_with('"')
                    || v.contains(['\r', '\n'])
            }) {
                return fail("expected source version requires a bounded strong quoted ETag");
            }
            if asset
                .datetime
                .as_ref()
                .is_some_and(|v| v.len() > 64 || chrono::DateTime::parse_from_rfc3339(v).is_err())
            {
                return fail("optional source datetime must be RFC3339");
            }
            let r = &entry.request;
            if !matches!(r.grid, Grid::Native)
                || !matches!(r.sink, Sink::Callback)
                || r.time.is_some()
                || !r.quality.is_empty()
                || !r.operations.is_empty()
                || r.variables.len() != 5
                || swot::PATHS.iter().any(|p| {
                    r.variables
                        .iter()
                        .filter(|v| **v == Variable::Dataset { path: (*p).into() })
                        .count()
                        != 1
                })
            {
                return fail("job requests require native grid, callback sink, exactly the five SWOT paths, and explicit index selection without time filtering or transforms");
            }
            let Spatial::ArraySlice { count, axes, .. } = &r.spatial else {
                return fail(
                    "job requires explicit native array_slice; geographic selection is unsupported",
                );
            };
            if axes != &["/y".to_string(), "/x".to_string()] {
                return fail(
                    "qualified SWOT job requires explicit /y and /x axes in row/column order",
                );
            }
            if count.contains(&0) {
                return fail("empty index selections are unsupported; counts must be positive (all-invalid nonempty selections remain valid)");
            }
            crate::planner::validate(r)?;
            if r.budget.max_nodes > 4096 {
                return fail("SWOT job source max_nodes is bounded at 4096");
            }
            let required = BOOKKEEPING_RESERVATION
                .checked_add(writer)
                .and_then(|n| n.checked_add(r.budget.buffer_bytes))
                .ok_or("aggregate memory reservation overflow")?;
            if required > self.limits.memory_bytes {
                return Err(format!("job aggregate memory admission failed for {}: source request cap {} + writer {} + job metadata/receipts {} = {} bytes > aggregate memory_bytes {}; working reservations, not RSS",entry.id,r.budget.buffer_bytes,writer,BOOKKEEPING_RESERVATION,required,self.limits.memory_bytes).into());
            }
            // Count unique v2 keys conservatively; actual source tile boundaries
            // may rewrite a destination chunk, but never create other keys.
            let edge = self.limits.output_chunk_edge;
            let tiles = count[0]
                .div_ceil(edge)
                .checked_mul(count[1].div_ceil(edge))
                .ok_or("output object count overflow")?;
            objects = objects
                .checked_add(34)
                .and_then(|n| n.checked_add(count[0].div_ceil(edge)))
                .and_then(|n| n.checked_add(count[1].div_ceil(edge)))
                .and_then(|n| n.checked_add(tiles.checked_mul(11)?))
                .ok_or("output object count overflow")?;
            if objects > self.limits.max_output_objects {
                return fail("scientific output keys exceed aggregate max_output_objects; increase chunk edge or explicit object limit");
            }
            // Validate every policy before the first source is acquired.
            entry.quality_policy.masks()?;
        }
        Ok(())
    }
    fn admission(&self) -> Value {
        let source = self
            .observations
            .iter()
            .map(|e| e.request.budget.buffer_bytes)
            .max()
            .unwrap_or(0);
        let writer = output::reservation(self.limits.output_chunk_edge).expect("validated edge");
        json!({"execution":"serial synchronous delivery; no next decoder while writer retains a tile","active_sources":1,"active_authentication_sessions":1,
            "queued_tiles":0,"consumer_retained_tiles":0,"job_metadata_receipts_bytes":BOOKKEEPING_RESERVATION,"writer_bytes":writer,
            "max_source_request_bytes":source,"aggregate_admitted_cap_bytes":source+writer+BOOKKEEPING_RESERVATION,"aggregate_limit_bytes":self.limits.memory_bytes,
            "source_scope":"source budget includes decoder, encoded-range retention, prepared metadata/coordinates, authentication and checkpoint index/write scratch",
            "scope":"working reservations; excludes allocator, TLS/HTTP runtime, OS/filesystem cache and caller-owned inputs; process RSS measured separately"})
    }
}

#[cfg(feature = "authenticated-http")]
pub type SessionFactory<'a> =
    dyn FnMut(&Asset) -> Result<Arc<crate::authenticated_http::AuthenticatedHttp>> + 'a;

/// Consumed by plan or fetch. A factory must construct a fresh exact-object
/// session on every call, including the final source-validation sweep.
pub struct Job<'a> {
    manifest: &'a Manifest,
    #[cfg(feature = "authenticated-http")]
    sessions: Option<&'a mut SessionFactory<'a>>,
    #[cfg(feature = "authenticated-http")]
    checkpoint: Option<crate::checkpoint::Job>,
}
impl<'a> Job<'a> {
    pub fn new(manifest: &'a Manifest) -> Result<Self> {
        manifest.validate()?;
        Ok(Self {
            manifest,
            #[cfg(feature = "authenticated-http")]
            sessions: None,
            #[cfg(feature = "authenticated-http")]
            checkpoint: None,
        })
    }
    #[cfg(feature = "authenticated-http")]
    pub fn authenticated(
        manifest: &'a Manifest,
        sessions: &'a mut SessionFactory<'a>,
        checkpoint: Option<crate::checkpoint::Options>,
    ) -> Result<Self> {
        let mut job = Self::new(manifest)?;
        job.sessions = Some(sessions);
        if let Some(options) = checkpoint {
            if manifest
                .observations
                .iter()
                .any(|e| e.asset().is_ok_and(|a| !crate::cog::remote(&a.locator)))
            {
                return fail(
                    "checkpoint jobs require authenticated remote sources for every observation",
                );
            }
            let identities = manifest.observations.iter().map(|e| Ok(json!({"id":e.id,"scientific":crate::checkpoint::scientific_identity(&e.normalized_request(),e.quality_policy.provenance())?}))).collect::<Result<Vec<_>>>()?;
            job.checkpoint = Some(crate::checkpoint::Job::open(options, &identities)?);
        }
        Ok(job)
    }
    fn checkpoint_description(&self) -> Value {
        #[cfg(feature = "authenticated-http")]
        if let Some(checkpoint) = &self.checkpoint {
            return checkpoint.describe();
        }
        json!({"enabled":false})
    }
    fn prepare(&mut self, index: usize) -> Result<swot::Prepared> {
        let entry = &self.manifest.observations[index];
        let request = entry.normalized_request();
        let policy = entry.quality_policy.clone();
        let mut cache = LayoutCache::default();
        #[cfg(feature = "authenticated-http")]
        if let Some(factory) = &mut self.sessions {
            let session = factory(entry.asset()?)?;
            session.require_fresh()?;
            if let Some(checkpoint) = &self.checkpoint {
                return swot::prepare_using(&request, policy, |r| {
                    let store = checkpoint.store(
                        index,
                        crate::checkpoint::scientific_identity(
                            r,
                            entry.quality_policy.provenance(),
                        )?,
                        session.checkpoint_policy()?,
                    )?;
                    acquisition::prepare_bound_checkpoint(r, &mut cache, session, store)
                });
            }
            return swot::prepare_authenticated(&request, policy, &mut cache, session);
        }
        swot::prepare(&request, policy, &mut cache)
    }
    fn inspect(prepared: &swot::Prepared) -> Result<Value> {
        let inspection = prepared.explain();
        bounded(&inspection, MAX_INSPECTION_BYTES, "per-source inspection")?;
        let datasets = inspection["sources"][0]["object_identities"]["datasets"]
            .as_array()
            .ok_or("missing native source schema")?;
        if datasets
            .iter()
            .any(|d| d["root_attributes"]["crid"]["value"] != "PGD0")
        {
            return fail("job profile requires qualified Version D PGD0 metadata; PID0 and other releases are unsupported");
        }
        let mapping = &datasets
            .iter()
            .find(|d| d["path"] == "/wse")
            .ok_or("missing WSE schema")?["grid_mapping"];
        let attributes = &mapping["attributes"];
        if mapping["path"] != "/crs"
            || attributes["grid_mapping_name"]["value"].as_str().is_none()
            || !["spatial_ref", "crs_wkt"].iter().any(|key| {
                attributes[key]["value"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            })
        {
            return fail("SWOT job profile requires explicit source /crs mapping attributes and WKT; no CRS is inferred");
        }
        swot::Policy::cookbook().validate_definitions(&inspection)?;
        swot::Policy::Bitwise {
            require: vec![],
            exclude: vec![],
        }
        .validate_definitions(&inspection)?;
        Ok(inspection)
    }
    pub fn plan(mut self) -> Result<Value> {
        let mut observations = Vec::new();
        for index in 0..self.manifest.observations.len() {
            let prepared = self.prepare(index).map_err(|e| {
                format!(
                    "observation {} preparation failed: {e}",
                    self.manifest.observations[index].id
                )
            })?;
            let inspection = Self::inspect(&prepared)?;
            let entry = &self.manifest.observations[index];
            observations.push(json!({"id":entry.id,"output_group":format!("observations/{}",entry.id),"inspection":inspection}));
        }
        Ok(
            json!({"document_type":"swot_job_inspection","job_schema":1,"profile":self.manifest.profile,"saved_plan_execution_supported":false,
            "ordering":"manifest order; repeated IDs and canonical sources rejected; equal timestamps do not imply equal observations",
            "observations":observations,"admission":self.manifest.admission(),"checkpoint":self.checkpoint_description(),
            "output":{"format":"zarr_v2","encoding":"uncompressed raw stored values","chunk_edge":self.manifest.limits.output_chunk_edge,"max_objects":self.manifest.limits.max_output_objects,"aoi_mask":false},
            "completion":"all required observations must finish, then every source is revalidated in a serial sweep before manifest/synchronization/no-replace publication",
            "known_costs":["full bounded legacy HDF5 index traversal remains default","each source uses its own authentication session; final sweep uses fresh sessions and online eight-byte GET proof","checkpoint replay reconstructs all values and rewrites all output","synchronization and no-replace publication are part of fetch"],
            "unsupported":["catalog/date/geographic discovery","empty index selections","reprojection/resampling/mosaicking","aligned time series","additional processing releases or products","partial output or completed-observation recovery"],
            "source_snapshot":"per-object versions; not an atomic snapshot of a changing collection"}),
        )
    }
    fn revalidate(
        &mut self,
        index: usize,
        identity: &Value,
        cancelled: &AtomicBool,
    ) -> Result<Value> {
        check_cancelled(cancelled)?;
        let entry = &self.manifest.observations[index];
        #[allow(unused_mut)]
        let mut transport = crate::http_transport::Transport::Anonymous;
        #[cfg(feature = "authenticated-http")]
        if let Some(factory) = &mut self.sessions {
            let session = factory(entry.asset()?)?;
            session.require_fresh()?;
            if !session.accepts(&entry.asset()?.locator) {
                return fail("final validation session does not match its canonical source");
            }
            transport = crate::http_transport::Transport::Authenticated(session);
        }
        let authenticated = transport.authenticated();
        let source =
            crate::storage::ObjectFile::open(entry.asset()?, &entry.request.budget, transport)?;
        source.phase("job_final_validation");
        if source.identity() != *identity {
            return fail("source identity changed before job publication");
        }
        if authenticated {
            source.read_cancellable(0, source.size.min(8) as usize, Some(cancelled))?;
        }
        source.validate_cancellable(Some(cancelled))?;
        if source.identity() != *identity {
            return fail("source identity changed during final job validation");
        }
        check_cancelled(cancelled)?;
        Ok(
            json!({"status":"validated","identity":identity,"transport":source.stats(),"scope":"fresh session, exact identity comparison, online authorization GET for authenticated source and conditional final HEAD/local stat; no atomic collection snapshot"}),
        )
    }
    pub fn fetch(mut self, path: impl AsRef<Path>, cancelled: Arc<AtomicBool>) -> Result<Value> {
        let started = Instant::now();
        let mut receipts = Vec::new();
        let mut current = None;
        let result = (|| -> Result<Value> {
            check_cancelled(&cancelled)?;
            let mut writer = output::Writer::new(
                path.as_ref(),
                self.manifest.limits.output_chunk_edge,
                &cancelled,
            )?;
            let mut peak = 0u64;
            let mut first_observation_seconds = None;
            for index in 0..self.manifest.observations.len() {
                current = Some(self.manifest.observations[index].id.clone());
                check_cancelled(&cancelled)?;
                let prepared = self.prepare(index)?;
                let inspection = Self::inspect(&prepared)?;
                let mut receipt = writer.observation(
                    &self.manifest.observations[index],
                    &prepared,
                    &inspection,
                    &cancelled,
                )?;
                let completed_seconds = started.elapsed().as_secs_f64();
                first_observation_seconds.get_or_insert(completed_seconds);
                receipt["job_elapsed_seconds"] = json!(completed_seconds);
                bounded(&receipt, MAX_RECEIPT_BYTES, "per-source completion receipt")?;
                let source_peak = receipt["observed"]["scheduler"]["reserved_bytes_peak"]
                    .as_u64()
                    .ok_or("missing source admission diagnostics")?;
                peak = peak.max(
                    source_peak
                        + BOOKKEEPING_RESERVATION as u64
                        + output::reservation(self.manifest.limits.output_chunk_edge)? as u64,
                );
                receipts.push(receipt);
                // Prepared metadata, coordinate arrays, auth session and source
                // checkpoint index are dropped before starting the next source.
            }
            let acquisition_and_write_seconds = started.elapsed().as_secs_f64();
            let final_sweep = Instant::now();
            for (index, receipt) in receipts.iter_mut().enumerate() {
                current = Some(self.manifest.observations[index].id.clone());
                let identity = receipt["source"]["object_identities"]["identity"].clone();
                let validation_start = started.elapsed().as_secs_f64();
                receipt["job_final_validation"] = self.revalidate(index, &identity, &cancelled)?;
                receipt["job_final_validation"]["started_seconds"] = json!(validation_start);
                receipt["job_final_validation"]["completed_seconds"] =
                    json!(started.elapsed().as_secs_f64());
                receipt["status"] = json!("complete");
            }
            current = None;
            let mut manifest = json!({"document_type":"swot_job_result","job_schema":1,"status":"complete","profile":self.manifest.profile,
                "format":"zarr_v2","writer":"zarrs 0.23.14","observation_ids":self.manifest.observations.iter().map(|e|&e.id).collect::<Vec<_>>(),"observations":receipts,
                "admission":self.manifest.admission(),"checkpoint":self.checkpoint_description(),
                "observed":{"acquisition_and_write_seconds":acquisition_and_write_seconds,"final_source_sweep_seconds":final_sweep.elapsed().as_secs_f64(),"first_observation_seconds":first_observation_seconds,
                    "working_reservation_peak_bytes":peak,"output_io":writer.statistics()},
                "completion":{"all_required_observations":true,"final_source_validation":"serial fresh-session sweep after all observations are written and individually validated, before final root metadata/manifest and publication synchronization",
                    "source_snapshot":"individually validated versions; not an atomic snapshot of a changing collection","publication":"existing checked atomic no-replace sibling rename on macOS/Linux",
                    "restart":"new staging and complete scientific reconstruction; reauthenticate every source; reuse verified source bytes only; no append, partial-output recovery or exactly-once processing",
                    "synchronization":"file handles synchronized before close, bottom-up directories, pre-rename device request, no-replace rename, parent synchronization and post-rename device request; process-interruption safety tested; power-loss durability unproven"},
                "processing":["five raw typed variables; exact selected x/y coordinates; source time and attributes","five separate source masks and a derived quality_mask; no AOI mask for index selection","independent native grids in manifest order; no calibration, alignment, resampling or mosaic"]});
            bounded(&manifest, MAX_RESULT_BYTES, "job result")?;
            let publication = Instant::now();
            writer.publish(&manifest, &cancelled)?;
            // Publication timing cannot be placed in its own pre-commit manifest.
            manifest["post_publication_receipt"] = json!({"publication_seconds":publication.elapsed().as_secs_f64(),"total_seconds":started.elapsed().as_secs_f64(),"scope":"returned CLI/API receipt; dataset manifest records all pre-publication work"});
            Ok(manifest)
        })();
        result.map_err(|e| {
            let message = format!("SWOT job failed{}: {e}",current.as_ref().map_or_else(String::new,|id|format!(" at observation {id}")));
            Box::new(Failure { receipt:json!({"document_type":"swot_job_result","job_schema":1,"status":"failed","failed_observation":current,
                "error":message,"provisional_source_receipts":receipts,"checkpoint":self.checkpoint_description(),
                "publication":"no successful job completion; inspect any explicit post-rename synchronization error","restart":"rebuild all scientific output in new staging; retained source checkpoints may be reused"}), message }) as Box<dyn std::error::Error + Send + Sync>
        })
    }
}
fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        return fail("acquisition cancelled; job output was not published");
    }
    Ok(())
}
/// CLI emits this bounded failure receipt to stdout with a nonzero exit status.
/// Earlier source deliveries are provisional and never qualify a partial job.
#[derive(Debug)]
pub struct Failure {
    pub receipt: Value,
    message: String,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
