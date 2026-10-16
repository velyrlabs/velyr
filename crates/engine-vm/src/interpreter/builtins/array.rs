//! The `Array` constructor, its statics and the array helpers the prototype methods share
//! (ECMA-262 §23.1). The prototype methods live in the submodules.

// Visible to the TypedArray methods that reuse the generic algorithms.
pub(super) mod access;
pub(super) mod callbacks;
pub(super) mod mutators;
pub(super) mod reorder;

use super::abstract_ops::MAX_SAFE_INTEGER;
use super::slots::{IterationKind, Slot};
use super::symbol::{SYMBOL_ITERATOR, SYMBOL_SPECIES, SYMBOL_UNSCOPABLES};
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::interpreter::operators::to_uint32;
use crate::{ObjectKey, ObjectRef, PropertyDescriptor, Value, same_value_zero};

const MAX_ARRAY_LENGTH: f64 = 4_294_967_295.0;

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.state.intrinsics.array_prototype;
    let constructor = vm.constructor("Array", 1, array_constructor, proto)?;
    vm.set_intrinsic(Intrinsic::ArrayConstructor, constructor);
    vm.method(constructor, "from", 1, array_from)?;
    vm.method(constructor, "isArray", 1, array_is_array)?;
    vm.method(constructor, "of", 0, array_of)?;
    let key = ObjectKey::Symbol(SYMBOL_SPECIES);
    vm.accessor(constructor, key, "[Symbol.species]", species_getter, None)?;
    access::install(vm, proto)?;
    callbacks::install(vm, proto)?;
    mutators::install(vm, proto)?;
    reorder::install(vm, proto)?;
    let values = vm.get_named(proto, "values")?;
    let desc = PropertyDescriptor::data(values.clone(), true, false, true);
    vm.define_property(proto, ObjectKey::Symbol(SYMBOL_ITERATOR), &desc)?;
    let values = vm.as_object(&values).expect("values is a function");
    vm.set_intrinsic(Intrinsic::ArrayPrototypeValues, values);
    install_unscopables(vm, proto)
}

fn install_unscopables(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    let list = vm.new_object_with(None)?;
    for name in [
        "at",
        "copyWithin",
        "entries",
        "fill",
        "find",
        "findIndex",
        "findLast",
        "findLastIndex",
        "flat",
        "flatMap",
        "includes",
        "keys",
        "toReversed",
        "toSorted",
        "toSpliced",
        "values",
    ] {
        let key = vm.realm.key_str(name);
        vm.create_data_property(list, key, Value::Bool(true))?;
    }
    let desc = PropertyDescriptor::data(object_value(list), false, false, true);
    vm.define_property(proto, ObjectKey::Symbol(SYMBOL_UNSCOPABLES), &desc)?;
    Ok(())
}

pub(crate) fn species_getter(_: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(this.clone())
}

impl Interpreter {
    /// IsArray (sees through proxies).
    pub(crate) fn is_array(&mut self, value: &Value) -> JsResult<bool> {
        let Some(mut object) = self.as_object(value) else {
            return Ok(false);
        };
        loop {
            if self.realm.is_array(object) {
                return Ok(true);
            }
            let Some(Slot::Proxy(state)) = self.slot(object) else {
                return Ok(false);
            };
            let target = state.borrow().target;
            match target {
                Some(target) => object = target,
                None => return self.throw_type("Cannot perform 'IsArray' on a revoked proxy"),
            }
        }
    }

    /// ArrayCreate(length, proto), rooted until the native returns.
    pub(crate) fn array_create(
        &mut self,
        length: f64,
        proto: Option<ObjectRef>,
    ) -> JsResult<ObjectRef> {
        if length > MAX_ARRAY_LENGTH {
            return self.throw_range("Invalid array length");
        }
        let proto = proto.or(Some(self.state.intrinsics.array_prototype));
        let array = self
            .realm
            .create_array(proto, length as u32, &self.state)
            .map_err(crate::interpreter::Abrupt::Heap)?;
        self.keep(&object_value(array));
        Ok(array)
    }

    /// ArraySpeciesCreate(originalArray, length).
    pub(crate) fn array_species_create(
        &mut self,
        original: ObjectRef,
        length: f64,
    ) -> JsResult<ObjectRef> {
        if !self.is_array(&object_value(original))? {
            return self.array_create(length, None);
        }
        let mut constructor = self.get_named(original, "constructor")?;
        if let Some(c) = self.as_object(&constructor) {
            constructor = self.get_key(c, ObjectKey::Symbol(SYMBOL_SPECIES))?;
            if matches!(constructor, Value::Null) {
                constructor = Value::Undefined;
            }
        }
        if matches!(constructor, Value::Undefined) {
            return self.array_create(length, None);
        }
        if !self.is_constructor(&constructor) {
            return self.throw_type("object.constructor[Symbol.species] is not a constructor");
        }
        self.construct_object(&constructor, &[Value::Number(length)])
    }

    /// Construct(C, args) whose result must be an object; rooted until the native returns.
    pub(crate) fn construct_object(
        &mut self,
        constructor: &Value,
        args: &[Value],
    ) -> JsResult<ObjectRef> {
        let result = self.construct_value(constructor, args, constructor)?;
        self.keep(&result);
        match self.as_object(&result) {
            Some(object) => Ok(object),
            None => self.throw_type("constructor did not return an object"),
        }
    }

