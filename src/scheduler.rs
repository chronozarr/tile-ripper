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
    nodes: Vec<usize>,
    host: String,
    bytes: usize,
    reader_bytes: usize,
    batch: Option<std::sync::Arc<crate::zarr_v3::Batch>>,
    #[cfg(feature = "native-cog")]
    native: crate::native_cog::Strategy,
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
    #[cfg(feature = "native-hdf5")]
    Hdf5(Box<crate::hdf5::SelectedReader>),
    Cog(Box<crate::execute::SelectedCogReader>),
    Zarr(Box<crate::zarr_acquire::SelectedReader>),
}
impl Reader {
    fn finish(&self, report: &mut ExecutionReport) -> Result<()> {
        #[cfg(feature = "native-hdf5")]
        if let Self::Hdf5(reader) = self {
            reader.finish(report);
        }
        if let Self::Zarr(reader) = self {
            reader.finish(report)?;
        } else if let Self::Cog(reader) = self {
            reader.finish(report);
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
            #[cfg(feature = "native-hdf5")]
            Self::Hdf5(reader) => {
                let Source::Assets { assets } = &r.source else {
                    return Err("missing assets".into());
                };
                let asset = assets
                    .iter()
                    .find(|a| a.id == node.asset_id)
                    .ok_or("missing HDF5 asset")?;
                reader.execute_node(asset, node, report, sink)
            }
            Self::Cog(reader) => reader.execute_node(node, report, sink),
            Self::Zarr(reader) => reader.execute_node(r, node, report, sink),
        }
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
fn element_bytes(dtype: &str) -> Result<usize> {
    match dtype {
        "Byte" | "|u1" => Ok(1),
        "UInt16" | "Int16" | "<u2" | "<i2" => Ok(2),
        "Float32" | "UInt32" | "<f4" | "<u4" => Ok(4),
        _ => Err(format!("unsupported scheduler dtype {dtype}").into()),
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
        if a.format == Format::Hdf5 {
            #[cfg(feature = "native-hdf5")]
            {
                let Variable::Dataset { path } = &l.variable else {
                    return Err("missing HDF5 path".into());
                };
                let binding = sources.hdf5.binding(&a.id, path)?;
                for group in &binding.groups {
                    out.push(Task {
                        selected: si,
                        nodes: group.nodes.clone(),
                        host: host(a)?,
                        bytes: group.reservation,
                        reader_bytes: 0,
                        batch: None,
                        #[cfg(feature = "native-cog")]
                        native: Default::default(),
                    });
                }
                continue;
            }
            #[cfg(not(feature = "native-hdf5"))]
            return Err("build with native-hdf5".into());
        }
        if a.format.is_zarr() {
            let Variable::Dataset { path } = &l.variable else {
                return Err("missing variable".into());
            };
            let prefix = format!("{}/", path.trim_start_matches('/'));
            let dims = selected.dimensions.as_ref().ok_or("missing dimensions")?;
            let element = element_bytes(&l.dtype)?;
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
                    read: Read::ChunkObject { key, indices },
                } = &node.operation
                else {
                    continue;
                };
                if !key.starts_with(&prefix) {
                    continue;
                }
                // HTTP sizes are already version-pinned by planning. No probes.
                let encoded = if a.format == Format::ZarrV3 {
                    sources.v3.binding(&a.id, path)?.encoded_bytes(indices)?
                } else {
                    usize::try_from(store.size_key(&StoreKey::new(key)?)?.unwrap_or(0))?
                };
                let bytes = native
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(encoded.checked_mul(2)?))
                    .and_then(|n| n.checked_add(plane.checked_mul(2)?))
                    .ok_or("working set overflow")?;
                out.push(Task {
                    selected: si,
                    nodes: vec![ni],
                    host: host(a)?,
                    bytes,
                    reader_bytes: if a.format == Format::ZarrV3 {
                        0
                    } else {
                        coordinates.checked_mul(3).ok_or("reader state overflow")?
                    },
                    batch: None,
                    #[cfg(feature = "native-cog")]
                    native: Default::default(),
                });
            }
        } else {
            // A selection owns one GDAL handle. Its windows share decoded source
            // blocks even when several output chunks intersect the same tile.
            // Only one output buffer is live per admitted selection.
            let mut nodes = Vec::new();
            let mut bytes = 0;
            let element = element_bytes(&l.dtype)?;
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
                    let window_bytes = window
                        .width
                        .checked_mul(window.height)
                        .and_then(|n| n.checked_mul(element))
                        .ok_or("window byte overflow")?;
                    bytes = bytes.max(window_bytes);
                    nodes.push(ni);
                }
            }
            if !nodes.is_empty() {
                #[allow(unused_mut)]
                let mut reader_bytes = 0;
                #[cfg(feature = "native-cog")]
                let mut native = crate::native_cog::Strategy::default();
                #[cfg(feature = "native-cog")]
                if crate::cog::backend()? == crate::cog::Backend::Native {
                    let read_nodes: Vec<_> = nodes.iter().map(|i| &p.nodes[*i]).collect();
                    let (retained, working, strategy) = crate::native_cog::reservation(
                        l,
                        &selected.window,
                        &read_nodes,
                        r.budget
                            .buffer_bytes
                            .saturating_sub(sources.native.bytes())
                            .saturating_sub(sources.transport_reservation()?),
                        crate::storage::worker_count()?.min(p.selected.len().max(1)),
                    )?;
                    native = strategy;
                    bytes = bytes
                        .checked_mul(2)
                        .and_then(|n| n.checked_add(working))
                        .and_then(|n| n.checked_add(retained))
                        .ok_or("native COG buffer reservation overflow")?;
                }
                out.push(Task {
                    selected: si,
                    nodes,
                    host: host(a)?,
                    bytes,
                    reader_bytes,
                    batch: None,
                    #[cfg(feature = "native-cog")]
                    native,
                });
            }
        }
    }
    if out.iter().any(|t| t.bytes > r.budget.buffer_bytes) {
        return Err("one read operation exceeds the scheduler working buffer; increase buffer_bytes or reduce chunk size".into());
    }
    if sources.v3.bytes() == 0 {
        return Ok(out);
    }
    coalesce_tasks(r, p, sources, out)
}

