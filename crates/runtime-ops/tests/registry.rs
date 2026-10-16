use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::ErrorCode;
use velyr_runtime_ops::{OpId, OpRegistry};

fn noop(_: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(Value::Undefined)
}

fn def(name: &'static str) -> HostFunctionDef {
    HostFunctionDef {
        name,
        arity: 0,
        callback: noop,
    }
}

#[test]
fn ids_are_sequential_in_registration_order() {
    let mut registry = OpRegistry::new();
    assert_eq!(registry.register(def("op_a_one")).unwrap(), OpId(0));
    assert_eq!(registry.register(def("op_a_two")).unwrap(), OpId(1));
    assert_eq!(registry.id_of("op_a_two"), Some(OpId(1)));
    assert_eq!(registry.id_of("op_missing_x"), None);
    assert_eq!(registry.host_functions().len(), 2);
}

#[test]
fn rejects_duplicate_names() {
    let mut registry = OpRegistry::new();
    registry.register(def("op_a_one")).unwrap();
    let err = registry.register(def("op_a_one")).unwrap_err();
    assert_eq!(err.code(), ErrorCode::AlreadyExists);
}

#[test]
fn rejects_names_outside_op_area_verb() {
    let mut registry = OpRegistry::new();
    for name in ["nativeAdd", "op_", "op_console", "op__write", "op_console_"] {
        let err = registry.register(def(name)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidData, "{name}");
    }
}

#[test]
fn builtin_registry_has_console_write() {
    assert!(OpRegistry::builtin().id_of("op_console_write").is_some());
}
