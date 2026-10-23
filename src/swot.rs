//! Explicit SWOT Raster WSE policies over aligned raw native layers.
//!
//! Qualification is for the inspected Version D PGD0 object. Matching a schema
//! is not qualification of other processing releases. No vector-product flags,
//! river processing, time conversion or implicit calibration is applied.
use crate::{
    acquisition, cog::LayoutCache, fail, Chunk, ExecutionReport, Format, Request, Result, Source,
    Variable,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

pub const PATHS: [&str; 5] = [
    "/wse",
    "/wse_qual",
    "/wse_qual_bitwise",
    "/wse_uncert",
    "/n_wse_pix",
];
pub const COOKBOOK: &str =
    "https://podaac.github.io/tutorials/notebooks/datasets/SWOT_Raster_Notebook_local.html";
/// The inspected PGD0 file's definitions, including its specular-ringing bits.
pub const FLAGS: [(u32, &str); 19] = [
    (2, "classification_qual_suspect"),
    (4, "geolocation_qual_suspect"),
    (32, "large_uncert_suspect"),
    (128, "bright_land"),
    (512, "specular_ringing_prior_water_suspect"),
    (4096, "few_pixels"),
    (8192, "far_range_suspect"),
    (16384, "near_range_suspect"),
    (262144, "classification_qual_degraded"),
    (524288, "geolocation_qual_degraded"),
    (1048576, "dark_water_degraded"),
    (2097152, "low_coherence_water_degraded"),
    (4194304, "specular_ringing_prior_land_degraded"),
    (16777216, "value_bad"),
    (67108864, "outside_data_window"),
    (268435456, "no_pixels"),
    (536870912, "outside_scene_bounds"),
    (1073741824, "inner_swath"),
    (2147483648, "missing_karin_data"),
];
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryQuality {
    Good,
    Suspect,
    Degraded,
    Bad,
}
impl SummaryQuality {
    fn code(self) -> u8 {
        match self {
            Self::Good => 0,
            Self::Suspect => 1,
            Self::Degraded => 2,
            Self::Bad => 3,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Policy {
    /// Only source WSE missingness; this makes no scientific-quality claim.
    Raw,
    Summary {
        accepted: Vec<SummaryQuality>,
    },
    /// Every required condition must be set; every excluded condition unset.
    /// Unassigned source bits and fill words are excluded, not reinterpreted.
    Bitwise {
        require: Vec<String>,
        exclude: Vec<String>,
    },
}
impl Policy {
    pub fn cookbook() -> Self {
        Self::Summary {
            accepted: vec![SummaryQuality::Good, SummaryQuality::Suspect],
        }
    }
    pub(crate) fn masks(&self) -> Result<(u32, u32)> {
        fn names(values: &[String]) -> Result<u32> {
            let mut mask = 0;
            for name in values {
                let bit = FLAGS
                    .iter()
                    .find(|(_, n)| n == name)
                    .map(|(b, _)| *b)
                    .ok_or("unknown or uninspected SWOT WSE flag condition")?;
                if mask & bit != 0 {
                    return fail("duplicate SWOT WSE flag condition");
                }
                mask |= bit;
            }
            Ok(mask)
        }
        match self {
            Self::Bitwise { require, exclude } => {
                let (a, b) = (names(require)?, names(exclude)?);
                if a & b != 0 {
                    return fail("SWOT condition cannot be both required and excluded");
                }
                Ok((a, b))
            }
            Self::Summary { accepted } => {
                if accepted.is_empty()
                    || accepted
                        .iter()
                        .enumerate()
                        .any(|(i, v)| accepted[..i].contains(v))
                {
                    return fail("summary policy needs distinct documented categories");
                }
                Ok((0, 0))
            }
            Self::Raw => Ok((0, 0)),
        }
    }
    pub(crate) fn validate_definitions(&self, inspection: &Value) -> Result<()> {
        self.masks()?;
        let ds = inspection["sources"][0]["object_identities"]["datasets"]
            .as_array()
            .ok_or("missing source definitions")?;
        let find = |path: &str| {
            ds.iter()
                .find(|v| v["path"] == path)
                .ok_or("missing SWOT variable definition")
        };
        let wse = find(PATHS[0])?;
        if !matches!(
            wse["root_attributes"]["crid"]["value"].as_str(),
            Some("PGD0" | "PID0")
        ) {
            return fail("SWOT aligned workflow requires Version D PGD0/PID0 source metadata");
        }
        for (path, dtype) in PATHS.iter().zip(["<f4", "|u1", "<u4", "<f4", "<u4"]) {
            let d = find(path)?;
            if d["shape"] != wse["shape"]
                || d["stored_dtype"] != dtype
                || d["grid_mapping"] != wse["grid_mapping"]
                || d["axes"] != wse["axes"]
            {
                return fail("SWOT layers must share dimensions, explicit axes and source CRS; stored types must match the qualified profile");
            }
        }
        if !matches!(self, Self::Raw) {
            let q = &find(PATHS[1])?["attributes"];
            if q["flag_values"]["value"] != json!([0, 1, 2, 3])
                || q["flag_meanings"]["value"] != "good suspect degraded bad"
            {
                return fail("ambiguous or uninspected SWOT summary quality definitions");
            }
        }
        if matches!(self, Self::Bitwise { .. }) {
            let q = &find(PATHS[2])?["attributes"];
            let names = FLAGS.iter().map(|(_, n)| *n).collect::<Vec<_>>().join(" ");
            if q["flag_masks"]["value"] != json!(FLAGS.iter().map(|(b, _)| *b).collect::<Vec<_>>())
                || q["flag_meanings"]["value"] != names
            {
                return fail("ambiguous or uninspected SWOT bitwise quality definitions");
            }
        }
        Ok(())
    }
    /// Compute a separate mask; raw layers and each source mask are preserved.
    pub fn mask(&self, layers: &[Chunk; 5]) -> Result<Vec<u8>> {
        let (required, excluded) = self.masks()?;
        let wse = layers[0].pixels.as_f32().ok_or("WSE requires Float32")?;
        let summary = layers[1].pixels.as_u8().ok_or("summary requires UInt8")?;
        let flags = layers[2].pixels.as_u32().ok_or("flags require UInt32")?;
        if layers.iter().zip(PATHS).any(|(c, path)| {
            c.variable != (Variable::Dataset { path: path.into() })
                || c.window != layers[0].window
                || c.transform != layers[0].transform
                || c.coordinates != layers[0].coordinates
                || c.crs != layers[0].crs
                || c.asset_id != layers[0].asset_id
                || c.pixels.len() != wse.len()
        }) {
            return fail("SWOT policy requires exactly aligned raw layers from one source");
        }
        let known = FLAGS.iter().fold(0, |m, (b, _)| m | b);
        Ok((0..wse.len())
            .map(|i| {
                let source = layers[0].pixels.valid(i, layers[0].nodata);
                u8::from(
                    source
                        && match self {
                            Self::Raw => true,
                            Self::Summary { accepted } => {
                                wse[i].is_finite()
                                    && layers[1].pixels.valid(i, layers[1].nodata)
                                    && accepted.iter().any(|q| q.code() == summary[i])
                            }
                            Self::Bitwise { .. } => {
                                wse[i].is_finite()
                                    && layers[2].pixels.valid(i, layers[2].nodata)
                                    && flags[i] & !known == 0
                                    && flags[i] & required == required
                                    && flags[i] & excluded == 0
                            }
                        },
                )
            })
            .collect())
    }
    pub fn provenance(&self) -> Value {
        json!({"policy":self,"cookbook_example":COOKBOOK,"source_flag_definitions":FLAGS,
            "semantics":"Raw uses source WSE nonmissingness only. Summary additionally requires finite WSE and an explicitly accepted source category. Bitwise additionally requires finite WSE, a nonmissing flag word, every required bit, no excluded bit and no unassigned source bits. Uncertainty/count missingness is independent. No magnitude, uncertainty, count or geographic threshold is inferred.",
            "unknown_summary_codes":"exclude","unknown_bit_conditions":"fail preparation","unassigned_observed_bits":"exclude for bitwise policy","calibration":"stored values; no automatic scale/offset","qualification":"inspected Version D PGD0 only; other releases require separate qualification"})
    }
}
/// Five raw layers in PATHS order over the same logical window; validity is
/// derived independently. All values remain provisional until stream.finish().
pub struct Tile {
    pub layers: [Chunk; 5],
    pub validity: Vec<u8>,
}
/// Immutable, version-bound native grid/profile inspection. Clones share only
/// retained source metadata and full axes; prepared requests own separate plans.
/// This runtime binding is deliberately not serializable.
#[derive(Clone)]
pub struct GridInspection {
    native: crate::hdf5::GridInspection,
}
impl GridInspection {
    pub fn inspect(request: &Request) -> Result<Self> {
        Ok(Self {
            native: crate::hdf5::GridInspection::inspect(request, Default::default())
                .map_err(|e| acquisition::safe_error(request, "grid inspection", e))?,
        })
    }
    #[cfg(feature = "authenticated-http")]
    pub fn inspect_authenticated(
        request: &Request,
        session: Arc<crate::authenticated_http::AuthenticatedHttp>,
    ) -> Result<Self> {
        if !matches!(&request.source,Source::Assets{assets} if assets.len()==1 && session.accepts(&assets[0].locator))
        {
            return fail("grid inspection requires its exact configured native HDF5 object");
        }
        session.require_fresh()?;
        Ok(Self {
            native: crate::hdf5::GridInspection::inspect(
                request,
                crate::http_transport::Transport::Authenticated(session),
            )
            .map_err(|e| acquisition::safe_error(request, "grid inspection", e))?,
        })
    }
    pub fn describe(&self) -> Value {
        let mut description = self.native.describe();
        acquisition::sanitize(&mut description);
        description
    }
    pub fn with_grid<T>(&self, f: impl FnOnce(&Value, &[f64], &[f64]) -> Result<T>) -> Result<T> {
        self.native.with_grid(f)
    }
    pub fn prepare(&self, request: &Request, policy: Policy) -> Result<Prepared> {
        prepare_using(request, policy, |r| {
            acquisition::prepare_inspected(r, &self.native)
        })
    }
}
#[derive(Clone)]
pub struct Prepared {
    base: acquisition::Prepared,
    policy: Policy,
    working_memory: Value,
    admission_seconds: f64,
}
/// Requires one HDF5 asset and exactly the five documented variables. Input
/// variable order is normalized to PATHS; source windows/strides stay unchanged.
pub fn prepare(request: &Request, policy: Policy, cache: &mut LayoutCache) -> Result<Prepared> {
    prepare_using(request, policy, |r| acquisition::prepare(r, cache))
}
/// Explicit authenticated counterpart; credentials and session never enter Request.
#[cfg(feature = "authenticated-http")]
pub fn prepare_authenticated(
    request: &Request,
    policy: Policy,
    cache: &mut LayoutCache,
    transport: Arc<crate::authenticated_http::AuthenticatedHttp>,
) -> Result<Prepared> {
    prepare_using(request, policy, |r| {
        acquisition::prepare_authenticated(r, cache, transport)
    })
}
/// Exact-range transfer resume. Quality policy and interpreted source flag
/// definitions participate in identity; delivered tiles can repeat after restart.
#[cfg(feature = "authenticated-http")]
pub fn prepare_authenticated_checkpointed(
    request: &Request,
    policy: Policy,
    cache: &mut LayoutCache,
    session: Arc<crate::authenticated_http::AuthenticatedHttp>,
    options: crate::checkpoint::Options,
) -> Result<Prepared> {
    policy.masks()?;
    let quality = policy.provenance();
    prepare_using(request, policy, |r| {
        acquisition::prepare_checkpointed(r, cache, session, options, quality)
    })
}
pub(crate) fn prepare_using(
    request: &Request,
    policy: Policy,
    prepare: impl FnOnce(&Request) -> Result<acquisition::Prepared>,
) -> Result<Prepared> {
    if !matches!(&request.source,Source::Assets{assets} if assets.len()==1 && assets[0].format==Format::Hdf5)
        || request.variables.len() != 5
        || PATHS.iter().any(|p| {
            request
                .variables
                .iter()
                .filter(|v| **v == (Variable::Dataset { path: (*p).into() }))
                .count()
                != 1
        })
    {
        return fail("SWOT aligned workflow requires one HDF5 source and exactly PATHS variables");
    }
    policy.masks()?;
    let mut r = request.clone();
    r.variables = PATHS
        .iter()
        .map(|p| Variable::Dataset { path: (*p).into() })
        .collect();
    let base = prepare(&r)?;
    let started = std::time::Instant::now();
    policy.validate_definitions(&base.explain())?;
    let working_memory = base.swot_working_memory()?;
    Ok(Prepared {
        base,
        policy,
        working_memory,
        admission_seconds: started.elapsed().as_secs_f64(),
    })
}
impl Prepared {
    pub(crate) fn with_coordinates<T>(
        &self,
        f: impl FnOnce(&[f64], &[f64]) -> Result<T>,
    ) -> Result<T> {
        self.base.with_swot_coordinates(f)
    }
    /// Synchronous delivery keeps the job writer and the next decoder mutually
    /// exclusive. Unlike a stream consumer, this path retains no preceding tile.
    pub(crate) fn execute_tiles(
        &self,
        cancelled: &AtomicBool,
        mut sink: impl FnMut(Tile) -> Result<()>,
    ) -> Result<ExecutionReport> {
        self.base.execute_swot_cancellable(cancelled, |layers| {
            let validity = self.policy.mask(&layers)?;
            sink(Tile { layers, validity })
        })
    }
    pub fn explain(&self) -> Value {
        let mut value = self.base.explain();
        value["quality_policy"] = self.policy.provenance();
        value["aligned_execution"] = json!({"ordering":"WSE source-chunk row/column order, then bounded logical subtiles; PATHS order within each tile","queue_capacity":0,"range_cache":"bounded, execution-local, one pinned source identity","output":"five raw arrays, independent source masks and separate derived validity; no file publication"});
        value["aligned_execution"]["working_memory"] = self.working_memory.clone();
        value["aligned_execution"]["scientific_admission_seconds"] = json!(self.admission_seconds);
        value
    }
    pub fn into_stream(self) -> Result<Stream> {
        let cancelled = Arc::new(AtomicBool::new(false));
        self.into_stream_cancellable(cancelled)
    }
    /// A caller-owned cancellation flag, also suitable for CLI signal handlers.
    pub fn into_stream_cancellable(self, cancelled: Arc<AtomicBool>) -> Result<Stream> {
        let provenance = self.explain();
        let flag = cancelled.clone();
        let (sender, receiver) = mpsc::sync_channel(0);
        let worker = std::thread::Builder::new()
            .name("tile-ripper-swot".into())
            .spawn(move || {
                self.base.execute_swot_cancellable(&flag, |layers| {
                    if flag.load(Ordering::Acquire) {
                        return fail("acquisition cancelled");
                    }
                    let validity = self.policy.mask(&layers)?;
                    sender
                        .send(Tile { layers, validity })
                        .map_err(|_| "consumer cancelled".into())
                })
            })?;
        Ok(Stream {
            receiver: Some(receiver),
            worker: Some(worker),
            cancelled,
            terminal: false,
            report: None,
            error: None,
            provenance,
        })
    }
}
#[must_use = "consume provisional tiles and call finish to validate completion"]
pub struct Stream {
    receiver: Option<mpsc::Receiver<Tile>>,
    worker: Option<std::thread::JoinHandle<Result<ExecutionReport>>>,
    cancelled: Arc<AtomicBool>,
    terminal: bool,
    report: Option<ExecutionReport>,
    error: Option<String>,
    pub provenance: Value,
}
impl Stream {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn join(&mut self) {
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(Ok(report)) => self.report = Some(report),
                Ok(Err(e)) => self.error = Some(e.to_string()),
                Err(_) => self.error = Some("SWOT stream worker panicked".into()),
            }
        }
    }
    pub fn finish(mut self) -> Result<ExecutionReport> {
        for tile in self.by_ref() {
            tile?;
        }
        if self.cancelled.load(Ordering::Acquire) {
            return fail("acquisition cancelled");
        }
        if let Some(e) = self.error.take() {
            return Err(e.into());
        }
        self.report
            .take()
            .ok_or_else(|| "stream ended without completion report".into())
    }
}
impl Iterator for Stream {
    type Item = Result<Tile>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.terminal {
            return None;
        }
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                self.receiver.take();
                self.join();
                self.terminal = true;
                self.error.get_or_insert("acquisition cancelled".into());
                return Some(Err(self.error.clone().unwrap().into()));
            }
            match self
                .receiver
                .as_ref()?
                .recv_timeout(std::time::Duration::from_millis(20))
            {
                Ok(tile) => return Some(Ok(tile)),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.receiver.take();
                    self.join();
                    self.terminal = true;
                    return self.error.as_ref().map(|e| Err(e.clone().into()));
                }
            }
        }
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.cancel();
        }
        self.receiver.take();
        self.join();
    }
}
