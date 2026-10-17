//! `Runtime::call`: a generated entry module imports the loaded module, awaits the export and
//! hands the JSON result back through a private op.

use std::sync::{Arc, Mutex};

use serde_json::Value as Json;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};

pub(crate) const RETURN_OP: &str = "op_embed_return";

pub(crate) const RETURN_DEF: HostFunctionDef = HostFunctionDef {
    name: RETURN_OP,
    arity: 1,
    callback: op_embed_return,
};

/// Isolate slot where [`op_embed_return`] leaves the JSON text.
#[derive(Clone, Default)]
pub(crate) struct ReturnSlot(pub(crate) Arc<Mutex<Option<String>>>);

fn op_embed_return(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(text)) = args.first() else {
        return Err(HostError::TypeError(format!(
            "{RETURN_OP} expects a string"
        )));
    };
    let slot = ctx
        .isolate_slot::<ReturnSlot>()
        .ok_or_else(|| HostError::TypeError(format!("{RETURN_OP} needs a call slot")))?;
    *slot.0.lock().expect("return slot is never poisoned") = Some(text.to_string_lossy());
    Ok(Value::Undefined)
}

/// Source of the entry module. Name, URL and args are JSON, which is valid JS literal syntax.
/// A static import keeps the first synchronous stretch of the export inside the entry module's
/// evaluation turn, which is what the CPU watchdog times. The imported module's top level runs
/// before the op is captured, so it could see (and spoof) `op_embed_return`; same trust domain.
pub(crate) fn wrapper_source(module_url: &str, export: &str, args: &[Json]) -> String {
    let literal = |value: &Json| value.to_string();
    format!(
        "import * as ns from {url};\n\
         const ret = globalThis.{RETURN_OP};\n\
         delete globalThis.{RETURN_OP};\n\
         const name = {name};\n\
         if (typeof ns[name] !== 'function') throw new TypeError('export `' + name + '` is not a function');\n\
         const out = await ns[name](...{args});\n\
         ret(JSON.stringify(out === undefined ? null : out));\n",
        name = literal(&Json::from(export)),
        url = literal(&Json::from(module_url)),
        args = literal(&Json::from(args)),
    )
}
