//! `Velyr.metrics()`: one op that returns the snapshot as JSON, wrapped by a tiny bootstrap.

use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, Isolate, JsException, Source, Value,
};

use super::global;

fn op_metrics_snapshot(ctx: &mut dyn CallContext, _args: &[Value]) -> Result<Value, HostError> {
    let json = global().snapshot().to_json().to_string();
    Ok(ctx.create_string(json.as_str().into()))
}

pub fn metrics_host_functions() -> Vec<HostFunctionDef> {
    vec![HostFunctionDef {
        name: "op_metrics_snapshot",
        arity: 0,
        callback: op_metrics_snapshot,
    }]
}

const METRICS_JS: &str = r#"(function (global) {
  "use strict";
  const snapshot = global.op_metrics_snapshot;
  delete global.op_metrics_snapshot;
  Object.defineProperty(global.Velyr, "metrics", {
    value: function metrics() { return JSON.parse(snapshot()); },
    writable: false, configurable: true, enumerable: true,
  });
})(globalThis);"#;

/// Defines `Velyr.metrics()`. Call after the `Velyr` global exists.
pub fn install_metrics(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate
        .eval_script(&Source {
            name: "velyr:metrics".into(),
            text: METRICS_JS.into(),
        })
        .map(drop)
}
