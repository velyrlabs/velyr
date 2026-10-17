//! OpenTelemetry export for the runtime (VELYR-121).
//!
//! The exporter is operator-only: it is configured from the environment or `velyr.json` and never
//! goes through the permission model of user code. Until [`init_otlp`] (or [`install`]) runs,
//! every hook here is a single atomic load.
//!
//! Spans: a server span per `Velyr.serve` request (parent from the incoming `traceparent`) and a
//! client span per outgoing `fetch` (with `traceparent` injected into the request headers).

mod metrics;

pub use metrics::{Point, Series, init_otlp_metrics, install_metrics};

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

const TRACER_NAME: &str = "velyr";
const METER_NAME: &str = "velyr";
const TRACES_PATH: &str = "/v1/traces";

static ENABLED: AtomicBool = AtomicBool::new(false);
static PROVIDER: Mutex<Option<SdkTracerProvider>> = Mutex::new(None);

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("cannot build the OTLP exporter: {0}")]
    Exporter(String),
}

/// Starts exporting to the OTLP/HTTP collector at `endpoint` (a base URL such as
/// `http://localhost:4318` or an `https://` collector; `/v1/traces` is appended).
pub fn init_otlp(endpoint: &str, service_name: &str) -> Result<(), TelemetryError> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(signal_url(endpoint, TRACES_PATH))
        .build()
        .map_err(|e| TelemetryError::Exporter(e.to_string()))?;
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name(service_name.to_owned())
                .build(),
        )
        .build();
    install(provider);
    Ok(())
}

/// Makes `provider` the one every hook reports to. For `init_otlp` and for tests that bring
/// their own (in-memory) exporter.
pub fn install(provider: SdkTracerProvider) {
    global::set_tracer_provider(provider.clone());
    *PROVIDER.lock().expect("provider slot") = Some(provider);
    ENABLED.store(true, Ordering::SeqCst);
}

/// Flushes pending spans and stops the exporter. Safe to call when telemetry was never started.
pub fn shutdown() {
    ENABLED.store(false, Ordering::SeqCst);
    if let Some(provider) = PROVIDER.lock().expect("provider slot").take() {
        let _ = provider.shutdown();
    }
    if let Some(provider) = metrics::METER_PROVIDER
        .lock()
        .expect("meter provider slot")
        .take()
    {
        let _ = provider.shutdown();
    }
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// A `tracing` layer that exports `tracing` spans too; `None` until telemetry is installed.
pub fn tracing_layer<S>() -> Option<OpenTelemetryLayer<S, SdkTracer>>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    use opentelemetry::trace::TracerProvider;
    let provider = PROVIDER.lock().expect("provider slot").clone()?;
    Some(tracing_opentelemetry::layer().with_tracer(provider.tracer(TRACER_NAME)))
}

/// `base` plus the signal `path`, unless `base` already ends with it.
fn signal_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with(path) {
        base.to_owned()
    } else {
        format!("{base}{path}")
    }
}

/// An open span; ends on [`finish`](Self::finish), [`fail`](Self::fail) or drop. A clone is another
/// handle on the same span (for parenting); ending either one ends it.
#[derive(Clone)]
pub struct ActiveSpan(Context);

impl ActiveSpan {
    /// Ends the span with the HTTP status. `error_from` is the lowest status counted as an
    /// error (500 for servers, 400 for clients).
    pub fn finish(self, status: u16, error_from: u16) {
        self.record_status(status, error_from);
        self.end();
    }

    /// Records the HTTP status without ending the span (a streamed body is still going out).
    pub fn record_status(&self, status: u16, error_from: u16) {
        let span = self.0.span();
        span.set_attribute(KeyValue::new(
            "http.response.status_code",
            i64::from(status),
        ));
        if status >= error_from {
            span.set_status(Status::error(""));
        }
    }

    pub fn end(self) {
        self.0.span().end();
    }

    pub fn fail(self, message: &str) {
        let span = self.0.span();
        span.set_status(Status::error(message.to_owned()));
        span.end();
    }
}

/// Server span for one incoming request, parented by the request's `traceparent` if it has one.
pub fn server_span(method: &str, target: &str, headers: &[(String, String)]) -> Option<ActiveSpan> {
    if !is_enabled() {
        return None;
    }
    let parent = TraceContextPropagator::new().extract(&HeaderList(headers));
    let attributes = vec![
        KeyValue::new("http.request.method", method.to_owned()),
        KeyValue::new(
            "url.path",
            target.split(['?', '#']).next().unwrap_or("").to_owned(),
        ),
    ];
    Some(start(method, SpanKind::Server, attributes, &parent))
}

/// Client span for one outgoing request, a child of `parent` when given; injects `traceparent`
/// into `headers`.
pub fn client_span(
    method: &str,
    url: &str,
    headers: &mut Vec<(String, String)>,
    parent: Option<&ActiveSpan>,
) -> Option<ActiveSpan> {
    if !is_enabled() {
        return None;
    }
    let attributes = vec![
        KeyValue::new("http.request.method", method.to_owned()),
        KeyValue::new("url.full", redact_url(url)),
    ];
    let parent = parent.map_or_else(Context::current, |span| span.0.clone());
    let span = start(method, SpanKind::Client, attributes, &parent);
    TraceContextPropagator::new().inject_context(&span.0, &mut HeaderInjector(headers));
    Some(span)
}

/// Internal span for script code (`Velyr.telemetry.span`), a child of `parent` when given.
pub fn user_span(name: &str, parent: Option<&ActiveSpan>) -> Option<ActiveSpan> {
    if !is_enabled() {
        return None;
    }
    let parent = parent.map_or_else(Context::new, |span| span.0.clone());
    Some(start(name, SpanKind::Internal, Vec::new(), &parent))
}

fn start(name: &str, kind: SpanKind, attributes: Vec<KeyValue>, parent: &Context) -> ActiveSpan {
    let tracer = global::tracer(TRACER_NAME);
    let span = tracer
        .span_builder(name.to_owned())
        .with_kind(kind)
        .with_attributes(attributes)
        .start_with_context(&tracer, parent);
    ActiveSpan(parent.with_span(span))
}

/// Drops credentials, query and fragment: they routinely carry secrets.
fn redact_url(url: &str) -> String {
    let url = url.split(['?', '#']).next().unwrap_or("");
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = authority.rsplit('@').next().unwrap_or("");
            format!("{scheme}://{host}{path}")
        }
        None => url.to_owned(),
    }
}

struct HeaderList<'a>(&'a [(String, String)]);

impl Extractor for HeaderList<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.iter().map(|(name, _)| name.as_str()).collect()
    }
}

struct HeaderInjector<'a>(&'a mut Vec<(String, String)>);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.retain(|(name, _)| !name.eq_ignore_ascii_case(key));
        self.0.push((key.to_owned(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials_and_query() {
        assert_eq!(
            redact_url("https://u:p@host:8/a/b?token=1#x"),
            "https://host:8/a/b"
        );
        assert_eq!(redact_url("http://host"), "http://host");
    }

    #[test]
    fn appends_traces_path_once() {
        assert_eq!(
            signal_url("http://c:4318/", TRACES_PATH),
            "http://c:4318/v1/traces"
        );
        assert_eq!(
            signal_url("http://c:4318/v1/traces", TRACES_PATH),
            "http://c:4318/v1/traces"
        );
    }
}
