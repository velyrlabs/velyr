//! HTTP host layer (TDD section 10): a hyper 1.x server (h1/h2, auto-detected) and a pooled client whose
//! connector goes through io-net's two-phase connect authorization. Hyper types never leave this crate:
//! callers see [`HttpRequestParts`] / [`HttpResponseParts`] and bodies as bounded byte channels
//! ([`BodyReceiver`]) whose capacity is the backpressure. HTTP/3 is out of scope.

mod body;
mod client;
#[cfg(feature = "http3")]
pub mod h3;
mod metrics;
mod quota;
mod server;
mod server_binding;

pub use body::{BodyReceiver, BodySender, body_channel};
pub use client::{ClientOptions, HttpClient};
pub use metrics::{HttpMetrics, set_http_metrics};
pub use server::{
    Handler, serve_connection, serve_listener, serve_listener_graceful, serve_streams_graceful,
};
pub use server_binding::{Incoming, channel_handler};

use std::sync::Arc;
use std::time::Duration;

use velyr_runtime_resources::QuotaContext;

/// Limits shared by server and client.
#[derive(Clone)]
pub struct Limits {
    /// Largest request/response head accepted.
    pub max_header_bytes: usize,
    /// Largest body accepted; a larger one is a 413 (server, known length) or `QuotaExceeded`.
    pub max_body_bytes: u64,
    /// How long a peer may take to send a head (server) or answer with one (client).
    pub header_timeout: Duration,
    /// Idle connection timeout (h2 keep-alive; on h1 the header timeout also bounds idle keep-alive).
    pub idle_timeout: Duration,
    /// When set, body bytes in flight are reserved against the `body_bytes` quota.
    pub quotas: Option<Arc<QuotaContext>>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_header_bytes: 64 * 1024,
            max_body_bytes: 16 * 1024 * 1024,
            header_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            quotas: None,
        }
    }
}

/// A request as the host sees it. `uri` is the path and query on the server, the absolute URL on the client.
pub struct HttpRequestParts {
    /// Peer address (server side only).
    pub remote_addr: Option<std::net::SocketAddr>,
    pub method: String,
    pub uri: String,
    /// `HTTP/1.1`, `HTTP/2.0`, ... (filled by the server; ignored by the client).
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: BodyReceiver,
}

/// Receives the raw connection once a `101 Switching Protocols` response was sent (HTTP/1.1).
pub type UpgradeCallback = Box<dyn FnOnce(Box<dyn velyr_io_net::Duplex>) + Send>;

pub struct HttpResponseParts {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BodyReceiver,
    /// Server side: set together with status 101; the callback gets the connection after the
    /// response went out. Never set by the client.
    pub upgrade: Option<UpgradeCallback>,
}
