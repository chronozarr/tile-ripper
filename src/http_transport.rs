//! The anonymous path retains its own client and never consults credentials.
use crate::Result;
use reqwest::{header::HeaderMap, Method, Url};
use std::{io::Read, sync::atomic::AtomicBool};

#[path = "http_trace.rs"]
mod trace;
pub(crate) use trace::Trace;

/// One anonymous COG connection pool for identity checks and conditional TIFF
/// reads. Connections may be reused; request URLs, ETags and metadata are not.
pub(crate) fn cog_client() -> Result<reqwest::blocking::Client> {
    use std::{sync::OnceLock, time::Duration};
    static CLIENT: OnceLock<std::result::Result<reqwest::blocking::Client, &'static str>> =
        OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(45))
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("tile-ripper/0.1")
                .build()
                .map_err(|_| "anonymous COG client initialization failed")
        })
        .as_ref()
        .cloned()
        .map_err(|e| (*e).into())
}

#[derive(Clone, Default)]
pub(crate) enum Transport {
    #[default]
    Anonymous,
    #[cfg(feature = "authenticated-http")]
    Authenticated(std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>),
    #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
    Checkpointed {
        session: std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
        store: std::sync::Arc<crate::checkpoint::Store>,
    },
}
pub(crate) enum Response<'a> {
    Anonymous(
        reqwest::blocking::Response,
        Option<Trace>,
        std::marker::PhantomData<&'a ()>,
    ),
    #[cfg(feature = "authenticated-http")]
    Authenticated(crate::authenticated_http::Response<'a>),
}
impl Transport {
    pub(crate) fn send<'a>(
        &'a self,
        client: &reqwest::blocking::Client,
        method: Method,
        url: Url,
        headers: HeaderMap,
        _cancelled: Option<&'a AtomicBool>,
    ) -> Result<Response<'a>> {
        match self {
            Self::Anonymous => {
                let mut trace = Trace::start(&method, &url, &headers);
                client
                    .request(method, url)
                    .headers(headers)
                    .send()
                    .map(|r| {
                        if let Some(t) = &mut trace {
                            t.received(r.status().as_u16());
                        }
                        Response::Anonymous(r, trace, std::marker::PhantomData)
                    })
                    .map_err(|_| "retryable HTTP transport failure".into())
            }
            #[cfg(feature = "authenticated-http")]
            Self::Authenticated(t) => t
                .send(method, url, headers, _cancelled)
                .map(Response::Authenticated),
            #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
            Self::Checkpointed { session, .. } => session
                .send(method, url, headers, _cancelled)
                .map(Response::Authenticated),
        }
    }
    pub(crate) fn authenticated(&self) -> bool {
        !matches!(self, Self::Anonymous)
    }
    #[cfg(feature = "native-hdf5")]
    pub(crate) fn reservation(&self) -> usize {
        match self {
            Self::Anonymous => 0,
            #[cfg(feature = "authenticated-http")]
            Self::Authenticated(_) => crate::authenticated_http::WORKING_RESERVATION,
            #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
            Self::Checkpointed { store, .. } => {
                crate::authenticated_http::WORKING_RESERVATION + store.reservation()
            }
        }
    }
    pub(crate) fn identity(&self) -> serde_json::Value {
        match self {
            Self::Anonymous => serde_json::Value::Null,
            #[cfg(feature = "authenticated-http")]
            Self::Authenticated(t) => t.identity(),
            #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
            Self::Checkpointed { session, .. } => session.identity(),
        }
    }
    #[cfg(feature = "native-hdf5")]
    pub(crate) fn stats(&self) -> serde_json::Value {
        match self {
            Self::Anonymous => serde_json::Value::Null,
            #[cfg(feature = "authenticated-http")]
            Self::Authenticated(t) => t.stats(),
            #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
            Self::Checkpointed { session, .. } => session.stats(),
        }
    }
    #[cfg(all(feature = "native-hdf5", feature = "authenticated-http"))]
    pub(crate) fn checkpoint(&self) -> Option<&crate::checkpoint::Store> {
        match self {
            Self::Checkpointed { store, .. } => Some(store),
            _ => None,
        }
    }
}
impl Response<'_> {
    pub(crate) fn status(&self) -> u16 {
        match self {
            Self::Anonymous(r, _, _) => r.status().as_u16(),
            #[cfg(feature = "authenticated-http")]
            Self::Authenticated(r) => r.inner.status().as_u16(),
        }
    }
    pub(crate) fn headers(&self) -> &HeaderMap {
        match self {
            Self::Anonymous(r, _, _) => r.headers(),
            #[cfg(feature = "authenticated-http")]
            Self::Authenticated(r) => r.inner.headers(),
        }
    }
    pub(crate) fn read_body(&mut self, cap: u64, body: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Anonymous(r, trace, _) => {
                let before = body.len();
                let result = r.take(cap + 1).read_to_end(body);
                if let Some(t) = trace {
                    t.body(body.len() - before);
                }
                result.map_err(|_| "retryable interrupted HTTP body")?;
            }
            #[cfg(feature = "authenticated-http")]
            Self::Authenticated(r) => r.read_body(cap, body)?,
        }
        if body.len() as u64 > cap {
            return Err("HTTP response body exceeds requested bounds".into());
        }
        Ok(())
    }
}
