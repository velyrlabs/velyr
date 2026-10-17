//! The `Atomics` namespace (ECMA-262 §25.4). On a SharedArrayBuffer every operation runs
//! under the buffer's mutex, so a read-modify-write is atomic across threads; `wait`
//! blocks only where the embedder allowed it (the native engine allows it everywhere).

use std::rc::Rc;
use std::time::Duration;

use super::super::abstract_ops::integer_part;
use super::super::bigint::to_bigint;
use super::super::{Builtin, Interpreter, JsResult, arg, object_value};
use super::TypedArray;
use super::elements::{ElementKind, read_raw, write_raw};
use super::store::{Store, WaitResult};
use crate::Value;

const NATIVE_LITTLE_ENDIAN: bool = cfg!(target_endian = "little");

const FUNCTIONS: [(&str, u32, Builtin); 13] = [
    ("add", 3, add),
    ("and", 3, and),
    ("compareExchange", 4, compare_exchange),
    ("exchange", 3, exchange),
    ("isLockFree", 1, is_lock_free),
    ("load", 2, load),
    ("notify", 3, notify),
    ("or", 3, or),
    ("pause", 0, pause),
    ("store", 3, store),
    ("sub", 3, sub),
    ("wait", 4, wait),
    ("xor", 3, xor),
];

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let atomics = vm.new_object()?;
    for (name, arity, f) in FUNCTIONS {
        vm.method(atomics, name, arity, f)?;
    }
    vm.define_to_string_tag(atomics, "Atomics")?;
    let global = vm.state.intrinsics.global;
    vm.define_method(global, "Atomics", object_value(atomics))
}

/// ValidateIntegerTypedArray.
fn validate(vm: &mut Interpreter, value: &Value, waitable: bool) -> JsResult<Rc<TypedArray>> {
    let (array, _) = vm.validate_typed_array(value, "Atomics")?;
    let accepted = match waitable {
        true => matches!(array.kind, ElementKind::Int32 | ElementKind::BigInt64),
        false => array.kind.is_atomic(),
    };
    if !accepted {
        return vm.throw_type(&format!("Atomics does not support {}", array.kind.name()));
    }
    Ok(array)
}

/// ValidateAtomicAccess, also used to revalidate after the conversions ran.
fn check_index(vm: &mut Interpreter, array: &TypedArray, index: usize) -> JsResult<usize> {
    let Some(length) = array.length() else {
        return vm.throw_type("Atomics: the typed array is detached or out of bounds");
    };
    if index >= length {
        return vm.throw_range("Atomics: index is out of range");
    }
    Ok(array.view.byte_offset + index * array.kind.size())
}

/// The operand as an element: ToBigInt, or ToIntegerOrInfinity for Number elements.
fn operand(vm: &mut Interpreter, kind: ElementKind, value: &Value) -> JsResult<Value> {
    match kind.is_bigint() {
        true => Ok(Value::BigInt(to_bigint(vm, value)?)),
        false => Ok(Value::Number(integer_part(vm.coerce_number(value)?))),
    }
}

/// Replaces the element at `start` with `f(old)` in one locked step; returns `old`.
fn update(array: &TypedArray, start: usize, f: impl FnOnce(u64) -> u64) -> u64 {
    let size = array.kind.size();
    array.view.store.with_bytes(|bytes| {
        let element = &mut bytes[start..start + size];
        let old = read_raw(element, NATIVE_LITTLE_ENDIAN);
        write_raw(element, array.kind.wrap(f(old)), NATIVE_LITTLE_ENDIAN);
        old
    })
}

/// AtomicReadModifyWrite.
fn read_modify_write(
    vm: &mut Interpreter,
    args: &[Value],
    op: fn(u64, u64) -> u64,
) -> JsResult<Value> {
    let array = validate(vm, &arg(args, 0), false)?;
    let index = vm.coerce_index(&arg(args, 1))?;
    check_index(vm, &array, index)?;
    let value = operand(vm, array.kind, &arg(args, 2))?;
    let start = check_index(vm, &array, index)?;
    let operand = array.kind.wrap(array.kind.encode(&value));
    let old = update(&array, start, |old| op(old, operand));
    Ok(array.kind.decode(old))
}

fn add(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(vm, args, u64::wrapping_add)
}

