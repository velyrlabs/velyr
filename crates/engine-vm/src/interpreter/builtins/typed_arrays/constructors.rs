//! `%TypedArray%`, its `from`/`of` statics and the twelve concrete constructors
//! (ECMA-262 §23.2.1-23.2.7), plus TypedArraySpeciesCreate for the prototype methods.

use std::rc::Rc;

use super::super::array::species_getter;
use super::super::symbol::{SYMBOL_ITERATOR, SYMBOL_SPECIES};
use super::super::{Builtin, Interpreter, Intrinsic, JsResult, arg, object_value};
use super::store::Store;
use super::{ElementKind, TypedArray, methods};
use crate::{ObjectKey, ObjectRef, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.new_object()?;
    // %TypedArray% is not a global, so `vm.constructor` does not fit.
    let abstract_constructor: Builtin = typed_array_constructor;
    let constructor =
        vm.create_native_function("TypedArray", 0, Rc::new(abstract_constructor), true)?;
    vm.keep(&object_value(constructor));
    vm.constant(constructor, "prototype", object_value(proto))?;
    vm.define_method(proto, "constructor", object_value(constructor))?;
    vm.set_intrinsic(Intrinsic::TypedArrayPrototype, proto);
    vm.method(constructor, "from", 1, typed_array_from)?;
    vm.method(constructor, "of", 0, typed_array_of)?;
    let key = ObjectKey::Symbol(SYMBOL_SPECIES);
    vm.accessor(constructor, key, "[Symbol.species]", species_getter, None)?;
    methods::install(vm, proto)?;
    for kind in ElementKind::ALL {
        let kind_proto = vm.new_object_with(Some(proto))?;
        let kind_constructor = vm.constructor(kind.name(), 3, constructor_fn(kind), kind_proto)?;
        vm.set_prototype(kind_constructor, Some(constructor))?;
        let size = Value::Number(kind.size() as f64);
        vm.constant(kind_constructor, "BYTES_PER_ELEMENT", size.clone())?;
        vm.constant(kind_proto, "BYTES_PER_ELEMENT", size)?;
        vm.set_intrinsic(kind.constructor(), kind_constructor);
    }
    Ok(())
}

macro_rules! typed_constructors {
    ($($name:ident => $kind:ident),* $(,)?) => {
        $(fn $name(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
            construct(vm, ElementKind::$kind, args)
        })*

        fn constructor_fn(kind: ElementKind) -> Builtin {
            match kind {
                $(ElementKind::$kind => $name,)*
            }
        }
    };
}

typed_constructors! {
    int8_array => Int8,
    uint8_array => Uint8,
    uint8_clamped_array => Uint8Clamped,
    int16_array => Int16,
    uint16_array => Uint16,
    int32_array => Int32,
    uint32_array => Uint32,
    float16_array => Float16,
    float32_array => Float32,
    float64_array => Float64,
    big_int64_array => BigInt64,
    big_uint64_array => BigUint64,
}

fn typed_array_constructor(vm: &mut Interpreter, _: &Value, _: &[Value]) -> JsResult<Value> {
    vm.throw_type("Abstract class TypedArray not directly constructable")
}

fn construct(vm: &mut Interpreter, kind: ElementKind, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type(&format!("Constructor {} requires 'new'", kind.name()));
    }
    let constructor = vm.intrinsic(kind.constructor());
    let default = vm.get_named(constructor, "prototype")?;
    let default = vm
        .as_object(&default)
        .expect("constructor prototypes are objects");
    let first = arg(args, 0);
    let Some(object) = vm.as_object(&first) else {
        let length = vm.coerce_index(&first)?;
        let proto = vm.proto_from_constructor(&new_target, default)?;
        return allocate(vm, proto, kind, length).map(|(array, _)| object_value(array));
    };
    let proto = vm.proto_from_constructor(&new_target, default)?;
    if let Some(store) = vm.buffer_store(&first) {
        return from_buffer(vm, proto, kind, object, &store, args);
    }
    let array = match vm.typed_array_of(&first) {
        Some(source) => from_typed_array(vm, proto, kind, &source)?,
        None => match vm.get_method(&first, ObjectKey::Symbol(SYMBOL_ITERATOR))? {
            Some(method) => {
                let values = iterated_values(vm, &first, &method)?;
                let (array, typed) = allocate(vm, proto, kind, values.len())?;
                for (index, value) in values.iter().enumerate() {
                    vm.typed_set(&typed, Some(index), value)?;
                }
                array
            }
            None => from_array_like(vm, proto, kind, object)?,
        },
    };
    Ok(object_value(array))
}

