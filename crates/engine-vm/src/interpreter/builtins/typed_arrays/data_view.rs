//! `DataView` (ECMA-262 §25.3): element reads and writes at any byte offset, big-endian
//! unless `littleEndian` is passed.

use std::rc::Rc;

use super::super::slots::Slot;
use super::super::{Builtin, Interpreter, Intrinsic, JsResult, arg, object_value};
use super::elements::{ElementKind, read_raw, write_raw};
use super::store::View;
use crate::{Value, to_boolean};

macro_rules! accessors {
    ($($get:ident, $set:ident => $kind:ident;)*) => {
        $(fn $get(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
            get_view_value(vm, this, args, ElementKind::$kind)
        }
        fn $set(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
            set_view_value(vm, this, args, ElementKind::$kind)
        })*

        const ACCESSORS: &[(ElementKind, Builtin, Builtin)] = &[
            $((ElementKind::$kind, $get, $set),)*
        ];
    };
}

accessors! {
    get_int8, set_int8 => Int8;
    get_uint8, set_uint8 => Uint8;
    get_int16, set_int16 => Int16;
    get_uint16, set_uint16 => Uint16;
    get_int32, set_int32 => Int32;
    get_uint32, set_uint32 => Uint32;
    get_float16, set_float16 => Float16;
    get_float32, set_float32 => Float32;
    get_float64, set_float64 => Float64;
    get_big_int64, set_big_int64 => BigInt64;
    get_big_uint64, set_big_uint64 => BigUint64;
}

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.new_object()?;
    vm.constructor("DataView", 1, data_view_constructor, proto)?;
    vm.set_intrinsic(Intrinsic::DataViewPrototype, proto);
    vm.getter(proto, "buffer", buffer)?;
    vm.getter(proto, "byteLength", byte_length)?;
    vm.getter(proto, "byteOffset", byte_offset)?;
    for (kind, get, set) in ACCESSORS {
        // `Int8Array` -> `Int8`.
        let name = kind.name().trim_end_matches("Array");
        vm.method(proto, &format!("get{name}"), 1, *get)?;
        vm.method(proto, &format!("set{name}"), 2, *set)?;
    }
    vm.define_to_string_tag(proto, "DataView")
}

fn data_view_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type("Constructor DataView requires 'new'");
    }
    let buffer = arg(args, 0);
    let Some(store) = vm.buffer_store(&buffer) else {
        return vm.throw_type("First argument to DataView constructor must be an ArrayBuffer");
    };
    let offset = vm.coerce_index(&arg(args, 1))?;
    let check_bounds = |vm: &mut Interpreter, fixed: Option<usize>| -> JsResult<()> {
        if store.is_detached() {
            return vm.throw_type("Cannot construct a DataView on a detached ArrayBuffer");
        }
        let buffer_length = store.byte_length();
        let fits = offset <= buffer_length
            && fixed.is_none_or(|length| {
                offset
                    .checked_add(length)
                    .is_some_and(|end| end <= buffer_length)
            });
        match fits {
            true => Ok(()),
            false => vm.throw_range("Invalid DataView offset or length"),
        }
    };
    check_bounds(vm, None)?;
    let fixed = match arg(args, 2) {
        Value::Undefined if store.max().is_some() => None,
        Value::Undefined => Some(store.byte_length() - offset),
        length => Some(vm.coerce_index(&length)?),
    };
    check_bounds(vm, fixed)?;
    let default = vm.intrinsic(Intrinsic::DataViewPrototype);
    let proto = vm.proto_from_constructor(&new_target, default)?;
    // GetPrototypeFromConstructor may run JS that detaches or shrinks the buffer.
    check_bounds(vm, fixed)?;
    let view = View {
        buffer: vm.as_object(&buffer).expect("buffers are objects"),
        store: store.clone(),
        byte_offset: offset,
        fixed,
    };
    let object = vm.new_slotted(proto, Slot::DataView(Rc::new(view)))?;
    Ok(object_value(object))
}

fn this_view(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Rc<View>> {
    match vm.value_slot(this) {
        Some(Slot::DataView(view)) => Ok(view),
        _ => vm.throw_type(&format!("{method} called on incompatible receiver")),
    }
}

/// The view's byte length, or a TypeError when detached or out of bounds.
fn view_length(vm: &mut Interpreter, view: &View, method: &str) -> JsResult<usize> {
    match view.length(1) {
        Some(length) => Ok(length),
        None => vm.throw_type(&format!(
            "{method}: the DataView is detached or out of bounds"
        )),
    }
}

fn buffer(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let view = this_view(vm, this, "get DataView.prototype.buffer")?;
    Ok(object_value(view.buffer))
}

fn byte_length(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let method = "get DataView.prototype.byteLength";
    let view = this_view(vm, this, method)?;
    Ok(Value::Number(view_length(vm, &view, method)? as f64))
}

fn byte_offset(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let method = "get DataView.prototype.byteOffset";
    let view = this_view(vm, this, method)?;
    view_length(vm, &view, method)?;
    Ok(Value::Number(view.byte_offset as f64))
}

/// The byte position of a `size`-byte access at `index` (checked against the view).
fn access_start(
    vm: &mut Interpreter,
    view: &View,
    index: usize,
    size: usize,
    method: &str,
) -> JsResult<usize> {
    let length = view_length(vm, view, method)?;
    if index.checked_add(size).is_none_or(|end| end > length) {
        return vm.throw_range("Offset is outside the bounds of the DataView");
    }
    Ok(view.byte_offset + index)
}

/// GetViewValue.
fn get_view_value(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    kind: ElementKind,
) -> JsResult<Value> {
    let method = "DataView.prototype.get";
    let view = this_view(vm, this, method)?;
    let index = vm.coerce_index(&arg(args, 0))?;
    let little_endian = to_boolean(&arg(args, 1));
    let size = kind.size();
    let start = access_start(vm, &view, index, size, method)?;
    let raw = view
        .store
        .with_bytes(|bytes| read_raw(&bytes[start..start + size], little_endian));
    Ok(kind.decode(raw))
}

/// SetViewValue.
fn set_view_value(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    kind: ElementKind,
) -> JsResult<Value> {
    let method = "DataView.prototype.set";
    let view = this_view(vm, this, method)?;
    let index = vm.coerce_index(&arg(args, 0))?;
    let value = vm.element_value(kind, &arg(args, 1))?;
    let little_endian = to_boolean(&arg(args, 2));
    let size = kind.size();
    let start = access_start(vm, &view, index, size, method)?;
    let raw = kind.encode(&value);
    view.store
        .with_bytes(|bytes| write_raw(&mut bytes[start..start + size], raw, little_endian));
    Ok(Value::Undefined)
}
