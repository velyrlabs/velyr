//! `ArrayBuffer` (resizable, `transfer`, detach) and `SharedArrayBuffer` (growable)
//! (ECMA-262 §25.1, §25.2).

use std::cell::RefCell;
use std::rc::Rc;

use super::super::abstract_ops::relative_index;
use super::super::array::species_getter;
use super::super::slots::Slot;
use super::super::symbol::SYMBOL_SPECIES;
use super::super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use super::store::{LocalBuffer, Store};
use crate::{ObjectKey, ObjectRef, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.new_object()?;
    let constructor = vm.constructor("ArrayBuffer", 1, array_buffer_constructor, proto)?;
    vm.set_intrinsic(Intrinsic::ArrayBufferConstructor, constructor);
    vm.set_intrinsic(Intrinsic::ArrayBufferPrototype, proto);
    vm.method(constructor, "isView", 1, is_view)?;
    install_species(vm, constructor)?;
    vm.getter(proto, "byteLength", byte_length)?;
    vm.getter(proto, "maxByteLength", max_byte_length)?;
    vm.getter(proto, "resizable", is_resizable)?;
    vm.getter(proto, "detached", is_detached)?;
    vm.method(proto, "resize", 1, resize)?;
    vm.method(proto, "slice", 2, slice)?;
    vm.method(proto, "transfer", 0, transfer)?;
    vm.method(proto, "transferToFixedLength", 0, transfer_to_fixed_length)?;
    vm.define_to_string_tag(proto, "ArrayBuffer")?;

    let proto = vm.new_object()?;
    let constructor = vm.constructor("SharedArrayBuffer", 1, shared_constructor, proto)?;
    vm.set_intrinsic(Intrinsic::SharedArrayBufferConstructor, constructor);
    vm.set_intrinsic(Intrinsic::SharedArrayBufferPrototype, proto);
    install_species(vm, constructor)?;
    vm.getter(proto, "byteLength", shared_byte_length)?;
    vm.getter(proto, "maxByteLength", shared_max_byte_length)?;
    vm.getter(proto, "growable", is_growable)?;
    vm.method(proto, "grow", 1, grow)?;
    vm.method(proto, "slice", 2, shared_slice)?;
    vm.define_to_string_tag(proto, "SharedArrayBuffer")
}

fn install_species(vm: &mut Interpreter, constructor: ObjectRef) -> JsResult<()> {
    let key = ObjectKey::Symbol(SYMBOL_SPECIES);
    vm.accessor(constructor, key, "[Symbol.species]", species_getter, None)
}

impl Interpreter {
    /// Accessor property with only a built-in getter.
    pub(crate) fn getter(
        &mut self,
        object: ObjectRef,
        name: &str,
        get: super::super::Builtin,
    ) -> JsResult<()> {
        let key = self.realm.key_str(name);
        self.accessor(object, key, name, get, None)
    }

    /// The store of `this` when it is an ArrayBuffer (`shared` false) or a
    /// SharedArrayBuffer (`shared` true).
    fn this_buffer(&mut self, this: &Value, shared: bool, method: &str) -> JsResult<Store> {
        match self.buffer_store(this) {
            Some(store) if matches!(store, Store::Shared(_)) == shared => Ok(store),
            _ => self.throw_type(&format!("{method} called on incompatible receiver")),
        }
    }

    fn this_local(&mut self, this: &Value, method: &str) -> JsResult<Rc<RefCell<LocalBuffer>>> {
        match self.this_buffer(this, false, method)? {
            Store::Local(buffer) => Ok(buffer),
            Store::Shared(_) => unreachable!("checked by this_buffer"),
        }
    }
}

/// Largest `maxByteLength` accepted (1 TiB); beyond it the reservation cannot be honored.
const MAX_BYTE_LENGTH: usize = 1 << 40;

