//! `Runtime::call`: a generated entry module imports the loaded module, awaits the export and
//! hands the JSON result back through a private op.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex};

use serde_json::Value as Json;
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};

pub(crate) const RETURN_OP: &str = "op_embed_return";

pub(crate) const RETURN_DEF: HostFunctionDef = HostFunctionDef {
    name: RETURN_OP,
    arity: 2,
    callback: op_embed_return,
};

/// Isolate slot where [`op_embed_return`] leaves the JSON text. `token` is a per-call secret that
/// only the generated wrapper knows; the op ignores callers that cannot present it.
#[derive(Clone)]
pub(crate) struct ReturnSlot {
    token: Arc<str>,
    pub(crate) result: Arc<Mutex<Option<String>>>,
}

impl ReturnSlot {
    pub(crate) fn new() -> Self {
        // `RandomState` is seeded from the OS CSPRNG; two hashes give 128 unpredictable bits.
        let bits = || RandomState::new().build_hasher().finish();
        Self {
            token: format!("{:016x}{:016x}", bits(), bits()).into(),
            result: Arc::default(),
        }
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

fn op_embed_return(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (Some(Value::String(token)), Some(Value::String(text))) = (args.first(), args.get(1))
    else {
        return Err(HostError::TypeError(format!(
            "{RETURN_OP} expects a token and a string"
        )));
    };
    let slot = ctx
        .isolate_slot::<ReturnSlot>()
        .ok_or_else(|| HostError::TypeError(format!("{RETURN_OP} needs a call slot")))?;
    if token.to_string_lossy() != *slot.token {
        return Err(HostError::TypeError(format!(
            "{RETURN_OP} rejected the token"
        )));
    }
    *slot.result.lock().expect("return slot is never poisoned") = Some(text.to_string_lossy());
    Ok(Value::Undefined)
}

/// Source of the entry module. Name, URL and args are JSON, which is valid JS literal syntax.
/// A static import keeps the first synchronous stretch of the export inside the entry module's
/// evaluation turn, which is what the CPU watchdog times. The imported module's top level runs
/// before the op is captured and can reach `op_embed_return`, so the op only accepts the per-call
/// `token`, which is embedded here and never visible to the imported module's code.
pub(crate) fn wrapper_source(module_url: &str, export: &str, args: &[Json], token: &str) -> String {
    let literal = |value: &Json| value.to_string();
    format!(
        "import * as ns from {url};\n\
         const ret = globalThis.{RETURN_OP};\n\
         delete globalThis.{RETURN_OP};\n\
         const name = {name};\n\
         if (typeof ns[name] !== 'function') throw new TypeError('export `' + name + '` is not a function');\n\
         const out = await ns[name](...{args});\n\
         ret({token}, JSON.stringify(out === undefined ? null : out));\n",
        token = literal(&Json::from(token)),
        name = literal(&Json::from(export)),
        url = literal(&Json::from(module_url)),
        args = literal(&Json::from(args)),
    )
}
