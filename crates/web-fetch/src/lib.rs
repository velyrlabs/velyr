//! Fetch API (`Headers`, `Request`, `Response`, body mixin, `fetch`) for the server-side runtime.
//! The Web classes are JS written from the Fetch Standard; the network goes through io-http.

mod client;
mod decode;
#[macro_use]
mod ops;
mod multipart;
mod serve;
mod telemetry;

pub use serve::{ServeInbox, attach_upgrade, resolve_listen_host, termination_token};

use velyr_engine_api::{HostFunctionDef, Isolate, JsException, Source};
use velyr_runtime_ops::CapabilityKind;

/// Bootstrap script; the fragments share one private scope, so they are only valid concatenated
/// in this order.
pub const FETCH_JS: &str = concat!(
    include_str!("js/00_head.js"),
    concat!(
        "  const USER_AGENT = \"Velyr/",
        env!("CARGO_PKG_VERSION"),
        "\";\n"
    ),
    include_str!("js/10_headers.js"),
    include_str!("js/20_body.js"),
    include_str!("js/25_form_data.js"),
    include_str!("js/30_request.js"),
    include_str!("js/40_response.js"),
    include_str!("js/50_fetch.js"),
    include_str!("js/60_serve.js"),
    include_str!("js/70_telemetry.js"),
    include_str!("js/90_exports.js"),
);

/// The fetch ops with the capability each declares, for the registry audit.
pub fn fetch_ops() -> Vec<(HostFunctionDef, CapabilityKind)> {
    ops::OPS
        .into_iter()
        .chain(serve::OPS)
        .chain(multipart::OPS)
        .chain(telemetry::OPS)
        .collect()
}

pub fn fetch_host_functions() -> Vec<HostFunctionDef> {
    fetch_ops().into_iter().map(|(def, _)| def).collect()
}

/// Runs [`FETCH_JS`]. Call after `install_net`, `install_web_streams`, events, encoding and URL.
pub fn install_web_fetch(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate.set_slot(client::FetchState::default());
    isolate
        .eval_script(&Source {
            name: "velyr:fetch".into(),
            text: FETCH_JS.into(),
        })
        .map(drop)
}
