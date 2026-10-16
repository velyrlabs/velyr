//! Hook for HTTP server metrics: the runtime installs one sink for the process and the server
//! reports every answered request to it. io-http stays free of any metrics backend.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Receives one call per request the server answered.
pub trait HttpMetrics: Send + Sync {
    /// `elapsed` runs from receiving the request head to producing the response head; time spent
    /// streaming the body is not included.
    fn request_finished(&self, status: u16, elapsed: Duration);
}

static SINK: OnceLock<Arc<dyn HttpMetrics>> = OnceLock::new();

/// Installs the process-wide sink; false when one is already installed (the first one stays).
pub fn set_http_metrics(sink: Arc<dyn HttpMetrics>) -> bool {
    SINK.set(sink).is_ok()
}

pub(crate) fn record(status: u16, elapsed: Duration) {
    if let Some(sink) = SINK.get() {
        sink.request_finished(status, elapsed);
    }
}
