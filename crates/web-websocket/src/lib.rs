//! `WebSocket` (WHATWG) and `Velyr.upgradeWebSocket` on tokio-tungstenite.

use velyr_engine_api::{HostFunctionDef, Isolate, JsException, Source};
use velyr_runtime_ops::CapabilityKind;

mod ops;
mod socket;

pub const WEBSOCKET_JS: &str = include_str!("websocket.js");

pub fn websocket_ops() -> Vec<(HostFunctionDef, CapabilityKind)> {
    ops::OPS.to_vec()
}

pub fn websocket_host_functions() -> Vec<HostFunctionDef> {
    websocket_ops().into_iter().map(|(def, _)| def).collect()
}

/// Runs [`WEBSOCKET_JS`]. Call after `install_web_fetch` (the server upgrade needs it), events,
/// url, encoding and web-streams.
pub fn install_web_websocket(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate
        .eval_script(&Source {
            name: "velyr:websocket".into(),
            text: WEBSOCKET_JS.into(),
        })
        .map(drop)
}
