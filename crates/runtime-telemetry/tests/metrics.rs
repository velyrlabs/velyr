use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use velyr_runtime_telemetry as telemetry;
use velyr_runtime_telemetry::{Point, Series};

const SERIES: &[Series] = &[
    Series { name: "velyr_heap_used_bytes", description: "heap", monotonic: false },
    Series { name: "velyr_http_requests_total", description: "requests", monotonic: true },
];

fn points() -> Vec<Point> {
    vec![
        Point { name: "velyr_heap_used_bytes", attrs: vec![], value: 2048.0 },
        Point { name: "velyr_http_requests_total", attrs: vec![("status", "2xx".to_owned())], value: 7.0 },
        Point { name: "velyr_http_requests_total", attrs: vec![("status", "5xx".to_owned())], value: 1.0 },
    ]
}

/// (name, attributes, value) of every data point exported.
fn exported(exporter: &InMemoryMetricExporter) -> Vec<(String, Vec<KeyValue>, f64)> {
    let mut out = Vec::new();
    for resource in exporter.get_finished_metrics().unwrap() {
        for scope in resource.scope_metrics() {
            for metric in scope.metrics() {
                let AggregatedMetrics::F64(data) = metric.data() else { continue };
                let rows: Vec<_> = match data {
                    MetricData::Gauge(g) => g.data_points().map(|p| (p.attributes().cloned().collect(), p.value())).collect(),
                    MetricData::Sum(s) => s.data_points().map(|p| (p.attributes().cloned().collect(), p.value())).collect(),
                    MetricData::Histogram(_) | MetricData::ExponentialHistogram(_) => continue,
                };
                out.extend(rows.into_iter().map(|(attrs, value)| (metric.name().to_owned(), attrs, value)));
            }
        }
    }
    out
}

#[test]
fn observed_points_become_gauges_and_counters() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    telemetry::install_metrics(provider.clone(), SERIES, points);
    provider.force_flush().unwrap();

    let rows = exported(&exporter);
    let heap = rows.iter().find(|(name, ..)| name == "velyr_heap_used_bytes").expect("gauge");
    assert_eq!(heap.2, 2048.0);
    let five_xx = rows
        .iter()
        .find(|(name, attrs, _)| name == "velyr_http_requests_total" && attrs.iter().any(|kv| kv.value.as_str() == "5xx"))
        .expect("counter series by status");
    assert_eq!(five_xx.2, 1.0);
    assert_eq!(rows.iter().filter(|(name, ..)| name == "velyr_http_requests_total").count(), 2);
}