fn coalesce_tasks(
    r: &Request,
    p: &Plan,
    sources: &crate::storage::Sources,
    tasks: Vec<Task>,
) -> Result<Vec<Task>> {
    let workers = crate::storage::worker_count()?
        .min(crate::runtime_config::per_host()?)
        .min(tasks.len().max(1));
    let config = crate::runtime_config::zarr_ranges(workers)?;
    if config.policy == crate::zarr_ranges::Policy::Individual {
        return Ok(tasks);
    }
    #[allow(unused_mut)]
    let mut prepared = sources.v3_prepared_bytes();
    #[cfg(feature = "native-cog")]
    {
        let transport = sources.transport_reservation()?;
        prepared = prepared
            .checked_add(sources.native.bytes())
            .and_then(|bytes| bytes.checked_add(transport))
            .ok_or("native COG prepared reservation overflow")?;
    }
    // Upper bound for the immutable batch descriptions, held for execution.
    // If even this small planning overhead is unaffordable, retain the old tasks.
    let metadata = tasks.len().saturating_mul(512).saturating_add(
        p.nodes
            .iter()
            .filter_map(|n| match &n.operation {
                Operation::Read {
                    read: Read::ChunkObject { key, .. },
                } => Some(key.len()),
                _ => None,
            })
            .sum::<usize>(),
    );
    let available = r.budget.buffer_bytes.saturating_sub(prepared);
    if metadata > available / 4 {
        return Ok(tasks);
    }
    let quota = (available - metadata) / workers;
    let Source::Assets { assets } = &r.source else {
        unreachable!()
    };
    let mut result = Vec::new();
    let mut input = tasks.into_iter().peekable();
    while let Some(first) = input.next() {
        let sel = &p.selected[first.selected];
        let asset = assets
            .iter()
            .find(|a| a.id == sel.asset_id)
            .ok_or("asset binding missing")?;
        let Variable::Dataset { path } = &sel.layout.as_ref().ok_or("layout missing")?.variable
        else {
            result.push(first);
            continue;
        };
        if asset.format != Format::ZarrV3 {
            result.push(first);
            continue;
        }
        let binding = sources.v3.binding(&asset.id, path)?;
        if !binding.coalescible() {
            result.push(first);
            continue;
        }
        let mut window = vec![first];
        while window.len() < 32
            && input
                .peek()
                .is_some_and(|t| t.selected == window[0].selected)
        {
            window.push(input.next().unwrap());
        }
        // Split until both retained bodies and decode/output work fit a fair
        // share. Small budgets therefore retain concurrency or individual reads.
        let mut pending = std::collections::VecDeque::from([window]);
        while let Some(mut window) = pending.pop_front() {
            let working = window.iter().map(|t| t.bytes).max().unwrap();
            let logical = window
                .iter()
                .flat_map(|t| &t.nodes)
                .map(|i| match &p.nodes[*i].operation {
                    Operation::Read {
                        read: Read::ChunkObject { indices, .. },
                    } => indices.clone(),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            let cap = quota.saturating_sub(working) / 2;
            let batch = binding.batch(&logical, &config, cap)?;
            let bytes = working.saturating_add(batch.bytes().saturating_mul(2));
            // A batch executes its remaining requests serially. Require the
            // cost policy to beat the original parallel request waves even
            // when this is the only ready window. Avoid serializing a sparse
            // window merely because two of its members happen to be close.
            let present = logical.iter().try_fold(0usize, |n, i| -> Result<_> {
                Ok(n + usize::from(binding.encoded_bytes(i)? > 0))
            })?;
            let requests = present
                - batch
                    .fetches
                    .iter()
                    .map(|f| f.range.members.len() - 1)
                    .sum::<usize>();
            let loses_parallelism = config.policy == crate::zarr_ranges::Policy::Cost
                && requests > present.div_ceil(workers);
            if (bytes > quota || loses_parallelism) && window.len() > 1 {
                let tail = window.split_off(window.len() / 2);
                pending.push_front(tail);
                pending.push_front(window);
                continue;
            }
            if batch.fetches.is_empty() || bytes > quota {
                result.extend(window);
                continue;
            }
            let mut task = window.remove(0);
            for t in window {
                task.nodes.extend(t.nodes);
            }
            task.bytes = bytes;
            task.batch = Some(std::sync::Arc::new(batch));
            result.push(task);
        }
    }
    Ok(result)
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
    #[allow(unused_mut)]
    let mut prepared_bytes = 0usize;
    #[cfg(feature = "native-cog")]
    if crate::cog::native_selected() {
        prepared_bytes = sources
            .native
            .bytes()
            .checked_add(sources.transport_reservation()?)
            .ok_or("native COG prepared reservation overflow")?;
    }
    #[cfg(feature = "native-hdf5")]
    {
        prepared_bytes = prepared_bytes
            .checked_add(sources.hdf5.bytes())
            .ok_or("HDF5 reservation overflow")?;
    }
    prepared_bytes = prepared_bytes
        .checked_add(sources.v3_prepared_bytes())
        .and_then(|n| {
            n.checked_add(
                tasks
                    .iter()
                    .filter_map(|t| t.batch.as_ref())
                    .map(|b| b.metadata_bytes())
                    .sum::<usize>(),
            )
        })
        .ok_or("prepared reservation overflow")?;
    let mut workers = crate::runtime_config::workers()?.min(tasks.len().max(1));
    // Reader contexts retain decoded coordinates/dimension indices across tasks.
    // Reserve their largest possible footprint for every worker for its lifetime.
    let per_reader = tasks.iter().map(|t| t.reader_bytes).max().unwrap_or(0);
    let largest_task = tasks.iter().map(|t| t.bytes).max().unwrap_or(0);
    while workers > 1
        && per_reader
            .checked_mul(workers)
            .and_then(|n| n.checked_add(largest_task))
            .and_then(|n| n.checked_add(prepared_bytes))
            .is_none_or(|n| n > r.budget.buffer_bytes)
    {
        workers -= 1;
    }
    let retained = per_reader
        .checked_mul(workers)
        .filter(|n| {
            n.checked_add(largest_task)
                .and_then(|total| total.checked_add(prepared_bytes))
                .is_some_and(|total| total <= r.budget.buffer_bytes)
        })
        .ok_or("reader state and one task exceed buffer_bytes; increase the working buffer")?;
    let per_host = crate::runtime_config::per_host()?;
    let Source::Assets { assets } = &r.source else {
        unreachable!()
    };
    // Preparation invalidates URL caches before binding the source ETag.
    // Pre/post version checks protect reuse of that same prepared snapshot.
    let admission = Admission {
        state: Mutex::new(State {
            used: retained + prepared_bytes,
            peak: retained + prepared_bytes,
            ..State::default()
        }),
        wake: Condvar::new(),
    };
    let abort = AtomicBool::new(false);
    let channels: Vec<_> = tasks.iter().map(|_| mpsc::sync_channel(0)).collect();
    let (senders, receivers): (Vec<_>, Vec<_>) = channels.into_iter().unzip();
    // Copy the caller's thread-local GDAL configuration into each worker.
    let configuration = crate::cog::thread_configuration()?;
    let mut result = empty_report(planning);
    let outcome = std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        let runtime = crate::runtime_config::active();
        for _ in 0..workers {
            let (tasks, senders, admission, abort, configuration) =
                (&tasks, &senders, &admission, &abort, &configuration);
            let runtime = runtime.clone();
            handles.push(scope.spawn(move || -> Result<ExecutionReport> {
                let _runtime = runtime
                    .as_ref()
                    .map(crate::runtime_config::RuntimeConfig::enter);
                let _quiet = crate::diagnostics::QuietNative::enter();
                crate::cog::apply_thread_configuration(configuration)?;
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
                                    Format::Hdf5 => {
                                        #[cfg(feature = "native-hdf5")]
                                        {
                                            Reader::Hdf5(Box::new(
                                                crate::hdf5::SelectedReader::new(
                                                    asset,
                                                    sel,
                                                    &sources.hdf5,
                                                    &task.nodes,
                                                )?,
                                            ))
                                        }
                                        #[cfg(not(feature = "native-hdf5"))]
                                        return Err("build with native-hdf5".into());
                                    }
                                    Format::Cog => Reader::Cog(Box::new(
                                        crate::execute::SelectedCogReader::new(
                                            asset,
                                            sel,
                                            sources,
                                            #[cfg(feature = "native-cog")]
                                            task.native.clone(),
                                        )?,
                                    )),
                                    Format::ZarrV2 | Format::ZarrV3 => Reader::Zarr(Box::new(
                                        crate::zarr_acquire::SelectedReader::new(
                                            r, asset, sources, sel,
                                        )?,
                                    )),
                                };
                                reader = Some((task.selected, physical));
                                report.source_open_seconds += open.elapsed().as_secs_f64();
                            }
                            if let Reader::Zarr(r) = &mut reader.as_mut().unwrap().1 {
                                r.begin_batch(task.batch.clone());
                            }
                            for node in &task.nodes {
                                if abort.load(Ordering::Acquire)
                                    || cancelled.load(Ordering::Acquire)
                                {
                                    return Err("acquisition cancelled".into());
                                }
                                reader.as_mut().unwrap().1.execute_node(
                                    r,
                                    &p.nodes[*node],
                                    &mut report,
                                    &mut deliver,
                                )?;
                            }
                            Ok(())
                        }))
                        .unwrap_or_else(|_| {
                            Err("reader worker panicked during acquisition".into())
                        });
                    // Runs on success, errors, cancellation and caught panics.
                    // No slice or body may outlive its task reservation.
                    if let Some((_, Reader::Zarr(r))) = &mut reader {
                        r.end_batch();
                    }
                    let failed = work.is_err();
                    // COG selections are single tasks, so their block caches
                    // have no cross-task lifetime. Drop before releasing the
                    // task's cache+working reservation. Zarr coordinates remain
                    // retained across that selection's individual chunk tasks.
                    #[allow(unused_mut)]
                    let mut release = matches!(reader, Some((_, Reader::Cog(_))));
                    #[cfg(feature = "native-hdf5")]
                    {
                        release |= matches!(reader, Some((_, Reader::Hdf5(_))));
                    }
                    if release {
                        if let Some((_, previous)) = reader.take() {
                            previous.finish(&mut report)?;
                        }
                    }
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
    let range_metrics = result.scheduler.get("zarr_v3_ranges").cloned();
    let native_buffers = result.scheduler.get("native_buffers").cloned();
    result.scheduler = serde_json::json!({"workers":workers,"per_host":per_host,"buffer_bytes":r.budget.buffer_bytes,"reserved_bytes_peak":state.peak,"reader_state_reserved_bytes":retained,"prepared_native_metadata_bytes":prepared_bytes,"authenticated_transport_reserved_bytes":sources.transport_reservation()?,"prepared_v3_metadata_index_bytes":sources.v3_prepared_bytes(),"active_workers_peak":state.active_peak,"completed_tasks":tasks.iter().map(|t|t.nodes.len()).sum::<usize>(),"admitted_jobs":tasks.len(),"cog_reader_scope":"selection","seconds":started.elapsed().as_secs_f64(),"order":"plan","memory_scope":"conservative reader working reservations; includes authenticated transport sessions, prepared COG and v3 metadata/index reservations, encoded data and working output planes; excludes process/runtime, GDAL cache, legacy v2 prepared metadata and caller retention"});
    #[cfg(feature = "native-hdf5")]
    {
        result.scheduler["prepared_hdf5_bytes"] = serde_json::json!(sources.hdf5.bytes());
    }
    if sources.v3.bytes() > 0 {
        result.scheduler["zarr_v3_coalescing"] = serde_json::json!({"config":crate::runtime_config::zarr_ranges(workers.min(per_host))?,"window_logical_chunks":32,"plan_metadata_bytes":tasks.iter().filter_map(|t|t.batch.as_ref()).map(|b|b.metadata_bytes()).sum::<usize>(),"batches":tasks.iter().filter_map(|t|t.batch.as_ref().map(|b|serde_json::json!({"selected":t.selected,"nodes":t.nodes,"reservation":t.bytes,"fetches":b.fetches}))).collect::<Vec<_>>()});
    }
    if let Some(metrics) = range_metrics {
        result.scheduler["zarr_v3_ranges"] = metrics;
    }
    if let Some(buffers) = native_buffers {
        result.scheduler["native_buffers"] = buffers;
    }
    #[cfg(feature = "native-cog")]
    if crate::cog::native_selected() {
        result.scheduler["native_metadata"] = sources.native.summary();
        result.scheduler["native_strategies"] = serde_json::json!(tasks
            .iter()
            .map(
                |t| serde_json::json!({"asset_id":p.selected[t.selected].asset_id,
                "task_reserved_bytes":t.bytes,"strategy":t.native.summary()})
            )
            .collect::<Vec<_>>());
    }
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
    if let Some(metrics) = b.scheduler.get("zarr_v3_ranges").and_then(|v| v.as_array()) {
        if a.scheduler.is_null() {
            a.scheduler = serde_json::json!({});
        }
        if !a.scheduler["zarr_v3_ranges"].is_array() {
            a.scheduler["zarr_v3_ranges"] = serde_json::json!([]);
        }
        a.scheduler["zarr_v3_ranges"]
            .as_array_mut()
            .unwrap()
            .extend(metrics.iter().cloned());
    }
    if let Some(buffers) = b.scheduler.get("native_buffers").and_then(|v| v.as_array()) {
        if a.scheduler.is_null() {
            a.scheduler = serde_json::json!({});
        }
        if !a.scheduler["native_buffers"].is_array() {
            a.scheduler["native_buffers"] = serde_json::json!([]);
        }
        a.scheduler["native_buffers"]
            .as_array_mut()
            .unwrap()
            .extend(buffers.iter().cloned());
    }
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