fn construct(vm: &mut Interpreter, args: &[Value], shared: bool) -> JsResult<Value> {
    let new_target = vm.new_target();
    let name = if shared {
        "SharedArrayBuffer"
    } else {
        "ArrayBuffer"
    };
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type(&format!("Constructor {name} requires 'new'"));
    }
    let length = vm.coerce_index(&arg(args, 0))?;
    let max = match vm.as_object(&arg(args, 1)) {
        Some(options) => match vm.get_named(options, "maxByteLength")? {
            Value::Undefined => None,
            max => Some(vm.coerce_index(&max)?),
        },
        None => None,
    };
    if max.is_some_and(|max| max > MAX_BYTE_LENGTH) {
        return vm.throw_range("Array buffer allocation failed");
    }
    if max.is_some_and(|max| length > max) {
        return vm.throw_range(&format!("{name} length exceeds maxByteLength"));
    }
    let default = match shared {
        true => Intrinsic::SharedArrayBufferPrototype,
        false => Intrinsic::ArrayBufferPrototype,
    };
    let default = vm.intrinsic(default);
    let proto = vm.proto_from_constructor(&new_target, default)?;
    Ok(object_value(vm.create_buffer(proto, length, max, shared)?))
}

fn array_buffer_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    construct(vm, args, false)
}

fn shared_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    construct(vm, args, true)
}

fn is_view(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let slot = vm.value_slot(&arg(args, 0));
    Ok(Value::Bool(matches!(
        slot,
        Some(Slot::TypedArray(_) | Slot::DataView(_))
    )))
}

fn byte_length(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let store = vm.this_buffer(this, false, "get ArrayBuffer.prototype.byteLength")?;
    Ok(Value::Number(store.byte_length() as f64))
}

fn max_byte_length(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let store = vm.this_buffer(this, false, "get ArrayBuffer.prototype.maxByteLength")?;
    let max = store.max().filter(|_| !store.is_detached());
    Ok(Value::Number(max.unwrap_or(store.byte_length()) as f64))
}

fn is_resizable(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let store = vm.this_buffer(this, false, "get ArrayBuffer.prototype.resizable")?;
    Ok(Value::Bool(store.max().is_some()))
}

fn is_detached(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let store = vm.this_buffer(this, false, "get ArrayBuffer.prototype.detached")?;
    Ok(Value::Bool(store.is_detached()))
}

fn resize(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let buffer = vm.this_local(this, "ArrayBuffer.prototype.resize")?;
    let Some(max) = buffer.borrow().max else {
        return vm.throw_type("ArrayBuffer.prototype.resize: the buffer is not resizable");
    };
    let length = vm.coerce_index(&arg(args, 0))?;
    let mut buffer = buffer.borrow_mut();
    let Some(block) = buffer.block.as_mut() else {
        return vm.throw_type("ArrayBuffer.prototype.resize: the buffer is detached");
    };
    if length > max {
        return vm.throw_range("ArrayBuffer.prototype.resize: length exceeds maxByteLength");
    }
    if !block.resize(length) {
        return vm.throw_range("Array buffer allocation failed");
    }
    Ok(Value::Undefined)
}

/// ArrayBufferCopyAndDetach: moves the bytes into a new buffer and detaches `this`.
fn copy_and_detach(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    keep_resizable: bool,
    method: &str,
) -> JsResult<Value> {
    let buffer = vm.this_local(this, method)?;
    let length = match arg(args, 0) {
        Value::Undefined => buffer.borrow().block.as_ref().map_or(0, |b| b.bytes.len()),
        length => vm.coerce_index(&length)?,
    };
    let max = buffer.borrow().max.filter(|_| keep_resizable);
    if buffer.borrow().block.is_none() {
        return vm.throw_type(&format!("{method}: the buffer is detached"));
    }
    if max.is_some_and(|max| length > max) {
        return vm.throw_range(&format!("{method}: length exceeds maxByteLength"));
    }
    let proto = vm.intrinsic(Intrinsic::ArrayBufferPrototype);
    let target = vm.create_buffer(proto, 0, max, false)?;
    let Some(Store::Local(new_buffer)) = vm.buffer_store(&object_value(target)) else {
        return Err(vm.internal("fresh ArrayBuffer"));
    };
    let mut block = buffer.borrow_mut().block.take().expect("checked above");
    if !block.resize(length) {
        buffer.borrow_mut().block = Some(block);
        return vm.throw_range("Array buffer allocation failed");
    }
    new_buffer.borrow_mut().block = Some(block);
    Ok(object_value(target))
}

