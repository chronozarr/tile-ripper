//! Shared admission of mature-reader operations. GDAL still owns its range I/O.
use crate::{
    execute::{empty_report, ExecutionReport},
    model::*,
    Chunk, Result,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Condvar, Mutex,
    },
    time::Instant,
};

struct Task {
    selected: usize,
    node: usize,
    host: String,
    bytes: usize,
    reader_bytes: usize,
}
#[derive(Default)]
struct State {
    next: usize,
    used: usize,
    peak: usize,
    active: usize,
    active_peak: usize,
    hosts: BTreeMap<String, usize>,
}
struct Admission {
    state: Mutex<State>,
    wake: Condvar,
}
enum Event {
    Chunk(Box<Chunk>, mpsc::SyncSender<()>),
    Done,
    Error(String),
}

enum Reader {
    Cog(Box<crate::execute::SelectedCogReader>),
    Zarr(Box<crate::zarr_acquire::SelectedReader>),
}
impl Reader {
    fn finish(&self, report: &mut ExecutionReport) -> Result<()> {
        if let Self::Zarr(reader) = self {
            reader.finish(report)?;
        }
        Ok(())
    }
    fn execute_node(
        &mut self,
        r: &Request,
        node: &Node,
        report: &mut ExecutionReport,
        sink: &mut impl FnMut(Chunk) -> Result<()>,
    ) -> Result<()> {
        match self {
            Self::Cog(reader) => reader.execute_node(node, report, sink),
            Self::Zarr(reader) => reader.execute_node(r, node, report, sink),
        }
    }
}

fn positive_env(name: &str, default: usize) -> Result<usize> {
    match std::env::var(name) {
        Ok(v) => v
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=64).contains(n))
            .ok_or_else(|| format!("{name} must be an integer from 1 to 64").into()),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(format!("invalid {name}").into()),
    }
}
fn host(a: &Asset) -> Result<String> {
    if crate::cog::remote(&a.locator) {
        Ok(reqwest::Url::parse(&a.locator)?
            .origin()
            .ascii_serialization())
    } else {
        Ok("local".into())
    }
}
fn tasks(r: &Request, p: &Plan, sources: &crate::storage::Sources) -> Result<Vec<Task>> {
    use zarrs::storage::{ReadableStorageTraits, StoreKey};
    let Source::Assets { assets } = &r.source else {
        return Err("scheduler requires bound assets".into());
    };
    let mut out = Vec::new();
    for (si, selected) in p.selected.iter().enumerate() {
        if !selected.capabilities.materialization {
            return Err("plan contains a planning-only format".into());
        }
        let a = assets
            .iter()
            .find(|a| a.id == selected.asset_id)
            .ok_or("asset binding missing")?;
        let l = selected.layout.as_ref().ok_or("missing layout")?;
        if a.format == Format::ZarrV2 {
            let Variable::Dataset { path } = &l.variable else {
                return Err("missing variable".into());
            };
            let prefix = format!("{}/", path.trim_start_matches('/'));
            let dims = selected.dimensions.as_ref().ok_or("missing dimensions")?;
            let element = if l.dtype == "<f4" { 4usize } else { 2 };
            let native = dims
                .chunk_shape
                .iter()
                .try_fold(element, |n, c| n.checked_mul(*c))
                .ok_or("chunk byte overflow")?;
            let plane = l
                .width
                .min(r.budget.chunk_edge)
                .checked_mul(l.height.min(r.budget.chunk_edge))
                .and_then(|n| n.checked_mul(element))
                .ok_or("plane overflow")?;
            let coordinates = dims
                .shape
                .iter()
                .try_fold(0usize, |n, c| n.checked_add(c.checked_mul(8)?))
                .ok_or("coordinate size overflow")?;
            let store = sources.get(a)?;
            for (ni, node) in p.nodes.iter().enumerate() {
                if node.asset_id != a.id {
                    continue;
                }
                let Operation::Read {
                    read: Read::ChunkObject { key, .. },
                } = &node.operation
                else {
                    continue;
                };
                if !key.starts_with(&prefix) {
                    continue;
                }
                // HTTP sizes are already version-pinned by planning. No probes.
                let encoded = usize::try_from(store.size_key(&StoreKey::new(key)?)?.unwrap_or(0))?;
                let bytes = native
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(encoded.checked_mul(2)?))
                    .and_then(|n| n.checked_add(plane.checked_mul(2)?))
                    .ok_or("working set overflow")?;
                out.push(Task {
                    selected: si,
                    node: ni,
                    host: host(a)?,
                    bytes,
                    reader_bytes: coordinates.checked_mul(3).ok_or("reader state overflow")?,
                });
            }
        } else {
            for (ni, node) in p
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.asset_id == a.id)
            {
                if let Operation::Read {
                    read: Read::LibraryWindow { window, band, .. },
                } = &node.operation
                {
                    if l.variable != (Variable::Band { index: *band }) {
                        continue;
                    }
                    let bytes = window
                        .width
                        .checked_mul(window.height)
                        .and_then(|n| n.checked_mul(2))
                        .ok_or("window byte overflow")?;
                    out.push(Task {
                        selected: si,
                        node: ni,
                        host: host(a)?,
                        bytes,
                        reader_bytes: 0,
                    });
                }
            }
        }
    }
    if out.iter().any(|t| t.bytes > r.budget.buffer_bytes) {
        return Err("one read operation exceeds the scheduler working buffer; increase buffer_bytes or reduce chunk size".into());
    }
    Ok(out)
}

