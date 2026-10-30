//! Machine-readable, closed diagnostics for the public CLI.
//!
//! Classification uses concrete error types and their source chain, never display
//! text. Messages/details supplied to `Diagnostic` must be safe, closed labels;
//! arbitrary underlying errors belong only in `with_cause`. Unknown errors use a
//! closed fallback instead of serializing their display text or arguments.
use serde::Serialize;
use serde_json::Value;
use std::{error::Error, fmt};

type Cause = Box<dyn Error + Send + Sync>;

pub struct Diagnostic {
    code: &'static str,
    stage: &'static str,
    message: String,
    retryable: Option<bool>,
    http_status: Option<u16>,
    source_reference: Option<String>,
    details: Option<Value>,
    cause: Option<Cause>,
}

impl Diagnostic {
    pub fn new(code: &'static str, stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            stage,
            message: message.into(),
            retryable: None,
            http_status: None,
            source_reference: None,
            details: None,
            cause: None,
        }
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    pub fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    /// Retain an HTTP source without query, fragment, userinfo or opaque input.
    pub fn with_source(mut self, source: &str) -> Self {
        self.source_reference = Some(sanitized_source(source));
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Retain an internal cause until a sanitizing boundary. Causes are never
    /// serialized, and `context` detaches them before exposing its source chain.
    pub fn with_cause(mut self, cause: Cause) -> Self {
        self.cause = Some(cause);
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl fmt::Debug for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Diagnostic")
            .field("code", &self.code)
            .field("stage", &self.stage)
            .field("message", &self.message)
            .field("source_reference", &self.source_reference)
            .finish_non_exhaustive()
    }
}

impl Error for Diagnostic {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_ref().map(|cause| cause.as_ref() as _)
    }
}

fn sanitized_source(source: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(source) else {
        return "[source]".into();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return "[source]".into();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.into()
}

/// A safe Display context retaining only detached typed metadata, never a raw
/// error/cause from before the sanitizing boundary.
struct ContextError {
    stage: String,
    message: String,
    diagnostic: Option<Diagnostic>,
}

impl fmt::Display for ContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.message)
    }
}

impl fmt::Debug for ContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextError")
            .field("stage", &self.stage)
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}

impl Error for ContextError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.diagnostic.as_ref().map(|diagnostic| diagnostic as _)
    }
}

pub(crate) fn context(stage: &str, safe_message: String, cause: Cause) -> Cause {
    let diagnostic = diagnostic_record(cause.as_ref()).map(|record| Diagnostic {
        code: record.code,
        stage: record.stage,
        // The original typed Display may also have contained a locator. Keep
        // exactly the text which passed the existing redaction boundary.
        message: safe_message.clone(),
        retryable: record.retryable,
        http_status: record.http_status,
        source_reference: record.source,
        details: record.details,
        cause: None,
    });
    Box::new(ContextError {
        stage: stage.into(),
        message: safe_message,
        diagnostic,
    })
}

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub schema_version: u32,
    pub document_type: &'static str,
    pub error: ErrorRecord,
}

