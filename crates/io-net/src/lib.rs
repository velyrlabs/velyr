//! TCP host service (ADR-010): the `op_net_*` ops behind `Velyr.connect` and `Velyr.listen`.
//!
//! Connect is authorized in two phases: the hostname before DNS (`connect_begin`) and every
//! resolved `SocketAddr` before it is dialed (`connect_finish`). Sockets and listeners live in the
//! isolate's `ResourceTable`, so their quotas (`sockets`, `listeners`) and the leak report apply.
//! The table is only reachable on the isolate thread, hence the split ops: connect inserts a
//! placeholder that the async half fills, and accept parks the new stream in the listener until
//! `accept_take` turns it into a resource.

mod conn;
mod ops;

// Building blocks for layered protocols (io-tls); additive, the JS API is unchanged.
pub use conn::{ConnInner, ConnRes, Duplex, TcpIo, addr_json, conn_info, dial, net_error};
pub use ops::{begin_connect, conn_by_id, insert_upgrading, resolver_of, take_conn};

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use velyr_engine_api::{HostFunctionDef, Isolate, JsException, Source};
use velyr_runtime_ops::CapabilityKind;

/// Bootstrap script: adds `Velyr.connect` and `Velyr.listen` on top of the private `op_net_*` bindings.
pub const NET_JS: &str = include_str!("net.js");

pub type ResolveFuture = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;

/// Maps `(hostname, port)` to candidate addresses. The default is `tokio::net::lookup_host`;
/// tests inject their own to simulate DNS answers.
pub type Resolver = Arc<dyn Fn(String, u16) -> ResolveFuture + Send + Sync>;

pub(crate) struct NetState {
    pub resolver: Resolver,
}

/// The default resolver: `tokio::net::lookup_host`.
pub fn system_resolver() -> Resolver {
    Arc::new(|host, port| {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), port)).await?;
            Ok(addrs.collect())
        })
    })
}

/// The net ops with the capability each declares, for the registry audit.
pub fn net_ops() -> Vec<(HostFunctionDef, CapabilityKind)> {
    ops::OPS.to_vec()
}

pub fn net_host_functions() -> Vec<HostFunctionDef> {
    net_ops().into_iter().map(|(def, _)| def).collect()
}

/// Runs [`NET_JS`] with the system resolver. Call after `install_op_context` and after
/// `install_web_streams` (`Conn.readable`/`writable` use its native stream hooks).
pub fn install_net(isolate: &mut impl Isolate) -> Result<(), JsException> {
    install_net_with_resolver(isolate, system_resolver())
}

pub fn install_net_with_resolver(
    isolate: &mut impl Isolate,
    resolver: Resolver,
) -> Result<(), JsException> {
    isolate.set_slot(NetState { resolver });
    isolate
        .eval_script(&Source {
            name: "velyr:net".into(),
            text: NET_JS.into(),
        })
        .map(drop)
}