pub(crate) fn execute(
    r: &Request,
    p: &Plan,
    planning: f64,
    sources: &crate::storage::Sources,
    cancelled: &AtomicBool,
    mut sink: impl FnMut(Chunk) -> Result<()>,
) -> Result<ExecutionReport> {
    let started = Instant::now();
    let tasks = tasks(r, p, sources)?;
    let mut workers = positive_env(
        "TILE_RIPPER_WORKERS",
        std::thread::available_parallelism().map_or(1, |n| n.get().min(4)),
    )?
    .min(tasks.len().max(1));
    // Reader contexts retain decoded coordinates/dimension indices across tasks.
    // Reserve their largest possible footprint for every worker for its lifetime.
    let per_reader = tasks.iter().map(|t| t.reader_bytes).max().unwrap_or(0);
    let largest_task = tasks.iter().map(|t| t.bytes).max().unwrap_or(0);
    while workers > 1
        && per_reader
            .checked_mul(workers)
            .and_then(|n| n.checked_add(largest_task))
            .is_none_or(|n| n > r.budget.buffer_bytes)
    {
        workers -= 1;
    }
    let retained = per_reader
        .checked_mul(workers)
        .filter(|n| {
            n.checked_add(largest_task)
                .is_some_and(|total| total <= r.budget.buffer_bytes)
        })
        .ok_or("reader state and one task exceed buffer_bytes; increase the working buffer")?;
    let per_host = positive_env("TILE_RIPPER_PER_HOST", 4)?;
    let Source::Assets { assets } = &r.source else {
        unreachable!()
    };
    // Invalidate URL-only COG caches once, before any worker opens a handle.
    for a in assets
        .iter()
        .filter(|a| a.format == Format::Cog && a.etag.is_some())
    {
        crate::cog::clear_remote_cache(a)?;
    }
    let admission = Admission {
        state: Mutex::new(State {
            used: retained,
            peak: retained,
            ..State::default()
        }),
        wake: Condvar::new(),
    };
    let abort = AtomicBool::new(false);
    let channels: Vec<_> = tasks.iter().map(|_| mpsc::sync_channel(0)).collect();
    let (senders, receivers): (Vec<_>, Vec<_>) = channels.into_iter().unzip();
    // Copy the caller's thread-local GDAL configuration into each worker.
    let configuration: Vec<_> = [
        "GDAL_HTTP_HEADERS",
        "GDAL_DISABLE_READDIR_ON_OPEN",
        "GDAL_INGESTED_BYTES_AT_OPEN",
        "GDAL_NUM_THREADS",
        "GDAL_CACHEMAX",
        "GDAL_HTTP_MAX_RETRY",
        "GDAL_HTTP_RETRY_DELAY",
        "GDAL_HTTP_TIMEOUT",
        "CPL_DEBUG",
        "CPL_CURL_VERBOSE",
    ]
    .iter()
    .map(|k| Ok((*k, gdal::config::get_thread_local_config_option(k, "")?)))
    .collect::<Result<_>>()?;
    let mut result = empty_report(planning);
    let outcome = std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let (tasks, senders, admission, abort, configuration) =
                (&tasks, &senders, &admission, &abort, &configuration);
            handles.push(scope.spawn(move || -> Result<ExecutionReport> {
                let _quiet = crate::diagnostics::QuietNative::enter();
                for (key, value) in configuration {
                    if !value.is_empty() {
                        gdal::config::set_thread_local_config_option(key, value)?;
                    }
                }
                let mut report = empty_report(0.);
                let mut reader: Option<(usize, Reader)> = None;
                loop {
                    let index = {
                        let mut state = admission.state.lock().unwrap();
                        loop {
                            if abort.load(Ordering::Acquire)
                                || cancelled.load(Ordering::Acquire)
                                || state.next == tasks.len()
                            {
                                break None;
                            }
                            let task = &tasks[state.next];
                            if state.used + task.bytes <= r.budget.buffer_bytes
                                && state.hosts.get(&task.host).copied().unwrap_or(0) < per_host
                            {
                                let index = state.next;
                                state.next += 1;
                                state.used += task.bytes;
                                state.peak = state.peak.max(state.used);
                                state.active += 1;
                                state.active_peak = state.active_peak.max(state.active);
                                *state.hosts.entry(task.host.clone()).or_default() += 1;
                                // Wake another worker to claim the following operation.
                                admission.wake.notify_all();
                                break Some(index);
                            }
                            state = admission
                                .wake
                                .wait_timeout(state, std::time::Duration::from_millis(20))
                                .unwrap()
                                .0;
                        }
                    };
                    let Some(index) = index else {
                        break;
                    };
                    let task = &tasks[index];
                    let sel = &p.selected[task.selected];
                    let asset = assets
                        .iter()
                        .find(|a| a.id == sel.asset_id)
                        .ok_or("asset binding missing")?;
                    let mut deliver = |c: Chunk| -> Result<()> {
                        if abort.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire) {
                            return Err("acquisition cancelled".into());
                        }
                        let (ack, received) = mpsc::sync_channel(0);
                        senders[index]
                            .send(Event::Chunk(Box::new(c), ack))
                            .map_err(|_| "consumer cancelled")?;
                        received.recv().map_err(|_| "consumer cancelled".into())
                    };
                    let work =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                            if reader.as_ref().is_none_or(|(si, _)| *si != task.selected) {
                                if let Some((_, previous)) = reader.take() {
                                    previous.finish(&mut report)?;
                                }
                                let open = Instant::now();
                                let physical = match asset.format {
                                    Format::Cog => Reader::Cog(Box::new(
                                        crate::execute::SelectedCogReader::new(asset, sel)?,
                                    )),
                                    Format::ZarrV2 => Reader::Zarr(Box::new(
                                        crate::zarr_acquire::SelectedReader::new(
                                            r, asset, sources, sel,
                                        )?,
                                    )),
                                };
                                reader = Some((task.selected, physical));
                                report.source_open_seconds += open.elapsed().as_secs_f64();
                            }
                            reader.as_mut().unwrap().1.execute_node(
                                r,
                                &p.nodes[task.node],
                                &mut report,
                                &mut deliver,
                            )?;
                            Ok(())
                        }))
                        .unwrap_or_else(|_| {
                            Err("reader worker panicked during acquisition".into())
                        });
                    let failed = work.is_err();
                    let event = match work {
                        Ok(()) => Event::Done,
                        Err(e) => Event::Error(e.to_string()),
                    };
                    let _ = senders[index].send(event);
                    {
                        let mut state = admission.state.lock().unwrap();
                        state.used -= task.bytes;
                        state.active -= 1;
                        *state.hosts.get_mut(&task.host).unwrap() -= 1;
                        admission.wake.notify_all();
                    }
                    if failed {
                        break;
                    }
                }
                if let Some((_, previous)) = reader {
                    previous.finish(&mut report)?;
                }
                Ok(report)
            }));
        }
        // Per-operation rendezvous channels bound reordering to active workers.
        // Drain in plan order so file sinks and callbacks remain deterministic.
        let consume = (|| -> Result<()> {
            for receiver in &receivers {
                loop {
                    if cancelled.load(Ordering::Acquire) {
                        return Err("acquisition cancelled".into());
                    }
                    match receiver.recv_timeout(std::time::Duration::from_millis(20)) {
                        Ok(Event::Chunk(chunk, ack)) => {
                            let t = Instant::now();
                            sink(*chunk)?;
                            result.sink_seconds += t.elapsed().as_secs_f64();
                            if result.first_output_seconds.is_none() {
                                result.first_output_seconds = Some(started.elapsed().as_secs_f64());
                            }
                            let _ = ack.send(());
                        }
                        Ok(Event::Done) => break,
                        Ok(Event::Error(error)) => return Err(error.into()),
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if handles.iter().all(|h| h.is_finished()) {
                                return Err(
                                    "worker ended without completing all observations".into()
                                );
                            }
                        }
                        Err(_) => return Err("worker ended without completion".into()),
                    }
                }
            }
            Ok(())
        })();
        abort.store(true, Ordering::Release);
        admission.wake.notify_all();
        drop(receivers); // Unblock every producer before joining, also on errors.
        let mut joined = Ok(());
        for handle in handles {
            match handle.join() {
                Ok(Ok(report)) => merge(&mut result, report),
                Ok(Err(e)) => {
                    joined = Err(e);
                }
                Err(_) => {
                    joined = Err("acquisition worker panicked".into());
                }
            }
        }
        consume?;
        joined
    });
    outcome?;
    // Sink time above is consumer CPU/I/O only; adapter sink timers include waits.
    let state = admission.state.lock().unwrap();
    result
        .source_reads
        .sort_by(|a, b| (&a.asset_id, &a.key).cmp(&(&b.asset_id, &b.key)));
    result.scheduler = serde_json::json!({"workers":workers,"per_host":per_host,"buffer_bytes":r.budget.buffer_bytes,"reserved_bytes_peak":state.peak,"reader_state_reserved_bytes":retained,"active_workers_peak":state.active_peak,"completed_tasks":tasks.len(),"seconds":started.elapsed().as_secs_f64(),"order":"plan","memory_scope":"conservative reader working reservations; excludes process/runtime, GDAL cache, prepared metadata and caller retention"});
    Ok(result)
}
fn add(a: &mut Option<f64>, b: Option<f64>) {
    if let Some(b) = b {
        *a = Some(a.unwrap_or(0.) + b);
    }
}
fn add_u(a: &mut Option<u64>, b: Option<u64>) {
    if let Some(b) = b {
        *a = Some(a.unwrap_or(0) + b);
    }
}
fn merge(a: &mut ExecutionReport, b: ExecutionReport) {
    a.source_reads.extend(b.source_reads);
    a.decoded_source_chunks += b.decoded_source_chunks;
    a.filled_source_chunks += b.filled_source_chunks;
    a.assembly_seconds += b.assembly_seconds;
    add(&mut a.source_retrieval_seconds, b.source_retrieval_seconds);
    add(&mut a.allocation_seconds, b.allocation_seconds);
    add(&mut a.copying_seconds, b.copying_seconds);
    if let Some(n) = b.native_materialized_bytes {
        a.native_materialized_bytes = Some(a.native_materialized_bytes.unwrap_or(0) + n);
    }
    add_u(&mut a.source_data_requests, b.source_data_requests);
    add_u(&mut a.source_data_bytes, b.source_data_bytes);
    a.chunks += b.chunks;
    a.decoded_bytes += b.decoded_bytes;
    a.non_nodata_pixels += b.non_nodata_pixels;
    a.source_open_seconds += b.source_open_seconds;
    a.read_decode_seconds += b.read_decode_seconds;
    a.masking_seconds += b.masking_seconds;
}
