//! TLS host service (ADR-010): `Velyr.connectTls`, `Velyr.listenTls` and `Velyr.startTls`, built on
//! rustls (aws-lc-rs provider) and layered over the io-net socket resources, so the two-phase
//! connect authorization and the `sockets`/`listeners` quotas are the ones io-net enforces.
//! The client verifies with the platform verifier plus the optional `caCerts`; the server takes
//! PEM strings or bytes and never reads files itself.

mod ops;
mod tls;

pub use tls::{client_config, server_config};

use velyr_engine_api::{HostFunctionDef, Isolate, JsException, Source};
use velyr_runtime_ops::CapabilityKind;

/// Bootstrap script: adds the TLS APIs to `Velyr` on top of the private `op_tls_*` bindings.
pub const TLS_JS: &str = include_str!("tls.js");

/// The tls ops with the capability each declares, for the registry audit.
pub fn tls_ops() -> Vec<(HostFunctionDef, CapabilityKind)> {
    ops::OPS.to_vec()
}

pub fn tls_host_functions() -> Vec<HostFunctionDef> {
    tls_ops().into_iter().map(|(def, _)| def).collect()
}

/// Runs [`TLS_JS`]. Call after `install_net` (it uses the net bootstrap and its internal hook).
pub fn install_tls(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate.set_slot(tls::TlsState::default());
    isolate
        .eval_script(&Source {
            name: "velyr:tls".into(),
            text: TLS_JS.into(),
        })
        .map(drop)
}