/// InitializeTypedArrayFromArrayLike: allocates first, so a huge `length` is a
/// RangeError from the buffer budget before any element is read.
fn from_array_like(
    vm: &mut Interpreter,
    proto: ObjectRef,
    kind: ElementKind,
    source: ObjectRef,
) -> JsResult<ObjectRef> {
    let length = vm.length_of(source)? as usize;
    let (array, typed) = allocate(vm, proto, kind, length)?;
    vm.keep(&object_value(array));
    for index in 0..length {
        let key = vm.index_key(index as f64);
        let value = vm.get_key(source, key)?;
        vm.typed_set(&typed, Some(index), &value)?;
    }
    Ok(array)
}

/// AllocateTypedArray with a fresh zeroed buffer of `length` elements.
fn allocate(
    vm: &mut Interpreter,
    proto: ObjectRef,
    kind: ElementKind,
    length: usize,
) -> JsResult<(ObjectRef, Rc<TypedArray>)> {
    let Some(byte_length) = length.checked_mul(kind.size()) else {
        return vm.throw_range("Invalid typed array length");
    };
    let buffer_proto = vm.intrinsic(Intrinsic::ArrayBufferPrototype);
    let buffer = vm.create_buffer(buffer_proto, byte_length, None, false)?;
    let array = vm.create_typed_array(proto, kind, buffer, 0, Some(length))?;
    let typed = vm
        .typed_array_of(&object_value(array))
        .expect("fresh typed array");
    Ok((array, typed))
}

/// InitializeTypedArrayFromArrayBuffer.
fn from_buffer(
    vm: &mut Interpreter,
    proto: ObjectRef,
    kind: ElementKind,
    buffer: ObjectRef,
    store: &Store,
    args: &[Value],
) -> JsResult<Value> {
    let size = kind.size();
    let offset = vm.coerce_index(&arg(args, 1))?;
    if offset % size != 0 {
        let message = format!(
            "start offset of {} should be a multiple of {size}",
            kind.name()
        );
        return vm.throw_range(&message);
    }
    let length = match arg(args, 2) {
        Value::Undefined => None,
        length => Some(vm.coerce_index(&length)?),
    };
    if store.is_detached() {
        return vm.throw_type("Cannot construct a typed array on a detached ArrayBuffer");
    }
    let buffer_length = store.byte_length();
    let fixed = match length {
        None if store.max().is_some() && offset <= buffer_length => None,
        None if !buffer_length.is_multiple_of(size) => {
            let message = format!(
                "byte length of {} should be a multiple of {size}",
                kind.name()
            );
            return vm.throw_range(&message);
        }
        None if offset <= buffer_length => Some((buffer_length - offset) / size),
        Some(length)
            if length
                .checked_mul(size)
                .and_then(|bytes| bytes.checked_add(offset))
                .is_some_and(|end| end <= buffer_length) =>
        {
            Some(length)
        }
        _ => return vm.throw_range(&format!("Invalid typed array length: {}", kind.name())),
    };
    let array = vm.create_typed_array(proto, kind, buffer, offset, fixed)?;
    Ok(object_value(array))
}

/// InitializeTypedArrayFromTypedArray.
fn from_typed_array(
    vm: &mut Interpreter,
    proto: ObjectRef,
    kind: ElementKind,
    source: &TypedArray,
) -> JsResult<ObjectRef> {
    let Some(length) = source.length() else {
        return vm.throw_type("Cannot construct from a detached or out of bounds typed array");
    };
    if source.kind.is_bigint() != kind.is_bigint() {
        return vm.throw_type("Cannot mix BigInt and other types, use explicit conversions");
    }
    let (array, target) = allocate(vm, proto, kind, length)?;
    copy_elements(source, &target, 0, length);
    Ok(array)
}

/// Copies `count` elements of `source` to `target` from `target_offset`, converting
/// between element types of the same content type.
pub(super) fn copy_elements(
    source: &TypedArray,
    target: &TypedArray,
    target_offset: usize,
    count: usize,
) {
    let values: Vec<u64> = (0..count).map(|index| source.load_raw(index)).collect();
    for (index, raw) in values.into_iter().enumerate() {
        let raw = match source.kind == target.kind {
            true => raw,
            false => target.kind.encode(&source.kind.decode(raw)),
        };
        target.store_raw(target_offset + index, raw);
    }
}

