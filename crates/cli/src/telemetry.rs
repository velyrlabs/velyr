//! Tracing subscriber setup. This is the only place a subscriber is installed.
//!
//! Filtering comes from the `VELYR_LOG` env var (`EnvFilter` syntax, default `warn`).
//! Output goes to stderr so stdout stays reserved for program output.
//!
//! Span naming convention: `velyr.<crate>.<op>`, for example `velyr.runtime.eval`
//! or `velyr.cli.run`. Env values and secrets must never be recorded as fields.

use clap::ValueEnum;
use serde::Deserialize;
use tracing::Level;
#[cfg(feature = "telemetry")]
use tracing::level_filters::LevelFilter;
#[cfg(feature = "telemetry")]
use velyr_runtime_telemetry::{Point, Series};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use velyr_runtime_core::metrics::{self, MetricsLayer};

const LOG_ENV_VAR: &str = "VELYR_LOG";
const DEFAULT_FILTER: &str = "warn";
#[cfg(feature = "telemetry")]
const DEFAULT_SERVICE_NAME: &str = "velyr";

/// `telemetry` of `velyr.json`. `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_SERVICE_NAME` win.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TelemetryConfig {
    /// Base URL of an OTLP/HTTP collector, e.g. `http://localhost:4318`.
    pub endpoint: Option<String>,
    pub service_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

pub fn init(format: LogFormat) {
    let filter =
        EnvFilter::try_from_env(LOG_ENV_VAR).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let fmt = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let fmt = match format {
        LogFormat::Pretty => fmt.pretty().with_filter(filter).boxed(),
        LogFormat::Json => fmt.json().with_filter(filter).boxed(),
    };
    // Independent of VELYR_LOG: denial counters must not depend on what gets printed.
    let denials = Targets::new()
        .with_target("velyr.permission.denied", Level::WARN)
        .with_target("velyr.quota.denied", Level::WARN);
    #[cfg(feature = "telemetry")]
    start_exporter();
    let registry = tracing_subscriber::registry()
        .with(fmt)
        .with(MetricsLayer::new(metrics::global().clone()).with_filter(denials));
    #[cfg(feature = "telemetry")]
    let registry = registry
        .with(velyr_runtime_telemetry::tracing_layer().map(|layer| layer.with_filter(LevelFilter::INFO)));
    registry.init();
}

/// Flushes pending spans; call before the process ends.
pub fn shutdown() {
    #[cfg(feature = "telemetry")]
    velyr_runtime_telemetry::shutdown();
}

/// Starts the OTLP exporter when an endpoint is configured. Operator config only: it never
/// consults the permission model.
#[cfg(feature = "telemetry")]
fn start_exporter() {
    let from_env = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
    let file = || crate::run::load_config().ok().flatten().and_then(|loaded| loaded.config.telemetry);
    let (endpoint, service) = match from_env("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Some(endpoint) => (endpoint, from_env("OTEL_SERVICE_NAME")),
        None => match file() {
            Some(TelemetryConfig { endpoint: Some(endpoint), service_name }) => {
                (endpoint, from_env("OTEL_SERVICE_NAME").or(service_name))
            }
            _ => return,
        },
    };
    let service = service.unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned());
    if let Err(err) = velyr_runtime_telemetry::init_otlp(&endpoint, &service) {
        eprintln!("warning: telemetry disabled: {err}");
        return;
    }
    if let Err(err) =
        velyr_runtime_telemetry::init_otlp_metrics(&endpoint, &service, METRIC_SERIES, metric_points)
    {
        eprintln!("warning: metrics export disabled: {err}");
    }
}

#[cfg(feature = "telemetry")]
const fn gauge(name: &'static str, description: &'static str) -> Series {
    Series { name, description, monotonic: false }
}

#[cfg(feature = "telemetry")]
const fn counter(name: &'static str, description: &'static str) -> Series {
    Series { name, description, monotonic: true }
}

/// The Prometheus series (`Snapshot::to_prometheus`) that fit OTLP's gauges and counters; the
/// request duration histogram goes out as its sum and count only.
/// shortcut: no bucketed histogram over OTLP, add a custom reader when latency percentiles are needed.
#[cfg(feature = "telemetry")]
const METRIC_SERIES: &[Series] = &[
    gauge("velyr_process_resident_memory_bytes", "Resident set size of the process."),
    gauge("velyr_heap_used_bytes", "JS heap in use, as of the last finished turn."),
    gauge("velyr_heap_total_bytes", "JS heap committed."),
    gauge("velyr_event_loop_lag_seconds", "How long the running JS turn has held the loop."),
    gauge("velyr_pending_ops", "Host ops started and not completed."),
    gauge("velyr_resources_active", "Open resources per quota class."),
    counter("velyr_permission_denied_total", "Permission checks that denied."),
    counter("velyr_quota_denied_total", "Quota reservations that failed."),
    counter("velyr_http_requests_total", "HTTP requests answered by Velyr.serve, by status class."),
    counter("velyr_http_request_duration_seconds_sum", "Total time from request to response head."),
    counter("velyr_http_request_duration_seconds_count", "Requests timed."),
];

#[cfg(feature = "telemetry")]
fn point(name: &'static str, value: f64) -> Point {
    Point { name, attrs: Vec::new(), value }
}

#[cfg(feature = "telemetry")]
fn metric_points() -> Vec<Point> {
    snapshot_points(&metrics::global().snapshot())
}

#[cfg(feature = "telemetry")]
fn snapshot_points(snapshot: &metrics::Snapshot) -> Vec<Point> {
    let mut points = vec![
        point("velyr_process_resident_memory_bytes", snapshot.rss_bytes.unwrap_or(0) as f64),
        point("velyr_pending_ops", snapshot.pending_ops as f64),
        point("velyr_permission_denied_total", snapshot.permission_denied as f64),
        point("velyr_quota_denied_total", snapshot.quota_denied as f64),
        point("velyr_http_request_duration_seconds_sum", snapshot.http.sum_seconds),
        point("velyr_http_request_duration_seconds_count", snapshot.http.requests() as f64),
    ];
    // One series per server worker when there are any, else the single isolate's.
    let isolates: Vec<(Option<usize>, Option<velyr_engine_api::HeapStats>, f64)> = if snapshot.workers.is_empty() {
        vec![(None, snapshot.heap, snapshot.loop_lag_seconds)]
    } else {
        snapshot.workers.iter().map(|w| (Some(w.id), w.heap, w.loop_lag_seconds)).collect()
    };
    for (id, heap, lag) in isolates {
        let attrs = id.map(|id| ("worker", id.to_string())).into_iter().collect::<Vec<_>>();
        let with_attrs = |name, value| Point { attrs: attrs.clone(), ..point(name, value) };
        points.push(with_attrs("velyr_event_loop_lag_seconds", lag));
        if let Some(heap) = heap {
            points.push(with_attrs("velyr_heap_used_bytes", heap.used_bytes as f64));
            points.push(with_attrs("velyr_heap_total_bytes", heap.total_bytes as f64));
        }
    }
    for (class, used) in &snapshot.resources {
        points.push(Point { attrs: vec![("class", (*class).to_owned())], ..point("velyr_resources_active", *used as f64) });
    }
    for (index, count) in snapshot.http.by_status.iter().enumerate() {
        let status = format!("{}xx", index + 1);
        points.push(Point { attrs: vec![("status", status)], ..point("velyr_http_requests_total", *count as f64) });
    }
    points
}
