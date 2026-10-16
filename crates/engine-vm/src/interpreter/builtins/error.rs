//! `Error` and the NativeError constructors (ECMA-262 §20.5), with the V8 stack API
//! (`Error.captureStackTrace`, `Error.stackTraceLimit`).

use std::rc::Rc;

use super::{Interpreter, JsResult, arg, object_value};
use crate::interpreter::ErrorKind;
use crate::interpreter::stack_trace::{DEFAULT_STACK_TRACE_LIMIT, capture_stack_trace};
use crate::{JsStr, PropertyDescriptor, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    for kind in ErrorKind::ALL {
        install_error(vm, kind)?;
    }
    let base = vm.state.intrinsics.error_prototypes[0];
    vm.method(base, "toString", 0, error_to_string)
}

fn install_error(vm: &mut Interpreter, kind: ErrorKind) -> JsResult<()> {
    let prototype = vm.state.intrinsics.error_prototypes[kind as usize];
    let construct = Rc::new(move |vm: &mut Interpreter, _: &Value, args: &[Value]| {
        construct_error(vm, kind, args)
    });
    let constructor = vm.create_native_function(kind.name(), 1, construct, true)?;
    vm.keep(&object_value(constructor));
    vm.constant(constructor, "prototype", object_value(prototype))?;
    vm.define_method(prototype, "constructor", object_value(constructor))?;
    vm.define_method(prototype, "name", Value::str(kind.name()))?;
    vm.define_method(prototype, "message", Value::str(""))?;
    if kind == ErrorKind::Error {
        install_stack_api(vm, constructor)?;
    } else {
        let error = vm.state.intrinsics.error_constructor;
        vm.set_prototype(constructor, Some(error))?;
    }
    let global = vm.state.intrinsics.global;
    vm.define_method(global, kind.name(), object_value(constructor))
}

fn install_stack_api(vm: &mut Interpreter, constructor: crate::ObjectRef) -> JsResult<()> {
    vm.state.intrinsics.error_constructor = constructor;
    let limit = Value::Number(DEFAULT_STACK_TRACE_LIMIT);
    let key = vm.realm.key_str("stackTraceLimit");
    let desc = PropertyDescriptor::data(limit, true, true, true);
    vm.define_property(constructor, key, &desc)?;
    vm.method(constructor, "captureStackTrace", 2, capture_stack_trace)?;
    vm.method(constructor, "isError", 1, error_is_error)
}

/// `Error.isError(value)`: whether `value` has an [[ErrorData]] slot (ES2026).
fn error_is_error(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(
        vm.value_slot(&arg(args, 0)),
        Some(super::slots::Slot::Error)
    )))
}

/// The NativeError/Error constructor body: `message` and `options.cause`.
fn construct_error(vm: &mut Interpreter, kind: ErrorKind, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    let default = vm.state.intrinsics.error_prototypes[kind as usize];
    let proto = match new_target {
        Value::Undefined => default,
        _ => vm.proto_from_constructor(&new_target, default)?,
    };
    let message = match arg(args, 0) {
        Value::Undefined => None,
        message => Some(vm.coerce_string(&message)?),
    };
    let error = vm.create_error_with_proto(proto, message)?;
    vm.keep(&error);
    let options = arg(args, 1);
    if let Some(options) = vm.as_object(&options) {
        let key = vm.realm.key_str("cause");
        if vm.has_property(options, key)? {
            let cause = vm.get_key(options, key)?;
            let object = vm.as_object(&error).expect("errors are objects");
            vm.define_method(object, "cause", cause)?;
        }
    }
    Ok(error)
}

fn error_to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let error = vm.this_object_for(this, "Error.prototype.toString")?;
    let text = crate::interpreter::intrinsics::describe_error(vm, error, this)?;
    Ok(Value::String(JsStr::from_str(&text)))
}
