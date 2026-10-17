//! The `op_cjs_*` host functions behind `require`. Both are synchronous (`require` is), so they
//! read with `std::fs` after the same `ImportLocal`/`FsRead` check `ModuleHost::load` makes.

use std::path::Path;
use std::sync::Arc;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, JsString, Value};
use velyr_runtime_core::require_esm;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::authorize_path;
use velyr_runtime_ops::CapabilityKind;
use velyr_runtime_permissions::CapabilitySet;

use crate::analysis::{analyze, rewrite_dynamic_imports};
use crate::format::Format;
use crate::fscache;
use crate::resolve::resolve;

/// Isolate slot the ops read.
pub(crate) struct CommonJsState {
    pub permissions: Arc<CapabilitySet>,
    pub conditions: Vec<String>,
}

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 3] = [
    (
        HostFunctionDef {
            name: "op_cjs_resolve",
            arity: 2,
            callback: op_cjs_resolve,
        },
        CapabilityKind::ImportLocal,
    ),
    (
        HostFunctionDef {
            name: "op_cjs_read",
            arity: 1,
            callback: op_cjs_read,
        },
        CapabilityKind::ImportLocal,
    ),
    (
        HostFunctionDef {
            name: "op_cjs_require_esm",
            arity: 1,
            callback: op_cjs_require_esm,
        },
        CapabilityKind::ImportLocal,
    ),
];

fn string_arg(args: &[Value], index: usize, op: &str) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(HostError::TypeError(format!(
            "{op} expects argument {index} to be a string"
        ))),
    }
}

fn state(ctx: &mut dyn CallContext) -> Result<(Arc<CapabilitySet>, Vec<String>), HostError> {
    ctx.isolate_slot::<CommonJsState>()
        .map(|s| (s.permissions.clone(), s.conditions.clone()))
        .ok_or_else(|| HostError::TypeError("CommonJS ops need install_commonjs".into()))
}

fn read_error(err: std::io::Error) -> VelyrError {
    match err.kind() {
        std::io::ErrorKind::NotFound => VelyrError::not_found("module not found"),
        _ => VelyrError::invalid_data("module is not readable UTF-8 text").with_cause(err),
    }
}

fn text(value: &str) -> Value {
    Value::String(JsString::from(value))
}

/// `op_cjs_resolve(request, parentFile) -> '{"path": ..., "format": "commonjs"|"json"|"esm"}'`.
fn op_cjs_resolve(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let request = string_arg(args, 0, "op_cjs_resolve")?;
    let parent = string_arg(args, 1, "op_cjs_resolve")?;
    let (permissions, conditions) = state(ctx)?;
    let path = resolve(&request, Path::new(&parent), &conditions)?;
    authorize_path(&path, &permissions)?;
    let format = fscache::global().classify(&path);
    let path = path.to_string_lossy();
    Ok(text(
        &serde_json::json!({ "path": path, "format": format.as_str() }).to_string(),
    ))
}

/// `op_cjs_read(path) -> source`; CommonJS text has its `import(` calls routed through the
/// wrapper's `__velyrImport`.
fn op_cjs_read(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let path = string_arg(args, 0, "op_cjs_read")?;
    let (permissions, _) = state(ctx)?;
    let path = Path::new(&path);
    authorize_path(path, &permissions)?;
    let read = fscache::global().read(path).map_err(read_error)?;
    if read.format == Format::Json || !read.text.contains("import") {
        return Ok(text(&read.text));
    }
    let offsets = analyze(&read.text)
        .map(|a| a.dynamic_imports)
        .unwrap_or_default();
    Ok(text(&rewrite_dynamic_imports(&read.text, &offsets)))
}

/// `op_cjs_require_esm(path) -> namespace`, or `undefined` when the module has top-level await
/// (a namespace is never `undefined`). The graph loads through the runtime's `EsmRequire`.
fn op_cjs_require_esm(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let path = string_arg(args, 0, "op_cjs_require_esm")?;
    Ok(require_esm(ctx, Path::new(&path))?.unwrap_or(Value::Undefined))
}
