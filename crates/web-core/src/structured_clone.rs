use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, Isolate, JsException, Source, Value,
};

/// Bootstrap script: defines the `structuredClone` global on top of the private
/// `op_structured_clone` binding.
pub const STRUCTURED_CLONE_JS: &str = include_str!("structured_clone.js");

pub fn structured_clone_host_functions() -> Vec<HostFunctionDef> {
    vec![HostFunctionDef {
        name: "op_structured_clone",
        arity: 2,
        callback: op_structured_clone,
    }]
}

/// Runs [`STRUCTURED_CLONE_JS`]. Call from the `install` hook of `run_file`, after the
/// `structured_clone_host_functions` are registered. Needs an engine with `EngineFeatures::SERIALIZE`.
pub fn install_structured_clone(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate
        .eval_script(&Source {
            name: "velyr:structured-clone".into(),
            text: STRUCTURED_CLONE_JS.into(),
        })
        .map(drop)
}

/// `op_structured_clone(value, transfer: Array) -> clone`
///
/// Serializes then deserializes in the same isolate; `transfer` buffers are detached and
/// reattached to the clone. Failures throw the engine's `DataCloneError`.
fn op_structured_clone(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [value, list @ Value::Object(_)] = args else {
        return Err(HostError::TypeError(
            "op_structured_clone expects (value, transfer: Array)".into(),
        ));
    };
    let transfer = array_items(ctx, list)?;
    let serialized = ctx.serialize(value, &transfer)?;
    ctx.deserialize(&serialized)
}

fn array_items(ctx: &mut dyn CallContext, array: &Value) -> Result<Vec<Value>, HostError> {
    let Value::Number(length) = ctx.get_property(array, "length")? else {
        return Err(HostError::TypeError("transfer list has no length".into()));
    };
    (0..length as usize)
        .map(|index| ctx.get_property(array, &index.to_string()))
        .collect()
}