/// The values of an iterable, or of an array-like when it has no `@@iterator`.
pub(super) fn values_of(vm: &mut Interpreter, source: &Value) -> JsResult<Vec<Value>> {
    if let Some(method) = vm.get_method(source, ObjectKey::Symbol(SYMBOL_ITERATOR))? {
        return iterated_values(vm, source, &method);
    }
    let object = vm.coerce_object(source)?;
    vm.list_from_array_like(&object_value(object))
}

fn iterated_values(vm: &mut Interpreter, source: &Value, method: &Value) -> JsResult<Vec<Value>> {
    let mut values = Vec::new();
    let (iterator, next) = vm.open_iterator_with(source, method)?;
    vm.drain_iterator(&iterator, &next, &mut |vm, value| {
        // Kept so a collection in the next `next()` cannot free it; the list and its
        // roots live outside the heap, so an endless iterator is checked as it grows.
        vm.keep(&value);
        values.push(value);
        if values.len().is_multiple_of(LIST_CHECK_INTERVAL) {
            vm.reserve_heap(values.len() * 2 * size_of::<Value>())?;
        }
        Ok(())
    })?;
    Ok(values)
}

/// Iterated values between checks of the heap limit.
const LIST_CHECK_INTERVAL: usize = 1024;

/// TypedArrayCreateFromConstructor: constructs and validates the result.
pub(super) fn create_from_constructor(
    vm: &mut Interpreter,
    constructor: &Value,
    args: &[Value],
) -> JsResult<(ObjectRef, Rc<TypedArray>)> {
    let result = vm.construct_value(constructor, args, constructor)?;
    vm.keep(&result);
    let (typed, length) = vm.validate_typed_array(&result, "TypedArray constructor result")?;
    if let [Value::Number(requested)] = args
        && (length as f64) < *requested
    {
        return vm.throw_type("TypedArray constructor returned a too short typed array");
    }
    Ok((
        vm.as_object(&result).expect("typed arrays are objects"),
        typed,
    ))
}

/// TypedArraySpeciesCreate.
pub(super) fn species_create(
    vm: &mut Interpreter,
    exemplar: &Value,
    kind: ElementKind,
    args: &[Value],
) -> JsResult<(ObjectRef, Rc<TypedArray>)> {
    let default = object_value(vm.intrinsic(kind.constructor()));
    let object = vm.as_object(exemplar).expect("validated typed array");
    let constructor = vm.species_constructor(object, default)?;
    let created = create_from_constructor(vm, &constructor, args)?;
    if created.1.kind.is_bigint() != kind.is_bigint() {
        return vm.throw_type("Cannot mix BigInt and other types, use explicit conversions");
    }
    Ok(created)
}

fn this_constructor(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<()> {
    match vm.is_constructor(this) {
        true => Ok(()),
        false => vm.throw_type(&format!("{method}: this is not a constructor")),
    }
}

fn typed_array_from(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    this_constructor(vm, this, "TypedArray.from")?;
    let mapper = arg(args, 1);
    let this_arg = arg(args, 2);
    let mapping = !matches!(mapper, Value::Undefined);
    if mapping && !vm.is_callable(&mapper) {
        return vm
            .throw_type("TypedArray.from: when provided, the second argument must be a function");
    }
    let values = values_of(vm, &arg(args, 0))?;
    for value in &values {
        vm.keep(value);
    }
    let length = Value::Number(values.len() as f64);
    let (target, _) = create_from_constructor(vm, this, &[length])?;
    for (index, value) in values.into_iter().enumerate() {
        let value = match mapping {
            true => vm.call_value(&mapper, &this_arg, &[value, Value::Number(index as f64)])?,
            false => value,
        };
        let key = vm.index_key(index as f64);
        vm.set_or_throw(target, key, value)?;
    }
    Ok(object_value(target))
}

fn typed_array_of(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    this_constructor(vm, this, "TypedArray.of")?;
    let length = Value::Number(args.len() as f64);
    let (target, _) = create_from_constructor(vm, this, &[length])?;
    for (index, value) in args.iter().enumerate() {
        let key = vm.index_key(index as f64);
        vm.set_or_throw(target, key, value.clone())?;
    }
    Ok(object_value(target))
}
