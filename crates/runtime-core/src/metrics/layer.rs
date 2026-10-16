//! Counts permission and quota denials from the structured events the runtime already emits
//! (`velyr.permission.denied`, `velyr.quota.denied`), so the deciding code needs no metrics
//! dependency. Attach it with a `Targets` filter so other events stay uninteresting.

use std::sync::Arc;

use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

use super::Metrics;

pub struct MetricsLayer {
    metrics: Arc<Metrics>,
}

impl MetricsLayer {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self { metrics }
    }
}

impl<S: Subscriber> Layer<S> for MetricsLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        match event.metadata().target() {
            "velyr.permission.denied" => self.metrics.count_permission_denied(),
            "velyr.quota.denied" => self.metrics.count_quota_denied(),
            _ => {}
        }
    }
}
