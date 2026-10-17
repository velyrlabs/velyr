use std::sync::OnceLock;

use opentelemetry::trace::{SpanKind, Status};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use velyr_runtime_telemetry as telemetry;

const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const PARENT_ID: &str = "00f067aa0ba902b7";

fn exporter() -> &'static InMemorySpanExporter {
    static EXPORTER: OnceLock<InMemorySpanExporter> = OnceLock::new();
    EXPORTER.get_or_init(|| {
        let exporter = InMemorySpanExporter::default();
        telemetry::install(SdkTracerProvider::builder().with_simple_exporter(exporter.clone()).build());
        exporter
    })
}

fn finished(path: &str) -> SpanData {
    exporter()
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .find(|span| span.attributes.iter().any(|kv| kv.value.as_str() == path))
        .expect("span was exported")
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    &headers.iter().find(|(n, _)| n == name).expect("header present").1
}

#[test]
fn server_span_takes_its_parent_from_traceparent() {
    exporter();
    let headers = vec![("Traceparent".to_owned(), format!("00-{TRACE_ID}-{PARENT_ID}-01"))];

    let span = telemetry::server_span("GET", "/orders?id=1", &headers).unwrap();
    span.finish(200, 500);

    let data = finished("/orders");
    assert_eq!(data.span_kind, SpanKind::Server);
    assert_eq!(data.span_context.trace_id().to_string(), TRACE_ID);
    assert_eq!(data.parent_span_id.to_string(), PARENT_ID);
    assert_eq!(data.status, Status::Unset);
}

#[test]
fn server_span_without_traceparent_is_a_root() {
    exporter();
    telemetry::server_span("POST", "/root", &[]).unwrap().finish(503, 500);

    let data = finished("/root");
    assert_eq!(data.parent_span_id, opentelemetry::trace::SpanId::INVALID);
    assert!(matches!(data.status, Status::Error { .. }));
}

#[test]
fn client_span_injects_traceparent() {
    exporter();
    let mut headers = vec![("accept".to_owned(), "*/*".to_owned())];

    let span = telemetry::client_span("GET", "https://user:pw@api.test/items?key=secret", &mut headers, None).unwrap();
    span.finish(404, 400);

    let data = finished("https://api.test/items");
    assert_eq!(data.span_kind, SpanKind::Client);
    let traceparent = header(&headers, "traceparent");
    assert!(traceparent.contains(&data.span_context.trace_id().to_string()));
    assert!(traceparent.contains(&data.span_context.span_id().to_string()));
    assert!(matches!(data.status, Status::Error { .. }));
}

fn named(name: &str) -> SpanData {
    exporter()
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .find(|span| span.name == name)
        .expect("span was exported")
}

#[test]
fn user_span_is_internal_and_nests_under_its_parent() {
    exporter();
    let outer = telemetry::user_span("outer-job", None).unwrap();
    let inner = telemetry::user_span("inner-job", Some(&outer)).unwrap();
    inner.fail("boom");
    outer.end();

    let (outer, inner) = (named("outer-job"), named("inner-job"));
    assert_eq!(outer.span_kind, SpanKind::Internal);
    assert_eq!(outer.parent_span_id, opentelemetry::trace::SpanId::INVALID);
    assert_eq!(inner.parent_span_id, outer.span_context.span_id());
    assert_eq!(inner.span_context.trace_id(), outer.span_context.trace_id());
    assert!(matches!(inner.status, Status::Error { .. }));
}

#[test]
fn client_span_nests_under_its_parent() {
    exporter();
    let parent = telemetry::user_span("fetching-parent", None).unwrap();
    let mut headers = Vec::new();
    telemetry::client_span("GET", "https://child.test/", &mut headers, Some(&parent)).unwrap().finish(200, 400);
    parent.end();

    let (parent, child) = (named("fetching-parent"), finished("https://child.test/"));
    assert_eq!(child.parent_span_id, parent.span_context.span_id());
}
