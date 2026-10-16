//! Host op behind `node:v8`: `op_node_v8(name, value)`. `"serialize"` structured-clones a value to
//! an `ArrayBuffer` through the engine's own serializer (so it round-trips with `"deserialize"`
//! on the same engine, and is not Node's wire format); `"deserialize"` rebuilds a value from
//! such bytes. The engine validates the bytes: malformed input throws instead of being trusted.

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, SerializedValue, Value};

pub(crate) fn v8_host_functions() -> Vec<HostFunctionDef> {
    vec![HostFunctionDef {
        name: "op_node_v8",
        arity: 2,
        callback: op_v8,
    }]
}

fn op_v8(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(name)) = args.first() else {
        return Err(HostError::TypeError(
            "op_node_v8 expects (name, value)".into(),
        ));
    };
    let value = args.get(1).unwrap_or(&Value::Undefined);
    match name.to_string_lossy().as_str() {
        "serialize" => {
            let serialized = ctx.serialize(value, &[])?;
            Ok(ctx.create_array_buffer(&serialized.bytes))
        }
        "deserialize" => {
            let bytes = ctx.read_bytes(value).ok_or_else(|| {
                HostError::TypeError("op_node_v8 deserialize expects bytes".into())
            })?;
            ctx.deserialize(&SerializedValue {
                bytes,
                transferred: Vec::new(),
            })
        }
        other => Err(HostError::TypeError(format!("unknown v8 query {other}"))),
    }
}
