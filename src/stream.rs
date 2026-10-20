//! Incremental native chunks with bounded buffering and explicit completion.
//!
//! Values received before successful `finish` are provisional: post-execution
//! source validation may still fail. Dropping cancels and joins the producer.
use crate::{acquisition::Prepared, Chunk, ExecutionReport, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

#[derive(Clone, Copy, Debug)]
pub struct StreamOptions {
    /// Number of queued chunks, 0 for rendezvous. Default 1; maximum 64.
    /// The queue adds at most `capacity * largest_output_chunk_bytes` to the
    /// scheduler working set. Metadata, GDAL caches and caller retention differ.
    pub capacity: usize,
}
impl Default for StreamOptions {
    fn default() -> Self {
        Self { capacity: 1 }
    }
}
/// Ordered provisional chunks; only successful [`ChunkStream::finish`] authorizes
/// committing consumer results. EOF alone is not a completion receipt.
#[must_use = "consume the provisional chunks and call finish to validate completion"]
pub struct ChunkStream {
    receiver: Option<mpsc::Receiver<Chunk>>,
    worker: Option<std::thread::JoinHandle<Result<ExecutionReport>>>,
    cancelled: Arc<AtomicBool>,
    terminal: bool,
    report: Option<ExecutionReport>,
    error: Option<String>,
    /// Prepared source references and pinned versions, with credentials removed.
    pub provenance: serde_json::Value,
}
impl ChunkStream {
    pub(crate) fn start(prepared: Prepared, options: StreamOptions) -> Result<Self> {
        if options.capacity > 64 {
            return Err("stream capacity must be between 0 and 64".into());
        }
        let provenance = prepared.explain();
        let (sender, receiver) = mpsc::sync_channel(options.capacity);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let worker = std::thread::Builder::new()
            .name("tile-ripper-stream".into())
            .spawn(move || {
                prepared.execute_cancellable(&flag, |chunk| {
                    if flag.load(Ordering::Acquire) {
                        return Err("acquisition cancelled".into());
                    }
                    sender.send(chunk).map_err(|_| "consumer cancelled".into())
                })
            })?;
        Ok(Self {
            receiver: Some(receiver),
            worker: Some(worker),
            cancelled,
            terminal: false,
            report: None,
            error: None,
            provenance,
        })
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    /// Drain unread chunks and validate final source versions. Never reports
    /// completion merely because the data channel closed.
    pub fn finish(mut self) -> Result<ExecutionReport> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err("acquisition cancelled".into());
        }
        for chunk in self.by_ref() {
            chunk?;
        }
        if let Some(error) = self.error.take() {
            return Err(error.into());
        }
        self.report
            .take()
            .ok_or_else(|| "stream ended without completion report".into())
    }
    fn join(&mut self) {
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(Ok(report)) => self.report = Some(report),
                Ok(Err(e)) => self.error = Some(e.to_string()),
                Err(_) => self.error = Some("acquisition stream worker panicked".into()),
            }
        }
    }
}
impl Iterator for ChunkStream {
    type Item = Result<Chunk>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.terminal {
            return None;
        }
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                self.receiver.take();
                self.join();
                self.terminal = true;
                let error = self
                    .error
                    .clone()
                    .unwrap_or_else(|| "acquisition cancelled".into());
                self.error = Some(error.clone());
                return Some(Err(error.into()));
            }
            match self
                .receiver
                .as_ref()?
                .recv_timeout(std::time::Duration::from_millis(20))
            {
                Ok(chunk) => return Some(Ok(chunk)),
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
impl Drop for ChunkStream {
    fn drop(&mut self) {
        self.cancel();
        self.receiver.take();
        self.join();
    }
}
