//! `SuppressedError` (explicit resource management): an error raised while disposing a
//! resource, carrying the earlier pending error.

use super::{Interpreter, Intrinsic, JsResult, arg};
use crate::{PropertyDescriptor, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let error_prototype = vm.state.intrinsics.error_prototypes[0];
    let proto = vm.new_object_with(Some(error_prototype))?;
    vm.set_intrinsic(Intrinsic::SuppressedErrorPrototype, proto);
    let constructor = vm.constructor("SuppressedError", 3, suppressed_error_constructor, proto)?;
    let error = vm.state.intrinsics.error_constructor;
    vm.set_prototype(constructor, Some(error))?;
    vm.define_method(proto, "name", Value::str("SuppressedError"))?;
    vm.define_method(proto, "message", Value::str(""))
}

/// `SuppressedError(error, suppressed, message)`.
fn suppressed_error_constructor(
    vm: &mut Interpreter,
    _: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let new_target = vm.new_target();
    let default = vm.intrinsic(Intrinsic::SuppressedErrorPrototype);
    let proto = match new_target {
        Value::Undefined => default,
        _ => vm.proto_from_constructor(&new_target, default)?,
    };
    let message = match arg(args, 2) {
        Value::Undefined => None,
        message => Some(vm.coerce_string(&message)?),
    };
    let error = vm.create_error_with_proto(proto, message)?;
    vm.keep(&error);
    define_pair(vm, &error, arg(args, 0), arg(args, 1))?;
    Ok(error)
}

/// A `SuppressedError` without a message: `error` is the newly thrown disposal error and
/// `suppressed` the earlier pending one (DisposeResources).
pub(crate) fn make_suppressed(
    vm: &mut Interpreter,
    error: Value,
    suppressed: Value,
) -> JsResult<Value> {
    let proto = vm.intrinsic(Intrinsic::SuppressedErrorPrototype);
    let result = vm.create_error_with_proto(proto, None)?;
    vm.keep(&result);
    define_pair(vm, &result, error, suppressed)?;
    Ok(result)
}

fn define_pair(
    vm: &mut Interpreter,
    target: &Value,
    error: Value,
    suppressed: Value,
) -> JsResult<()> {
    let object = vm.as_object(target).expect("errors are objects");
    for (name, value) in [("error", error), ("suppressed", suppressed)] {
        let key = vm.realm.key_str(name);
        vm.define_or_throw(
            object,
            key,
            &PropertyDescriptor::data(value, true, false, true),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_suppressed_roles() {
        let mut vm = Interpreter::new();
        let value =
            make_suppressed(&mut vm, Value::Number(1.0), Value::Number(2.0)).expect("builds");
        let object = vm.as_object(&value).expect("object");
        for (name, expected) in [("error", 1.0), ("suppressed", 2.0)] {
            let key = vm.realm.key_str(name);
            assert!(matches!(vm.get_key(object, key), Ok(Value::Number(n)) if n == expected));
        }
        let message = vm.realm.key_str("message");
        assert!(vm.own_property(object, message).expect("lookup").is_none());
        let proto = vm.intrinsic(Intrinsic::SuppressedErrorPrototype);
        assert_eq!(vm.get_prototype(object).expect("proto"), Some(proto));
    }
}
