use velyr_engine_api::HostFunctionDef;

use crate::CapabilityKind;

/// Globals that exist only for tests (feature `test-ops`); empty otherwise. Unlike ops
/// these stay on the global object, so they are not named `op_*`.
pub fn test_host_functions() -> Vec<HostFunctionDef> {
    test_ops().into_iter().map(|(def, _)| def).collect()
}

/// [`test_host_functions`] with the capability each declares, for the registry audit.
pub fn test_ops() -> Vec<(HostFunctionDef, CapabilityKind)> {
    #[cfg(feature = "test-ops")]
    return vec![
        (
            HostFunctionDef {
                name: "nativeAdd",
                arity: 2,
                callback: native_add,
            },
            CapabilityKind::None,
        ),
        (
            HostFunctionDef {
                name: "nativeAsync",
                arity: 0,
                callback: native_async,
            },
            CapabilityKind::None,
        ),
        (
            HostFunctionDef {
                name: "nativeAsyncFail",
                arity: 0,
                callback: native_async_fail,
            },
            CapabilityKind::None,
        ),
        (
            HostFunctionDef {
                name: "nativeEnvRead",
                arity: 1,
                callback: native_env_read,
            },
            CapabilityKind::EnvRead,
        ),
        (
            HostFunctionDef {
                name: "nativeLeakOpen",
                arity: 0,
                callback: native_leak_open,
            },
            CapabilityKind::None,
        ),
    ];
    #[cfg(not(feature = "test-ops"))]
    Vec::new()
}

/// `nativeEnvRead(name)`: the variable's value (or `undefined`), only after `authorize`.
#[cfg(feature = "test-ops")]
fn native_env_read(
    ctx: &mut dyn velyr_engine_api::CallContext,
    args: &[velyr_engine_api::Value],
) -> Result<velyr_engine_api::Value, velyr_engine_api::HostError> {
    use velyr_engine_api::{HostError, Value};
    use velyr_runtime_permissions::Request;
    let [Value::String(name)] = args else {
        return Err(HostError::TypeError("nativeEnvRead expects a name".into()));
    };
    let name = name.to_string_lossy();
    crate::authorize(ctx, "nativeEnvRead", Request::EnvRead(&name))?;
    Ok(match std::env::var(&name) {
        Ok(value) => ctx.create_string(value.as_str().into()),
        Err(_) => Value::Undefined,
    })
}

#[cfg(feature = "test-ops")]
fn native_add(
    _: &mut dyn velyr_engine_api::CallContext,
    args: &[velyr_engine_api::Value],
) -> Result<velyr_engine_api::Value, velyr_engine_api::HostError> {
    use velyr_engine_api::{HostError, Value};
    match args {
        [Value::Number(a), Value::Number(b)] => Ok(Value::Number(a + b)),
        _ => Err(HostError::TypeError("nativeAdd expects two numbers".into())),
    }
}

/// Resolves `"done"` after one runtime tick.
#[cfg(feature = "test-ops")]
fn native_async(
    ctx: &mut dyn velyr_engine_api::CallContext,
    _: &[velyr_engine_api::Value],
) -> Result<velyr_engine_api::Value, velyr_engine_api::HostError> {
    crate::op_async(ctx, |_token| async {
        tokio::task::yield_now().await;
        Ok(velyr_runtime_event_loop::CompletionValue::String(
            "done".into(),
        ))
    })
}

/// Rejects with `VELYR_INTERNAL`.
#[cfg(feature = "test-ops")]
fn native_async_fail(
    ctx: &mut dyn velyr_engine_api::CallContext,
    _: &[velyr_engine_api::Value],
) -> Result<velyr_engine_api::Value, velyr_engine_api::HostError> {
    crate::op_async(ctx, |_token| async {
        tokio::task::yield_now().await;
        Err(velyr_runtime_errors::VelyrError::internal(
            "native async failure",
        ))
    })
}

/// `nativeLeakOpen()`: inserts a resource into the table and never closes it.
#[cfg(feature = "test-ops")]
fn native_leak_open(
    ctx: &mut dyn velyr_engine_api::CallContext,
    _: &[velyr_engine_api::Value],
) -> Result<velyr_engine_api::Value, velyr_engine_api::HostError> {
    use velyr_engine_api::{HostError, Value};
    use velyr_runtime_resources::{QuotaClass, Resource};

    struct TestLeak;
    impl Resource for TestLeak {
        fn kind(&self) -> &'static str {
            "test_leak"
        }
        fn close(self: Box<Self>) {}
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let op_ctx = ctx
        .isolate_slot::<crate::OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))?;
    let id = op_ctx
        .resources
        .insert(Box::new(TestLeak), QuotaClass::Files, None)?;
    Ok(Value::Number(id.to_js_number()))
}
