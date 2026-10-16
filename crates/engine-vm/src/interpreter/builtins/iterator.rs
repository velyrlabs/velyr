//! %IteratorPrototype%, the built-in iterator prototypes (Array, String, Map, Set) and
//! the Rust-side iteration helper used by built-ins that consume iterables.

use std::cell::RefCell;
use std::rc::Rc;

use super::slots::{ArrayIteratorState, IterationKind, Slot, TableIteratorState};
use super::symbol::{SYMBOL_ITERATOR, SYMBOL_TO_STRING_TAG};
use super::{Interpreter, Intrinsic, JsResult, object_value};
use crate::interpreter::Abrupt;
use crate::{JsStr, ObjectKey, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let iterator_proto = vm.intrinsic(Intrinsic::IteratorPrototype);
    vm.symbol_method(iterator_proto, SYMBOL_ITERATOR, 0, iterator_self)?;
    let tag = ObjectKey::Symbol(SYMBOL_TO_STRING_TAG);
    vm.accessor(
        iterator_proto,
        tag,
        "[Symbol.toStringTag]",
        iterator_tag,
        Some(set_iterator_tag),
    )?;
    let protos: [(Intrinsic, &str, super::Builtin); 3] = [
        (
            Intrinsic::StringIteratorPrototype,
            "String Iterator",
            string_iterator_next,
        ),
        (
            Intrinsic::MapIteratorPrototype,
            "Map Iterator",
            map_iterator_next,
        ),
        (
            Intrinsic::SetIteratorPrototype,
            "Set Iterator",
            set_iterator_next,
        ),
    ];
    let array_iterator = vm.state.intrinsics.array_iterator_prototype;
    vm.method(array_iterator, "next", 0, array_iterator_next)?;
    vm.define_to_string_tag(array_iterator, "Array Iterator")?;
    for (which, tag, next) in protos {
        let proto = vm.intrinsic(which);
        vm.method(proto, "next", 0, next)?;
        vm.define_to_string_tag(proto, tag)?;
    }
    Ok(())
}

fn iterator_tag(_: &mut Interpreter, _: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::str("Iterator"))
}

/// SetterThatIgnoresPrototypeProperties for `%IteratorPrototype%[@@toStringTag]`.
fn set_iterator_tag(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let Some(object) = vm.as_object(this) else {
        return vm.throw_type("Iterator.prototype[Symbol.toStringTag] setter called on non-object");
    };
    if object == vm.intrinsic(Intrinsic::IteratorPrototype) {
        return vm.throw_type("Cannot assign to read only property 'Symbol(Symbol.toStringTag)'");
    }
    let key = ObjectKey::Symbol(SYMBOL_TO_STRING_TAG);
    let value = super::arg(args, 0);
    match vm.own_property(object, key)? {
        None => vm.create_data_or_throw(object, key, value)?,
        Some(_) => vm.set_or_throw(object, key, value)?,
    }
    Ok(Value::Undefined)
}

fn iterator_self(_: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    Ok(this.clone())
}

impl Interpreter {
    /// Runs `f` on every value of `iterable`; an abrupt `f` closes the iterator.
    pub(crate) fn iterate(
        &mut self,
        iterable: &Value,
        mut f: impl FnMut(&mut Self, Value) -> JsResult<()>,
    ) -> JsResult<()> {
        let (iterator, next) = self.open_iterator(iterable)?;
        self.drain_iterator(&iterator, &next, &mut f)
    }

    pub(crate) fn drain_iterator(
        &mut self,
        iterator: &Value,
        next: &Value,
        f: &mut dyn FnMut(&mut Self, Value) -> JsResult<()>,
    ) -> JsResult<()> {
        self.keep(iterator);
        self.keep(next);
        while let Some(value) = self.next_value(iterator, next)? {
            self.keep(&value);
            if let Err(abrupt) = f(self, value) {
                return Err(self.close_after_abrupt(iterator, abrupt));
            }
        }
        Ok(())
    }

    /// IteratorClose with a throw completion: the original completion wins.
    pub(crate) fn close_after_abrupt(&mut self, iterator: &Value, abrupt: Abrupt) -> Abrupt {
        if !matches!(abrupt, Abrupt::Throw(_)) {
            return abrupt;
        }
        let key = self.realm.key_str("return");
        if let Ok(Some(method)) = self.get_method(iterator, key) {
            let _ = self.call_value(&method, iterator, &[]);
        }
        abrupt
    }

    /// CreateIterResultObject.
    pub(crate) fn iter_result(&mut self, value: Value, done: bool) -> JsResult<Value> {
        self.keep(&value);
        let result = self.new_object()?;
        let key = self.realm.key_str("value");
        self.create_data_property(result, key, value)?;
        let key = self.realm.key_str("done");
        self.create_data_property(result, key, Value::Bool(done))?;
        Ok(object_value(result))
    }