#[derive(Debug, Serialize)]
pub struct ErrorRecord {
    pub code: &'static str,
    pub stage: &'static str,
    pub message: String,
    pub retryable: Option<bool>,
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Render the first typed diagnostic in the source chain. The fallback stage is
/// `cli`: no narrower stage or retry/status classification is inferred from text.
pub fn envelope(error: &(dyn Error + 'static)) -> ErrorEnvelope {
    let error = diagnostic_record(error).unwrap_or_else(|| ErrorRecord {
        code: "OPERATION_FAILED",
        stage: "cli",
        message: "Operation failed; no typed diagnostic is available.".into(),
        retryable: None,
        http_status: None,
        source: None,
        details: None,
    });
    ErrorEnvelope {
        schema_version: 1,
        document_type: "error",
        error,
    }
}

fn diagnostic_record(error: &(dyn Error + 'static)) -> Option<ErrorRecord> {
    let mut current = Some(error);
    let mut diagnostic = None;
    let mut contexts = Vec::new();
    while let Some(error) = current {
        if diagnostic.is_none() {
            diagnostic = error.downcast_ref::<Diagnostic>();
        }
        if let Some(context) = error.downcast_ref::<ContextError>() {
            contexts.push(context.stage.clone());
        }
        current = error.source();
    }
    diagnostic.map(|diagnostic| {
        let mut details = diagnostic.details.clone();
        if !contexts.is_empty() {
            let mut values = match details.take() {
                Some(Value::Object(values)) => values,
                Some(value) => serde_json::Map::from_iter([("diagnostic_details".into(), value)]),
                None => serde_json::Map::new(),
            };
            let mut stages: Vec<Value> = contexts.into_iter().map(Value::String).collect();
            if let Some(Value::Array(previous)) = values.remove("context_stages") {
                stages.extend(previous);
            }
            values.insert("context_stages".into(), Value::Array(stages));
            details = Some(Value::Object(values));
        }
        ErrorRecord {
            code: diagnostic.code,
            stage: diagnostic.stage,
            message: diagnostic.message.clone(),
            retryable: diagnostic.retryable,
            http_status: diagnostic.http_status,
            source: diagnostic.source_reference.clone(),
            details,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitizing_boundary_detaches_raw_causes_and_preserves_typed_identity() {
        let url = "https://user:password@example.test/a.tif?sig=SECRET#fragment";
        let request: crate::Request = serde_json::from_value(json!({
            "source": {"kind":"assets", "assets":[{
                "id":"red", "locator":url, "format":"cog"
            }]},
            "spatial":{"kind":"bounds", "crs":"EPSG:32618", "bbox":[0,0,10,10]},
            "variables":[{"kind":"band", "index":1}],
            "grid":{"kind":"native"}, "sink":{"kind":"callback"},
            "budget":{"buffer_bytes":16777216,"max_nodes":1000,"chunk_edge":64}
        }))
        .unwrap();
        let error = Diagnostic::new(
            "TEST_HTTP",
            "source_identity",
            format!("HEAD failed: {url}"),
        )
        .with_source(url)
        .with_http_status(403)
        .with_retryable(false)
        .with_cause("raw cause sig=SECRET password".into());
        let error = crate::acquisition::safe_error(&request, "prepare", error.into());
        let error = crate::acquisition::safe_error(&request, "execution", error);
        let value = serde_json::to_value(envelope(error.as_ref())).unwrap();
        assert_eq!(value["error"]["code"], "TEST_HTTP");
        assert_eq!(value["error"]["http_status"], 403);
        assert_eq!(value["error"]["source"], "https://example.test/a.tif");
        assert_eq!(
            value["error"]["details"]["context_stages"],
            json!(["execution", "prepare"])
        );
        assert!(!value.to_string().contains("SECRET"));
        assert!(!value.to_string().contains("password"));
        let mut current: Option<&(dyn Error + 'static)> = Some(error.as_ref());
        let mut exposed = 0;
        while let Some(error) = current {
            let rendered = format!("{error} {error:?}");
            assert!(!rendered.contains("SECRET"), "{rendered}");
            assert!(!rendered.contains("password"), "{rendered}");
            exposed += 1;
            current = error.source();
        }
        assert_eq!(
            exposed, 2,
            "only safe context and detached metadata are exposed"
        );
    }

    #[test]
    fn unknown_errors_have_no_message_based_classification_or_secret_echo() {
        let error: Cause = "HTTP 403 https://user:password@example.test/?sig=SECRET".into();
        let value = serde_json::to_value(envelope(error.as_ref())).unwrap();
        assert_eq!(value["error"]["code"], "OPERATION_FAILED");
        assert!(value["error"]["retryable"].is_null());
        assert!(value["error"]["http_status"].is_null());
        assert!(!value.to_string().contains("SECRET"));
        assert!(!value.to_string().contains("password"));
        let safe = context("prepare", "closed failure".into(), error);
        assert!(safe.source().is_none());
        let rendered = format!("{safe} {safe:?}");
        assert!(!rendered.contains("SECRET"));
        assert!(!rendered.contains("password"));
    }
}
