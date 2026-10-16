//! The two ops under `AsyncLocalStorage`: set and read the engine's continuation context (the
//! value V8 restores around promise reactions and `await`). On an engine without that feature
//! they do nothing and read `undefined`, which `internal/async_context` detects and covers.

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};

pub(crate) fn async_context_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_async_context_set",
            arity: 1,
            callback: op_async_context_set,
        },
        HostFunctionDef {
            name: "op_async_context_get",
            arity: 0,
            callback: op_async_context_get,
        },
    ]
}

/// `op_async_context_set(frame)`
fn op_async_context_set(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    ctx.set_continuation_context(args.first().cloned().unwrap_or(Value::Undefined));
    Ok(Value::Undefined)
}

/// `op_async_context_get() -> frame`
fn op_async_context_get(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(ctx.get_continuation_context())
}
