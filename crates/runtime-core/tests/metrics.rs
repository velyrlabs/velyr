//! Runtime metrics: counters fed by tracing events, loop lag from the isolate wrapper, heap
//! sampling, the Prometheus text and `Velyr.metrics()`.

use std::sync::Arc;
use std::time::Duration;

use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::metrics::{self, Metrics, MetricsLayer};
use velyr_runtime_core::{RuntimeConfig, run_file};
use velyr_runtime_resources::QuotaContext;

#[test]
fn permission_and_quota_denials_are_counted_from_tracing_events() {
    let metrics = Arc::new(Metrics::new());
    let layer = MetricsLayer::new(metrics.clone())
        .with_filter(Targets::new().with_target("velyr", tracing::Level::WARN));
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!(target: "velyr.permission.denied", "velyr.permission.denied");
        tracing::warn!(target: "velyr.permission.denied", "velyr.permission.denied");
        tracing::warn!(target: "velyr.quota.denied", class = "cpu", "velyr.quota.denied");
        tracing::warn!(target: "velyr.other", "ignored");
    });
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.permission_denied, 2);
    assert_eq!(snapshot.quota_denied, 1);
}

#[test]
fn loop_lag_follows_the_running_turn_and_keeps_the_last_one() {
    let metrics = Metrics::new();
    assert_eq!(metrics.snapshot().loop_lag_seconds, 0.0);
    metrics.turn_start();
    std::thread::sleep(Duration::from_millis(60));
    let during = metrics.snapshot().loop_lag_seconds;
    assert!(during >= 0.05, "lag while busy: {during}");
    metrics.turn_end();
    let after = metrics.snapshot().loop_lag_seconds;
    assert!(after >= 0.05, "lag after the turn: {after}");
    metrics.turn_start();
    metrics.turn_end();
    assert!(metrics.snapshot().loop_lag_seconds < 0.05);
}

#[test]
fn http_requests_fill_a_cumulative_histogram() {
    let metrics = Metrics::new();
    metrics.record_http_request(200, Duration::from_millis(3));
    metrics.record_http_request(404, Duration::from_millis(30));
    metrics.record_http_request(503, Duration::from_secs(60));
    let text = metrics.snapshot().to_prometheus();
    for expected in [
        "velyr_http_requests_total{status=\"2xx\"} 1",
        "velyr_http_requests_total{status=\"4xx\"} 1",
        "velyr_http_requests_total{status=\"5xx\"} 1",
        "velyr_http_request_duration_seconds_bucket{le=\"0.005\"} 1",
        "velyr_http_request_duration_seconds_bucket{le=\"0.05\"} 2",
        "velyr_http_request_duration_seconds_bucket{le=\"+Inf\"} 3",
        "velyr_http_request_duration_seconds_count 3",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}

#[test]
fn prometheus_text_has_rss_lag_and_counters() {
    let text = Metrics::new().snapshot().to_prometheus();
    for name in [
        "velyr_process_resident_memory_bytes",
        "velyr_event_loop_lag_seconds",
        "velyr_pending_ops",
        "velyr_permission_denied_total",
        "velyr_quota_denied_total",
    ] {
        assert!(
            text.contains(&format!("\n{name} ")) || text.starts_with(name),
            "{name} in:\n{text}"
        );
        assert!(text.contains(&format!("# TYPE {name} ")), "TYPE of {name}");
    }
}

#[test]
fn a_run_feeds_heap_turns_and_velyr_metrics() {
    let path = std::env::temp_dir().join(format!("velyr-metrics-{}.js", std::process::id()));
    std::fs::write(
        &path,
        r#"
        const m = Velyr.metrics();
        const need = (ok, what) => { if (!ok) throw new Error("bad snapshot: " + what); };
        need(typeof m.rssBytes === "number" && m.rssBytes > 0, "rssBytes");
        need(typeof m.loopLagSeconds === "number", "loopLagSeconds");
        need(typeof m.pendingOps === "number", "pendingOps");
        need(m.heap.usedBytes > 0 && m.heap.totalBytes >= m.heap.usedBytes, "heap");
        need(typeof m.resources.sockets === "number" && typeof m.resources.files === "number", "resources");
        need(typeof m.permissionDenied === "number" && typeof m.quotaDenied === "number", "denials");
        need(m.http.requests === 0 || m.http.requests > 0, "http");
        need(Array.isArray(m.http.buckets), "buckets");
        "#,
    )
    .unwrap();
    let result = run_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            ..RuntimeConfig::default()
        },
        metrics::metrics_host_functions(),
        "globalThis.Velyr = {};",
        |isolate, sender, _timers| {
            metrics::global().bind(sender, Arc::new(QuotaContext::default()));
            metrics::install_metrics(isolate).expect("valid JS");
        },
        path.clone(),
    );
    std::fs::remove_file(&path).unwrap();
    result.unwrap();
    let snapshot = metrics::global().snapshot();
    assert!(snapshot.heap.is_some_and(|heap| heap.used_bytes > 0));
}
