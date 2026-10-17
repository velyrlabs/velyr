use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};

pub(crate) fn microtask_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_queue_microtask",
            arity: 1,
            callback: op_queue_microtask,
        },
        HostFunctionDef {
            name: "op_microtask_checkpoint",
            arity: 0,
            callback: op_microtask_checkpoint,
        },
    ]
}

/// `op_queue_microtask(fn) -> queued`; `false` when the engine has no microtask queue.
fn op_queue_microtask(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [func] = args else {
        return Err(HostError::TypeError(
            "op_queue_microtask expects (fn: function)".into(),
        ));
    };
    Ok(Value::Bool(ctx.enqueue_microtask(func)?))
}

/// `op_microtask_checkpoint()`
fn op_microtask_checkpoint(ctx: &mut dyn CallContext, _args: &[Value]) -> Result<Value, HostError> {
    ctx.run_microtask_checkpoint();
    Ok(Value::Undefined)
}