    /// `Set(O, "length", len, true)`.
    pub(crate) fn set_length(&mut self, object: ObjectRef, length: f64) -> JsResult<()> {
        let key = self.state.intrinsics.length_key;
        self.set_or_throw(object, key, Value::Number(length))
    }

    /// Throws the TypeError of a length that would pass 2^53 - 1.
    pub(crate) fn check_length(&mut self, length: f64) -> JsResult<()> {
        if length > MAX_SAFE_INTEGER {
            return self.throw_type("Array length exceeds the maximum safe integer");
        }
        Ok(())
    }

    /// The callback argument of an iteration method, checked callable.
    pub(crate) fn callback_arg(&mut self, args: &[Value], method: &str) -> JsResult<Value> {
        let callback = arg(args, 0);
        if !self.is_callable(&callback) {
            return self.throw_type(&format!(
                "Array.prototype.{method}: callback is not a function"
            ));
        }
        Ok(callback)
    }
}

fn array_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = match vm.new_target() {
        Value::Undefined => object_value(vm.intrinsic(Intrinsic::ArrayConstructor)),
        target => target,
    };
    let default = vm.state.intrinsics.array_prototype;
    let proto = vm.proto_from_constructor(&new_target, default)?;
    match args {
        [] => Ok(object_value(vm.array_create(0.0, Some(proto))?)),
        [Value::Number(length)] => {
            let int_length = f64::from(to_uint32(*length));
            if !same_value_zero(&Value::Number(int_length), &Value::Number(*length)) {
                return vm.throw_range("Invalid array length");
            }
            Ok(object_value(vm.array_create(int_length, Some(proto))?))
        }
        [value] => {
            let array = vm.array_create(0.0, Some(proto))?;
            vm.create_data_or_throw(array, ObjectKey::Index(0), value.clone())?;
            Ok(object_value(array))
        }
        values => {
            let array = vm.array_create(values.len() as f64, Some(proto))?;
            for (index, value) in values.iter().enumerate() {
                vm.create_data_or_throw(array, ObjectKey::Index(index as u32), value.clone())?;
            }
            Ok(object_value(array))
        }
    }
}

fn array_is_array(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(vm.is_array(&arg(args, 0))?))
}

/// `IsConstructor(C) ? Construct(C, args) : ArrayCreate(length)`.
fn construct_or_array(
    vm: &mut Interpreter,
    c: &Value,
    args: &[Value],
    length: f64,
) -> JsResult<ObjectRef> {
    if vm.is_constructor(c) {
        vm.construct_object(c, args)
    } else {
        vm.array_create(length, None)
    }
}

fn array_of(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let length = args.len() as f64;
    let array = construct_or_array(vm, this, &[Value::Number(length)], length)?;
    for (index, value) in args.iter().enumerate() {
        vm.create_data_or_throw(array, ObjectKey::Index(index as u32), value.clone())?;
    }
    vm.set_length(array, length)?;
    Ok(object_value(array))
}

fn array_from(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let items = arg(args, 0);
    let mapper = arg(args, 1);
    let this_arg = arg(args, 2);
    let mapping = !matches!(mapper, Value::Undefined);
    if mapping && !vm.is_callable(&mapper) {
        return vm.throw_type("Array.from: when provided, the second argument must be a function");
    }
    if let Some(method) = vm.get_method(&items, ObjectKey::Symbol(SYMBOL_ITERATOR))? {
        let array = construct_or_array(vm, this, &[], 0.0)?;
        let (iterator, next) = vm.open_iterator_with(&items, &method)?;
        let mut index = 0.0;
        vm.drain_iterator(&iterator, &next, &mut |vm, value| {
            vm.check_length(index + 1.0)?;
            let value = match mapping {
                true => vm.call_value(&mapper, &this_arg, &[value, Value::Number(index)])?,
                false => value,
            };
            let key = vm.index_key(index);
            vm.create_data_or_throw(array, key, value)?;
            index += 1.0;
            Ok(())
        })?;
        vm.set_length(array, index)?;
        return Ok(object_value(array));
    }
    let source = vm.coerce_object(&items)?;
    let length = vm.length_of(source)?;
    let array = construct_or_array(vm, this, &[Value::Number(length)], length)?;
    let mut index = 0.0;
    while index < length {
        let key = vm.index_key(index);
        let mut value = vm.get_key(source, key)?;
        if mapping {
            value = vm.call_value(&mapper, &this_arg, &[value, Value::Number(index)])?;
        }
        vm.create_data_or_throw(array, key, value)?;
        index += 1.0;
    }
    vm.set_length(array, length)?;
    Ok(object_value(array))
}

/// `keys`/`values`/`entries` on `Array.prototype`.
pub(crate) fn array_iterator(
    vm: &mut Interpreter,
    this: &Value,
    kind: IterationKind,
) -> JsResult<Value> {
    let object = vm.coerce_object(this)?;
    vm.create_array_iterator(&object_value(object), kind)
}
