//! HTTP/3 prototype (VELYR-122), behind the `http3` feature: a quinn + h3 server that reuses the
//! io-http [`Handler`](crate::Handler) type, a connection-oriented client, QUIC TLS config helpers and
//! the `Alt-Svc` header value for the TCP side. Not wired into the CLI or web-fetch; see
//! `docs/research/http3.md` for the design, the numbers and the fallback plan.

mod client;
mod server;
mod tls;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use std::time::Duration;

pub use client::H3Connection;
pub use server::serve_h3;
pub use tls::{client_endpoint, server_endpoint};

/// Version string handlers see in `HttpRequestParts::version`.
pub(crate) const VERSION: &str = "HTTP/3.0";

/// Value of the `Alt-Svc` header a TCP (h1/h2) response carries to advertise this h3 server on the
/// same host and the given UDP `port`.
pub fn alt_svc_value(port: u16, max_age: Duration) -> String {
    format!(r#"h3=":{port}"; ma={}"#, max_age.as_secs())
}
