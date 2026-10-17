//! OTLP metrics export. The host owns the numbers (the same ones Prometheus serves); this module
//! only turns a list of [`Point`]s into observable gauges and counters on every collection.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};

use crate::{METER_NAME, TelemetryError, signal_url};

const METRICS_PATH: &str = "/v1/metrics";
/// OTLP default; `OTEL_METRIC_EXPORT_INTERVAL` is not read.
const EXPORT_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) static METER_PROVIDER: Mutex<Option<SdkMeterProvider>> = Mutex::new(None);

/// One metric the host reports. A counter (`monotonic`) is cumulative, a gauge is a reading.
#[derive(Debug, Clone, Copy)]
pub struct Series {
    pub name: &'static str,
    pub description: &'static str,
    pub monotonic: bool,
}

/// One reading of a [`Series`], with its attributes (a series can have several points).
#[derive(Debug, Clone)]
pub struct Point {
    pub name: &'static str,
    pub attrs: Vec<(&'static str, String)>,
    pub value: f64,
}

/// Starts exporting `series` to the OTLP/HTTP collector at `endpoint` (`/v1/metrics` is
/// appended) every minute; `source` is asked for the current points at each collection.
pub fn init_otlp_metrics(
    endpoint: &str,
    service_name: &str,
    series: &'static [Series],
    source: impl Fn() -> Vec<Point> + Send + Sync + 'static,
) -> Result<(), TelemetryError> {
    let exporter = MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(signal_url(endpoint, METRICS_PATH))
        .build()
        .map_err(|e| TelemetryError::Exporter(e.to_string()))?;
    let provider = SdkMeterProvider::builder()
        .with_reader(
            PeriodicReader::builder(exporter)
                .with_interval(EXPORT_INTERVAL)
                .build(),
        )
        .with_resource(
            Resource::builder()
                .with_service_name(service_name.to_owned())
                .build(),
        )
        .build();
    install_metrics(provider, series, source);
    Ok(())
}

/// Registers `series` on `provider`; also for tests that bring their own (in-memory) exporter.
/// `shutdown` flushes and stops it.
pub fn install_metrics(
    provider: SdkMeterProvider,
    series: &'static [Series],
    source: impl Fn() -> Vec<Point> + Send + Sync + 'static,
) {
    let meter = provider.meter(METER_NAME);
    let source = Arc::new(source);
    // shortcut: every instrument asks `source` again (a few dozen cheap reads per minute);
    // share one reading per collection if it ever gets expensive.
    for &Series {
        name,
        description,
        monotonic,
    } in series
    {
        let source = Arc::clone(&source);
        let observe = move |observe: &dyn Fn(f64, &[KeyValue])| {
            for point in source().iter().filter(|point| point.name == name) {
                let attrs: Vec<_> = point
                    .attrs
                    .iter()
                    .map(|(k, v)| KeyValue::new(*k, v.clone()))
                    .collect();
                observe(point.value, &attrs);
            }
        };
        if monotonic {
            meter
                .f64_observable_counter(name)
                .with_description(description)
                .with_callback(move |o| observe(&|value, attrs| o.observe(value, attrs)))
                .build();
        } else {
            meter
                .f64_observable_gauge(name)
                .with_description(description)
                .with_callback(move |o| observe(&|value, attrs| o.observe(value, attrs)))
                .build();
        }
    }
    *METER_PROVIDER.lock().expect("meter provider slot") = Some(provider);
}