    /// CreateArrayIterator.
    pub(crate) fn create_array_iterator(
        &mut self,
        target: &Value,
        kind: IterationKind,
    ) -> JsResult<Value> {
        let state = ArrayIteratorState {
            target: target.clone(),
            index: 0,
            kind,
        };
        let proto = self.state.intrinsics.array_iterator_prototype;
        let slot = Slot::ArrayIterator(Rc::new(RefCell::new(state)));
        Ok(object_value(self.new_slotted(proto, slot)?))
    }

    pub(crate) fn create_string_iterator(&mut self, text: JsStr) -> JsResult<Value> {
        let proto = self.intrinsic(Intrinsic::StringIteratorPrototype);
        let slot = Slot::StringIterator(Rc::new(RefCell::new(Some((text, 0)))));
        Ok(object_value(self.new_slotted(proto, slot)?))
    }

    pub(crate) fn create_table_iterator(
        &mut self,
        slot: &Slot,
        kind: IterationKind,
    ) -> JsResult<Value> {
        let (table, proto, is_map) = match slot {
            Slot::Map(table) => (table.clone(), Intrinsic::MapIteratorPrototype, true),
            Slot::Set(table) => (table.clone(), Intrinsic::SetIteratorPrototype, false),
            _ => return Err(self.internal("not a keyed collection")),
        };
        let state = Rc::new(RefCell::new(TableIteratorState {
            table: Some(table),
            position: 0,
            kind,
        }));
        let slot = if is_map {
            Slot::MapIterator(state)
        } else {
            Slot::SetIterator(state)
        };
        let proto = self.intrinsic(proto);
        Ok(object_value(self.new_slotted(proto, slot)?))
    }
}

fn array_iterator_next(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let Some(Slot::ArrayIterator(state)) = vm.value_slot(this) else {
        return vm.throw_type("next method called on incompatible receiver");
    };
    let (target, index, kind) = {
        let state = state.borrow();
        (state.target.clone(), state.index, state.kind)
    };
    let Some(array) = vm.as_object(&target) else {
        return vm.iter_result(Value::Undefined, true);
    };
    let length = vm.length_of(array)?;
    if index as f64 >= length {
        state.borrow_mut().target = Value::Undefined;
        return vm.iter_result(Value::Undefined, true);
    }
    state.borrow_mut().index = index + 1;
    let index_value = Value::Number(index as f64);
    let value = match kind {
        IterationKind::Keys => index_value,
        IterationKind::Values => {
            let key = vm.index_key(index as f64);
            vm.get_key(array, key)?
        }
        IterationKind::Entries => {
            let key = vm.index_key(index as f64);
            let element = vm.get_key(array, key)?;
            vm.keep(&element);
            vm.array_from_list(&[index_value, element])?
        }
    };
    vm.iter_result(value, false)
}

fn string_iterator_next(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let Some(Slot::StringIterator(state)) = vm.value_slot(this) else {
        return vm.throw_type("next method called on incompatible receiver");
    };
    let Some((text, position)) = state.borrow().clone() else {
        return vm.iter_result(Value::Undefined, true);
    };
    let units = text.to_utf16();
    if position >= units.len() {
        *state.borrow_mut() = None;
        return vm.iter_result(Value::Undefined, true);
    }
    let size = super::string::code_point_size(&units, position);
    let piece = JsStr::from_utf16(units[position..position + size].to_vec());
    *state.borrow_mut() = Some((text, position + size));
    vm.iter_result(Value::String(piece), false)
}

fn table_iterator_next(
    vm: &mut Interpreter,
    state: Rc<RefCell<TableIteratorState>>,
) -> JsResult<Value> {
    let (table, position, kind) = {
        let state = state.borrow();
        (state.table.clone(), state.position, state.kind)
    };
    let Some(table) = table else {
        return vm.iter_result(Value::Undefined, true);
    };
    let entry = table.borrow().entry_from(position);
    let Some((next, key, value)) = entry else {
        state.borrow_mut().table = None;
        return vm.iter_result(Value::Undefined, true);
    };
    state.borrow_mut().position = next;
    let result = match kind {
        IterationKind::Keys => key,
        IterationKind::Values => value,
        IterationKind::Entries => vm.array_from_list(&[key, value])?,
    };
    vm.iter_result(result, false)
}

fn map_iterator_next(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    match vm.value_slot(this) {
        Some(Slot::MapIterator(state)) => table_iterator_next(vm, state),
        _ => vm.throw_type("Map Iterator next called on incompatible receiver"),
    }
}

fn set_iterator_next(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    match vm.value_slot(this) {
        Some(Slot::SetIterator(state)) => table_iterator_next(vm, state),
        _ => vm.throw_type("Set Iterator next called on incompatible receiver"),
    }
}
