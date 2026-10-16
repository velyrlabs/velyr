//! The `Promise` constructor and prototype (ECMA-262 §27.2, VELYR-108). The abstract
//! operations live in `promise_ops`, the combinators in `promise_combinators` and the job
//! queue in `interpreter::jobs`.

use std::rc::Rc;

use super::array::species_getter;
use super::promise_ops::{Captures, PromiseRecord};
use super::slots::Slot;
use super::symbol::SYMBOL_SPECIES;
use super::{Interpreter, Intrinsic, JsResult, arg, object_value, promise_combinators};
use crate::interpreter::Abrupt;
use crate::{ObjectKey, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let object_prototype = vm.state.intrinsics.object_prototype;
    let proto = vm.new_object_with(Some(object_prototype))?;
    vm.set_intrinsic(Intrinsic::PromisePrototype, proto);
    let constructor = vm.constructor("Promise", 1, promise_constructor, proto)?;
    vm.set_intrinsic(Intrinsic::PromiseConstructor, constructor);
    promise_combinators::install(vm, constructor)?;
    vm.method(constructor, "reject", 1, promise_reject)?;
    vm.method(constructor, "resolve", 1, promise_resolve)?;
    vm.method(constructor, "try", 1, promise_try)?;
    vm.method(constructor, "withResolvers", 0, promise_with_resolvers)?;
    let species = ObjectKey::Symbol(SYMBOL_SPECIES);
    vm.accessor(
        constructor,
        species,
        "[Symbol.species]",
        species_getter,
        None,
    )?;
    vm.method(proto, "catch", 1, promise_catch)?;
    vm.method(proto, "finally", 1, promise_finally)?;
    vm.method(proto, "then", 2, promise_then)?;
    vm.define_to_string_tag(proto, "Promise")
}

/// `new Promise(executor)`.
fn promise_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type("Promise constructor cannot be invoked without 'new'");
    }
    let executor = arg(args, 0);
    if !vm.is_callable(&executor) {
        return vm.throw_type("Promise resolver is not a function");
    }
    let record = PromiseRecord::pending();
    let default = vm.intrinsic(Intrinsic::PromisePrototype);
    let slot = Some(Slot::Promise(record.clone()));
    let promise = vm.create_from_new_target(&new_target, default, slot)?;
    let (resolve, reject) = vm.resolving_functions(&record)?;
    match vm.call_value(&executor, &Value::Undefined, &[resolve, reject.clone()]) {
        Err(Abrupt::Throw(error)) => {
            vm.call_value(&reject, &Value::Undefined, &[error])?;
        }
        other => {
            other?;
        }
    }
    Ok(object_value(promise))
}

/// The `this` constructor of a static method, which must be an object.
fn this_constructor(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Value> {
    if vm.as_object(this).is_none() {
        let message = format!("{method} called on a non-object");
        return vm.throw_type(&message);
    }
    Ok(this.clone())
}

fn promise_resolve(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let constructor = this_constructor(vm, this, "Promise.resolve")?;
    vm.promise_resolve(&constructor, arg(args, 0))
}

fn promise_reject(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let capability = vm.new_capability(this)?;
    vm.call_value(&capability.reject, &Value::Undefined, &[arg(args, 0)])?;
    Ok(capability.promise)
}

/// `Promise.try(callback, ...args)` (ES2025), with the 2026 change that returns a promise
/// result through PromiseResolve instead of wrapping it.
fn promise_try(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let constructor = this_constructor(vm, this, "Promise.try")?;
    let rest = args.get(1..).unwrap_or_default();
    match vm.call_function(&arg(args, 0), &Value::Undefined, rest) {
        Ok(value) => vm.promise_resolve(&constructor, value),
        Err(Abrupt::Throw(error)) => {
            vm.keep(&error);
            let capability = vm.new_capability(&constructor)?;
            vm.call_value(&capability.reject, &Value::Undefined, &[error])?;
            Ok(capability.promise)
        }
        Err(abrupt) => Err(abrupt),
    }
}

fn promise_with_resolvers(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let capability = vm.new_capability(this)?;
    let result = vm.new_object()?;
    for (name, value) in [
        ("promise", capability.promise),
        ("resolve", capability.resolve),
        ("reject", capability.reject),
    ] {
        let key = vm.realm.key_str(name);
        vm.create_data_property(result, key, value)?;
    }
    Ok(object_value(result))
}

/// `Promise.prototype.then(onFulfilled, onRejected)`.
fn promise_then(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let Some(record) = vm.promise_record(this) else {
        return vm.throw_type("Promise.prototype.then called on incompatible receiver");
    };
    let object = vm.as_object(this).expect("promises are objects");
    let constructor = vm.promise_species(object)?;
    let capability = vm.new_capability(&constructor)?;
    let promise = capability.promise.clone();
    vm.perform_then(&record, &arg(args, 0), &arg(args, 1), capability);
    Ok(promise)
}

fn promise_catch(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    vm.invoke(this, "then", &[Value::Undefined, arg(args, 0)])
}

/// `Promise.prototype.finally(onFinally)`.
fn promise_finally(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let object = vm.this_object_for(this, "Promise.prototype.finally")?;
    let constructor = vm.promise_species(object)?;
    vm.keep(&constructor);
    let on_finally = arg(args, 0);
    if !vm.is_callable(&on_finally) {
        return vm.invoke(this, "then", &[on_finally.clone(), on_finally]);
    }
    let make = |vm: &mut Interpreter, rejects| {
        let captures = Captures::Finally {
            on_finally: on_finally.clone(),
            constructor: constructor.clone(),
            rejects,
        };
        vm.promise_function(1, Rc::new(captures))
    };
    let then_finally = make(vm, false)?;
    let catch_finally = make(vm, true)?;
    vm.invoke(this, "then", &[then_finally, catch_finally])
}