fn sub(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(vm, args, u64::wrapping_sub)
}

fn and(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(vm, args, |a, b| a & b)
}

fn or(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(vm, args, |a, b| a | b)
}

fn xor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(vm, args, |a, b| a ^ b)
}

fn exchange(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    read_modify_write(vm, args, |_, b| b)
}

fn compare_exchange(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let array = validate(vm, &arg(args, 0), false)?;
    let index = vm.coerce_index(&arg(args, 1))?;
    check_index(vm, &array, index)?;
    let expected = operand(vm, array.kind, &arg(args, 2))?;
    let replacement = operand(vm, array.kind, &arg(args, 3))?;
    let start = check_index(vm, &array, index)?;
    let expected = array.kind.wrap(array.kind.encode(&expected));
    let replacement = array.kind.encode(&replacement);
    let old = update(&array, start, |old| {
        if old == expected { replacement } else { old }
    });
    Ok(array.kind.decode(old))
}

fn load(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let array = validate(vm, &arg(args, 0), false)?;
    let index = vm.coerce_index(&arg(args, 1))?;
    let start = check_index(vm, &array, index)?;
    let old = update(&array, start, |old| old);
    Ok(array.kind.decode(old))
}

fn store(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let array = validate(vm, &arg(args, 0), false)?;
    let index = vm.coerce_index(&arg(args, 1))?;
    check_index(vm, &array, index)?;
    let value = operand(vm, array.kind, &arg(args, 2))?;
    let start = check_index(vm, &array, index)?;
    let raw = array.kind.encode(&value);
    update(&array, start, |_| raw);
    Ok(value)
}

fn is_lock_free(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let size = vm.coerce_integer(&arg(args, 0))?;
    Ok(Value::Bool(
        matches!(size as i64, 1 | 2 | 4 | 8) && size.fract() == 0.0,
    ))
}

fn notify(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let array = validate(vm, &arg(args, 0), true)?;
    let index = vm.coerce_index(&arg(args, 1))?;
    check_index(vm, &array, index)?;
    let count = match arg(args, 2) {
        Value::Undefined => usize::MAX,
        count => vm.coerce_integer(&count)?.max(0.0) as usize,
    };
    let start = check_index(vm, &array, index)?;
    let woken = match &array.view.store {
        Store::Shared(buffer) => buffer.notify(start, count),
        Store::Local(_) => 0,
    };
    Ok(Value::Number(woken as f64))
}

/// `Atomics.pause([N])`: a spin-loop hint, so N only bounds the iterations (at most 1000).
fn pause(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    match arg(args, 0) {
        Value::Undefined => {}
        Value::Number(n) if n.is_finite() && n.fract() == 0.0 => {
            (0..n.clamp(0.0, 1000.0) as u32).for_each(|_| std::hint::spin_loop());
        }
        _ => {
            return vm.throw_type("Atomics.pause argument must be undefined or an integral Number");
        }
    }
    Ok(Value::Undefined)
}

fn wait(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let array = validate(vm, &arg(args, 0), true)?;
    let Store::Shared(buffer) = array.view.store.clone() else {
        return vm.throw_type("Atomics.wait needs a typed array on a SharedArrayBuffer");
    };
    let index = vm.coerce_index(&arg(args, 1))?;
    check_index(vm, &array, index)?;
    let value = vm.element_value(array.kind, &arg(args, 2))?;
    let timeout = match vm.coerce_number(&arg(args, 3))? {
        ms if ms.is_nan() => None,
        // Past `Duration`'s range (or +Infinity) is the same as no timeout.
        ms => Duration::try_from_secs_f64(ms.max(0.0) / 1000.0).ok(),
    };
    if !vm.atomics_wait_allowed {
        return vm.throw_type("Atomics.wait cannot be called in this context");
    }
    let start = check_index(vm, &array, index)?;
    let expected = array.kind.wrap(array.kind.encode(&value));
    let size = array.kind.size();
    let holds =
        |bytes: &[u8]| read_raw(&bytes[start..start + size], NATIVE_LITTLE_ENDIAN) == expected;
    let result = match buffer.wait(start, holds, timeout) {
        WaitResult::Ok => "ok",
        WaitResult::NotEqual => "not-equal",
        WaitResult::TimedOut => "timed-out",
    };
    Ok(Value::str(result))
}
