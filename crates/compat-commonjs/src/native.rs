//! `op_cjs_dlopen`, behind `require('./x.node')` and `process.dlopen`: loads a Node-API addon
//! through `velyr-compat-napi` in the engine's native scope. `NativeLoad` is checked before
//! `dlopen`; the addon envs are torn down when the isolate is disposed.

use std::path::Path;
use std::sync::Arc;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_ops::CapabilityKind;

use crate::ops::CommonJsState;

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 1] = [(
    HostFunctionDef {
        name: "op_cjs_dlopen",
        arity: 1,
        callback: op_cjs_dlopen,
    },
    CapabilityKind::NativeLoad,
)];

/// `op_cjs_dlopen(path) -> exports`; an exception the module init threw is rethrown.
fn op_cjs_dlopen(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(path)) = args.first() else {
        return Err(HostError::TypeError(
            "op_cjs_dlopen expects argument 0 to be a string".into(),
        ));
    };
    let path = path.to_string_lossy();
    let permissions = ctx
        .isolate_slot::<CommonJsState>()
        .map(|state| Arc::clone(&state.permissions))
        .ok_or_else(|| HostError::TypeError("CommonJS ops need install_commonjs".into()))?;
    let exports = ctx.with_native_scope(&mut |scope| {
        velyr_compat_napi::load_native(scope, Path::new(&path), &permissions)
    });
    // Also after a failed load: a module init that threw still left an env behind.
    ctx.on_native_teardown(velyr_compat_napi::shutdown_native)?;
    exports
}

#[cfg(test)]
mod tests {
    use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source};
    use velyr_engine_v8::V8Engine;
    use velyr_runtime_permissions::CapabilitySet;

    use super::*;

    fn eval_with(permissions: CapabilitySet, code: &str) -> Value {
        let mut isolate = V8Engine::new()
            .create_isolate(IsolateOptions::default())
            .unwrap();
        isolate.register_host_function(OPS[0].0).unwrap();
        isolate.set_slot(CommonJsState {
            permissions: Arc::new(permissions),
            conditions: Vec::new(),
        });
        let source = Source {
            name: "test.js".into(),
            text: code.into(),
        };
        isolate.eval_script(&source).unwrap()
    }

    #[test]
    fn without_native_permission_dlopen_is_denied() {
        let probe = std::env::temp_dir().join("velyr-cjs-denied.node");
        let code = format!(
            "try {{ op_cjs_dlopen({:?}); 'loaded' }} catch (e) {{ e.code }}",
            probe.to_str().unwrap()
        );
        let result = eval_with(CapabilitySet::default(), &code);
        assert_eq!(result, Value::String("VELYR_PERMISSION_DENIED".into()));
    }

    #[test]
    fn a_non_string_path_is_a_type_error() {
        let result = eval_with(
            CapabilitySet::default(),
            "try { op_cjs_dlopen(1); 'loaded' } catch (e) { e.name }",
        );
        assert_eq!(result, Value::String("TypeError".into()));
    }
}
