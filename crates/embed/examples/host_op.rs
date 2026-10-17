//! A host op (`op_host_greet`) called from a JS module, then an exported function called from Rust.
//!
//! Run with `cargo run -p velyr --example host_op`.

use serde_json::json;
use velyr::{
    CallContext, Capability, CapabilityKind, HostError, HostFunctionDef, PathPattern, Permissions,
    Runtime, Value,
};

fn op_host_greet(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(name)) = args.first() else {
        return Err(HostError::TypeError("op_host_greet expects a name".into()));
    };
    Ok(ctx.create_string(format!("hello, {}", name.to_string_lossy()).as_str().into()))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join("velyr-embed-example");
    std::fs::create_dir_all(&dir)?;
    let module = dir.join("app.mjs");
    std::fs::write(
        &module,
        "export const greet = (name) => op_host_greet(name);",
    )?;

    let permissions = Permissions::new().allow(Capability::ImportLocal(PathPattern::new(
        dir.canonicalize()?.to_string_lossy(),
    )));
    let mut runtime = Runtime::builder()
        .permissions(permissions)
        .op(
            HostFunctionDef {
                name: "op_host_greet",
                arity: 1,
                callback: op_host_greet,
            },
            CapabilityKind::None,
        )
        .build()?;
    runtime.run_module(&module.to_string_lossy())?;
    println!("{}", runtime.call("greet", &[json!("velyr")])?);
    Ok(())
}
