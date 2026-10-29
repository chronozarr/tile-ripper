//! Opt-in benchmark transaction timelines. Never retain query strings, headers,
//! credentials, or response bodies. Disabled unless a trace path is supplied.
use reqwest::{header::HeaderMap, Method, Url};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) struct Trace {
    id: u64,
    status: Option<u16>,
    bytes: usize,
}

fn output() -> Option<&'static Mutex<File>> {
    static OUT: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    OUT.get_or_init(|| {
        let path = std::env::var_os("TILE_RIPPER_HTTP_TRACE")?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new)
    })
    .as_ref()
}

fn event(mut value: serde_json::Value) {
    let Some(out) = output() else { return };
    value["time_unix_s"] = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| serde_json::json!(d.as_secs_f64()))
        .unwrap_or_default();
    value["pid"] = std::process::id().into();
    if let Ok(mut file) = out.lock() {
        let _ = writeln!(file, "{value}");
    }
}

impl Trace {
    pub(crate) fn head(locator: &str) -> Option<Self> {
        output()?;
        Self::start(&Method::HEAD, &Url::parse(locator).ok()?, &HeaderMap::new())
    }

    pub(crate) fn start(method: &Method, url: &Url, headers: &HeaderMap) -> Option<Self> {
        output()?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        // File names identify the pinned HLS objects without copying signed
        // routes. A synthetic route's query cannot enter this event.
        let source = url
            .path_segments()
            .and_then(|mut parts| parts.next_back())
            .unwrap_or("");
        event(serde_json::json!({
            "event":"start", "id":id, "client":"native", "method":method.as_str(),
            "source":source,
            "range": headers.get(reqwest::header::RANGE).and_then(|v| v.to_str().ok()),
            "conditional": headers.contains_key(reqwest::header::IF_MATCH),
        }));
        Some(Self {
            id,
            status: None,
            bytes: 0,
        })
    }

    pub(crate) fn received(&mut self, status: u16) {
        self.status = Some(status);
    }

    pub(crate) fn body(&mut self, bytes: usize) {
        self.bytes += bytes;
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        event(
            serde_json::json!({"event":"end", "id":self.id, "client":"native", "status":self.status, "bytes":self.bytes}),
        );
    }
}
