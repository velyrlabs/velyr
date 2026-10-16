use std::collections::BTreeMap;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_permissions::Request;

use crate::context::string_arg;
use crate::{OpContext, authorize};

pub const OP_ENV_GET: HostFunctionDef = HostFunctionDef {
    name: "op_env_get",
    arity: 1,
    callback: op_env_get,
};
pub const OP_ENV_HAS: HostFunctionDef = HostFunctionDef {
    name: "op_env_has",
    arity: 1,
    callback: op_env_has,
};
pub const OP_ENV_SET: HostFunctionDef = HostFunctionDef {
    name: "op_env_set",
    arity: 2,
    callback: op_env_set,
};
pub const OP_ENV_ENTRIES: HostFunctionDef = HostFunctionDef {
    name: "op_env_entries",
    arity: 0,
    callback: op_env_entries,
};

fn op_context(ctx: &mut dyn CallContext) -> Result<&mut OpContext, HostError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))
}

/// Overlay first, then the process environment; non-Unicode values read as absent.
fn lookup(op_ctx: &OpContext, name: &str) -> Option<String> {
    op_ctx
        .env_overlay
        .get(name)
        .cloned()
        .or_else(|| std::env::var(name).ok())
}

/// `op_env_get(name)`: the value or `undefined`; denied unless `name` is granted.
fn op_env_get(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.env_get").entered();
    let name = string_arg(args, 0, "op_env_get")?;
    authorize(ctx, "op_env_get", Request::EnvRead(&name))?;
    let value = lookup(op_context(ctx)?, &name);
    Ok(value.map_or(Value::Undefined, |v| ctx.create_string(v.as_str().into())))
}

/// `op_env_has(name)`: denied unless `name` is granted, so presence of other names stays hidden.
fn op_env_has(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.env_has").entered();
    let name = string_arg(args, 0, "op_env_has")?;
    authorize(ctx, "op_env_has", Request::EnvRead(&name))?;
    Ok(Value::Bool(lookup(op_context(ctx)?, &name).is_some()))
}

/// `op_env_set(name, value)`: stores in the per-isolate overlay (the crate forbids `unsafe`, and
/// `set_var` would leak across isolates anyway).
fn op_env_set(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.env_set").entered();
    let name = string_arg(args, 0, "op_env_set")?;
    authorize(ctx, "op_env_set", Request::EnvWrite(&name))?;
    let value = string_arg(args, 1, "op_env_set")?;
    if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
        return Err(HostError::TypeError("invalid environment variable".into()));
    }
    op_context(ctx)?.env_overlay.insert(name, value);
    Ok(Value::Undefined)
}

/// `op_env_entries()`: flat `[name, value, ...]` of every readable variable. Needs the `*` grant;
/// variables an explicit deny covers are left out.
fn op_env_entries(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.env_entries").entered();
    authorize(ctx, "op_env_entries", Request::EnvRead("*"))?;
    let op_ctx = op_context(ctx)?;
    let mut vars: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    vars.extend(op_ctx.env_overlay.clone());
    vars.retain(|name, _| op_ctx.permissions.check(&Request::EnvRead(name)).is_allow());
    let flat: Vec<Value> = vars
        .iter()
        .flat_map(|(k, v)| [k, v])
        .map(|s| ctx.create_string(s.as_str().into()))
        .collect();
    Ok(ctx.create_array(&flat))
}