fn transfer(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    copy_and_detach(vm, this, args, true, "ArrayBuffer.prototype.transfer")
}

fn transfer_to_fixed_length(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let method = "ArrayBuffer.prototype.transferToFixedLength";
    copy_and_detach(vm, this, args, false, method)
}

/// `ArrayBuffer.prototype.slice` and `SharedArrayBuffer.prototype.slice`.
fn slice_buffer(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    shared: bool,
) -> JsResult<Value> {
    let method = match shared {
        true => "SharedArrayBuffer.prototype.slice",
        false => "ArrayBuffer.prototype.slice",
    };
    let store = vm.this_buffer(this, shared, method)?;
    if store.is_detached() {
        return vm.throw_type(&format!("{method}: the buffer is detached"));
    }
    let length = store.byte_length() as f64;
    let first = relative_index(vm.coerce_integer(&arg(args, 0))?, length);
    let end = match arg(args, 1) {
        Value::Undefined => length,
        end => relative_index(vm.coerce_integer(&end)?, length),
    };
    let new_length = (end - first).max(0.0);
    let default = match shared {
        true => Intrinsic::SharedArrayBufferConstructor,
        false => Intrinsic::ArrayBufferConstructor,
    };
    let default = object_value(vm.intrinsic(default));
    let object = vm.as_object(this).expect("buffers are objects");
    let constructor = vm.species_constructor(object, default)?;
    let result = vm.construct_value(&constructor, &[Value::Number(new_length)], &constructor)?;
    vm.keep(&result);
    let target = match vm.buffer_store(&result) {
        Some(target) if matches!(target, Store::Shared(_)) == shared => target,
        _ => {
            return vm.throw_type(&format!(
                "{method}: species constructor did not return a buffer"
            ));
        }
    };
    let same = match (&store, &target) {
        (Store::Local(a), Store::Local(b)) => Rc::ptr_eq(a, b),
        (Store::Shared(a), Store::Shared(b)) => std::sync::Arc::ptr_eq(a, b),
        _ => false,
    };
    if target.is_detached() || same || (target.byte_length() as f64) < new_length {
        return vm.throw_type(&format!(
            "{method}: species constructor returned an unusable buffer"
        ));
    }
    if store.is_detached() {
        return vm.throw_type(&format!("{method}: the buffer is detached"));
    }
    let (first, count) = (first as usize, new_length as usize);
    let bytes = store.with_bytes(|bytes| {
        let end = (first + count).min(bytes.len());
        bytes
            .get(first..end)
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
    });
    target.with_bytes(|dst| dst[..bytes.len()].copy_from_slice(&bytes));
    Ok(result)
}

fn slice(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    slice_buffer(vm, this, args, false)
}

fn shared_slice(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    slice_buffer(vm, this, args, true)
}

fn shared_byte_length(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let store = vm.this_buffer(this, true, "get SharedArrayBuffer.prototype.byteLength")?;
    Ok(Value::Number(store.byte_length() as f64))
}

fn shared_max_byte_length(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let method = "get SharedArrayBuffer.prototype.maxByteLength";
    let store = vm.this_buffer(this, true, method)?;
    Ok(Value::Number(
        store.max().unwrap_or(store.byte_length()) as f64
    ))
}

fn is_growable(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let store = vm.this_buffer(this, true, "get SharedArrayBuffer.prototype.growable")?;
    Ok(Value::Bool(store.max().is_some()))
}

fn grow(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let Store::Shared(buffer) = vm.this_buffer(this, true, "SharedArrayBuffer.prototype.grow")?
    else {
        unreachable!("checked by this_buffer");
    };
    let Some(max) = buffer.max else {
        return vm.throw_type("SharedArrayBuffer.prototype.grow: the buffer is not growable");
    };
    let length = vm.coerce_index(&arg(args, 0))?;
    let mut state = buffer.lock();
    if length > max || length < state.block.bytes.len() {
        return vm.throw_range("SharedArrayBuffer.prototype.grow: invalid length");
    }
    if !state.block.resize(length) {
        return vm.throw_range("Array buffer allocation failed");
    }
    Ok(Value::Undefined)
}
