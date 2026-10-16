//! Prometheus text exposition and the JSON shape behind `Velyr.metrics()`.

use std::fmt::Write;

use serde_json::{Value, json};

use velyr_engine_api::HeapStats;

use super::{HTTP_BUCKETS_SECONDS, STATUS_CLASSES, Snapshot};

/// One series of the per-isolate gauges.
struct Worker {
    id: Option<usize>,
    heap: Option<HeapStats>,
    lag: f64,
}

impl Worker {
    fn label(&self) -> String {
        self.id
            .map_or_else(String::new, |id| format!("{{worker=\"{id}\"}}"))
    }
}

fn metric(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = write!(out, "# HELP {name} {help}\n# TYPE {name} {kind}\n");
}

impl Snapshot {
    /// Prometheus text format (version 0.0.4).
    pub fn to_prometheus(&self) -> String {
        let mut out = String::new();
        metric(
            &mut out,
            "velyr_process_resident_memory_bytes",
            "gauge",
            "Resident set size of the process.",
        );
        let _ = writeln!(
            out,
            "velyr_process_resident_memory_bytes {}",
            self.rss_bytes.unwrap_or(0)
        );
        // One series per server worker when there are any, else the single isolate's.
        let single = Worker {
            id: None,
            heap: self.heap,
            lag: self.loop_lag_seconds,
        };
        let isolates: Vec<Worker> = if self.workers.is_empty() {
            vec![single]
        } else {
            self.workers
                .iter()
                .map(|worker| Worker {
                    id: Some(worker.id),
                    heap: worker.heap,
                    lag: worker.loop_lag_seconds,
                })
                .collect()
        };
        let heaps =
            |name: &str, help: &str, read: fn(HeapStats) -> Option<usize>, out: &mut String| {
                let rows: Vec<_> = isolates
                    .iter()
                    .filter_map(|w| Some((w.label(), read(w.heap?)?)))
                    .collect();
                if !rows.is_empty() {
                    metric(out, name, "gauge", help);
                    for (label, value) in rows {
                        let _ = writeln!(out, "{name}{label} {value}");
                    }
                }
            };
        heaps(
            "velyr_heap_used_bytes",
            "JS heap in use, as of the last finished turn.",
            |heap| Some(heap.used_bytes),
            &mut out,
        );
        heaps(
            "velyr_heap_total_bytes",
            "JS heap committed.",
            |heap| Some(heap.total_bytes),
            &mut out,
        );
        heaps(
            "velyr_heap_limit_bytes",
            "Configured JS heap limit.",
            |heap| heap.limit_bytes,
            &mut out,
        );
        metric(
            &mut out,
            "velyr_event_loop_lag_seconds",
            "gauge",
            "How long the running JS turn has held the loop (last turn when idle).",
        );
        for w in &isolates {
            let _ = writeln!(out, "velyr_event_loop_lag_seconds{} {}", w.label(), w.lag);
        }
        metric(
            &mut out,
            "velyr_pending_ops",
            "gauge",
            "Host ops started and not completed.",
        );
        let _ = writeln!(out, "velyr_pending_ops {}", self.pending_ops);
        metric(
            &mut out,
            "velyr_resources_active",
            "gauge",
            "Open resources per quota class.",
        );
        for (class, used) in &self.resources {
            let _ = writeln!(out, "velyr_resources_active{{class=\"{class}\"}} {used}");
        }
        metric(
            &mut out,
            "velyr_permission_denied_total",
            "counter",
            "Permission checks that denied.",
        );
        let _ = writeln!(
            out,
            "velyr_permission_denied_total {}",
            self.permission_denied
        );
        metric(
            &mut out,
            "velyr_quota_denied_total",
            "counter",
            "Quota reservations that failed.",
        );
        let _ = writeln!(out, "velyr_quota_denied_total {}", self.quota_denied);
        metric(
            &mut out,
            "velyr_http_requests_total",
            "counter",
            "HTTP requests answered by Velyr.serve, by status class.",
        );
        for (class, count) in STATUS_CLASSES.iter().zip(self.http.by_status) {
            let _ = writeln!(
                out,
                "velyr_http_requests_total{{status=\"{class}\"}} {count}"
            );
        }
        metric(
            &mut out,
            "velyr_http_request_duration_seconds",
            "histogram",
            "Time from receiving a request to its response head.",
        );
        for (bound, count) in HTTP_BUCKETS_SECONDS.iter().zip(&self.http.buckets) {
            let _ = writeln!(
                out,
                "velyr_http_request_duration_seconds_bucket{{le=\"{bound}\"}} {count}"
            );
        }
        let total = self.http.buckets.last().copied().unwrap_or(0);
        let _ = writeln!(
            out,
            "velyr_http_request_duration_seconds_bucket{{le=\"+Inf\"}} {total}"
        );
        let _ = writeln!(
            out,
            "velyr_http_request_duration_seconds_sum {}",
            self.http.sum_seconds
        );
        let _ = writeln!(out, "velyr_http_request_duration_seconds_count {total}");
        out
    }

    /// The object `Velyr.metrics()` returns.
    pub fn to_json(&self) -> Value {
        let heap = self.heap.map(|heap| {
            json!({"usedBytes": heap.used_bytes, "totalBytes": heap.total_bytes, "limitBytes": heap.limit_bytes})
        });
        let resources: serde_json::Map<String, Value> = self
            .resources
            .iter()
            .map(|(class, used)| ((*class).to_owned(), json!(used)))
            .collect();
        let by_status: serde_json::Map<String, Value> = STATUS_CLASSES
            .iter()
            .zip(self.http.by_status)
            .map(|(class, count)| ((*class).to_owned(), json!(count)))
            .collect();
        let buckets: Vec<Value> = self
            .http
            .buckets
            .iter()
            .enumerate()
            .map(|(i, count)| json!({"le": HTTP_BUCKETS_SECONDS.get(i), "count": count}))
            .collect();
        let workers: Vec<Value> = self
            .workers
            .iter()
            .map(|worker| {
                json!({
                    "id": worker.id,
                    "loopLagSeconds": worker.loop_lag_seconds,
                    "heap": worker.heap.map(|heap| json!({"usedBytes": heap.used_bytes, "totalBytes": heap.total_bytes, "limitBytes": heap.limit_bytes})),
                })
            })
            .collect();
        json!({
            "rssBytes": self.rss_bytes,
            "heap": heap,
            "loopLagSeconds": self.loop_lag_seconds,
            "pendingOps": self.pending_ops,
            "resources": resources,
            "workers": workers,
            "permissionDenied": self.permission_denied,
            "quotaDenied": self.quota_denied,
            "http": {
                "requests": self.http.requests(),
                "byStatus": by_status,
                "buckets": buckets,
                "sumSeconds": self.http.sum_seconds,
            },
        })
    }
}
