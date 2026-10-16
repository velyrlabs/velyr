//! `AggregateError` (ECMA-262 §20.5.7), needed by `Promise.any`.

use super::{Interpreter, Intrinsic, JsResult, arg};
use crate::{JsStr, PropertyDescriptor, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let error_prototype = vm.state.intrinsics.error_prototypes[0];
    let proto = vm.new_object_with(Some(error_prototype))?;
    vm.set_intrinsic(Intrinsic::AggregateErrorPrototype, proto);
    let constructor = vm.constructor("AggregateError", 2, aggregate_error_constructor, proto)?;
    let error = vm.state.intrinsics.error_constructor;
    vm.set_prototype(constructor, Some(error))?;
    vm.define_method(proto, "name", Value::str("AggregateError"))?;
    vm.define_method(proto, "message", Value::str(""))
}

/// `AggregateError(errors, message, options)`.
fn aggregate_error_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    let default = vm.intrinsic(Intrinsic::AggregateErrorPrototype);
    let proto = match new_target {
        Value::Undefined => default,
        _ => vm.proto_from_constructor(&new_target, default)?,
    };
    let message = match arg(args, 1) {
        Value::Undefined => None,
        message => Some(vm.coerce_string(&message)?),
    };
    let error = vm.create_error_with_proto(proto, message)?;
    vm.keep(&error);
    let object = vm.as_object(&error).expect("errors are objects");
    if let Some(options) = vm.as_object(&arg(args, 2)) {
        let key = vm.realm.key_str("cause");
        if vm.has_property(options, key)? {
            let cause = vm.get_key(options, key)?;
            vm.define_method(object, "cause", cause)?;
        }
    }
    let mut errors = Vec::new();
    vm.iterate(&arg(args, 0), |_, value| {
        errors.push(value);
        Ok(())
    })?;
    let errors = vm.array_from_list(&errors)?;
    vm.define_method(object, "errors", errors)?;
    Ok(error)
}

impl Interpreter {
    /// A `%AggregateError%` with `message` and the `errors` array, as `Promise.any` makes.
    pub(crate) fn aggregate_error(&mut self, errors: Value, message: &str) -> JsResult<Value> {
        let proto = self.intrinsic(Intrinsic::AggregateErrorPrototype);
        let error = self.create_error_with_proto(proto, Some(JsStr::from_str(message)))?;
        self.keep(&error);
        let object = self.as_object(&error).expect("errors are objects");
        let key = self.realm.key_str("errors");
        let desc = PropertyDescriptor::data(errors, true, false, true);
        self.define_or_throw(object, key, &desc)?;
        Ok(error)
    }
}
