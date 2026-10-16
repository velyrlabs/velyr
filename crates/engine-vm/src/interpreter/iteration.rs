//! Iterator protocol (for-of, spread, destructuring) and for-in key enumeration.

use std::collections::HashSet;

use velyr_engine_gc::{Gc, Trace, Tracer};

use super::builtins::symbol::SYMBOL_ITERATOR;
use super::state::CodeInfo;
use super::{Abrupt, ErrorKind, Interpreter, JsResult};
use crate::bytecode::{Instruction as I, Reg};
use crate::{ObjectKey, Value, to_boolean};

/// Heap cell behind a for-in enumerator register.
struct ForInState {
    object: Value,
    keys: Vec<ObjectKey>,
    next: usize,
}

impl Trace for ForInState {
    fn trace(&self, tracer: &mut Tracer) {
        self.object.trace(tracer);
    }
}

fn offset(record: Reg, by: u32) -> Reg {
    Reg(record.0 + by)
}

impl Interpreter {
    pub(crate) fn step_iteration(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<()> {
        match instruction {
            I::GetIterator { record } => {
                let iterable = self.state.acc.clone();
                let (iterator, next) = self.open_iterator(&iterable)?;
                self.set_reg(*record, iterator);
                self.set_reg(offset(*record, 1), next);
                self.set_reg(offset(*record, 2), Value::Bool(false));
            }
            I::IteratorClose { record } => self.close_iterator(*record)?,
            I::IteratorCloseAfterThrow { record } => match self.close_iterator(*record) {
                Err(Abrupt::Throw(_)) | Ok(()) => {}
                Err(abrupt) => return Err(abrupt),
            },
            I::ForInPrepare { enumerator } => {
                let object = self.state.acc.clone();
                let keys = self.for_in_keys(&object)?;
                let state = ForInState {
                    object,
                    keys,
                    next: 0,
                };
                let cell = self.alloc_cell(state)?;
                self.set_reg(*enumerator, Value::Object(cell));
            }
            I::SpreadIntoArray { array } => self.spread_into_array(*array)?,
            other => return self.step_class(info, other),
        }
        Ok(())
    }

    /// GetIterator(value, sync): `(iterator, next method)`.
    pub(crate) fn open_iterator(&mut self, iterable: &Value) -> JsResult<(Value, Value)> {
        let method = self.get_value(iterable, ObjectKey::Symbol(SYMBOL_ITERATOR), None)?;
        if self.callable(&method).is_none() {
            return Err(self.error(ErrorKind::Type, "object is not iterable"));
        }
        self.open_iterator_with(iterable, &method)
    }

    /// GetIteratorFromMethod.
    pub(crate) fn open_iterator_with(
        &mut self,
        iterable: &Value,
        method: &Value,
    ) -> JsResult<(Value, Value)> {
        let iterator = self.call_value(method, iterable, &[])?;
        if !matches!(iterator, Value::Object(_)) {
            let message = "Result of the Symbol.iterator method is not an object";
            return Err(self.error(ErrorKind::Type, message));
        }
        self.state.scratch.push(iterator.clone());
        let key = self.realm.key_str("next");
        let next = self.get_value(&iterator, key, None);
        self.state.scratch.pop();
        Ok((iterator, next?))
    }

    /// IteratorStep: `Some(value)` or `None` when done.
    pub(crate) fn next_value(&mut self, iterator: &Value, next: &Value) -> JsResult<Option<Value>> {
        let result = self.call_value(next, iterator, &[])?;
        let Some(result_object) = self.as_object(&result) else {
            let message = "Iterator result is not an object";
            return Err(self.error(ErrorKind::Type, message));
        };
        let key = self.realm.key_str("done");
        if to_boolean(&self.get(result_object, key, &result)?) {
            return Ok(None);
        }
        let key = self.realm.key_str("value");
        self.get(result_object, key, &result).map(Some)
    }

    /// `IteratorStep` instruction: `true` with the value in `acc`; any abrupt
    /// completion or exhaustion sets the record's done flag.
    pub(crate) fn iterator_step(&mut self, record: Reg) -> JsResult<bool> {
        let iterator = self.reg(record).clone();
        let next = self.reg(offset(record, 1)).clone();
        let step = self.next_value(&iterator, &next);
        if !matches!(step, Ok(Some(_))) {
            self.set_reg(offset(record, 2), Value::Bool(true));
        }
        match step? {
            Some(value) => {
                self.state.acc = value;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// IteratorClose with a normal completion; no-op once the iterator is done.
    fn close_iterator(&mut self, record: Reg) -> JsResult<()> {
        if to_boolean(self.reg(offset(record, 2))) {
            return Ok(());
        }
        self.set_reg(offset(record, 2), Value::Bool(true));
        let iterator = self.reg(record).clone();
        let key = self.realm.key_str("return");
        let method = self.get_value(&iterator, key, None)?;
        if matches!(method, Value::Undefined | Value::Null) {
            return Ok(());
        }
        let result = self.call_value(&method, &iterator, &[])?;
        if !matches!(result, Value::Object(_)) {
            let message = "Iterator result is not an object";
            return Err(self.error(ErrorKind::Type, message));
        }
        Ok(())
    }

    fn spread_into_array(&mut self, array: Reg) -> JsResult<()> {
        let iterable = self.state.acc.clone();
        let (iterator, next) = self.open_iterator(&iterable)?;
        let base = self.state.scratch.len();
        self.state.scratch.extend([iterator.clone(), next.clone()]);
        let result = self.drain_into(array, &iterator, &next);
        self.state.scratch.truncate(base);
        result
    }

    fn drain_into(&mut self, array: Reg, iterator: &Value, next: &Value) -> JsResult<()> {
        let array = self
            .as_object(self.reg(array))
            .expect("spread target is an array");
        while let Some(value) = self.next_value(iterator, next)? {
            let length = self.array_length(array)?;
            self.create_data_property(array, ObjectKey::Index(length), value)?;
        }
        Ok(())
    }

    /// EnumerateObjectProperties: enumerable string keys along the prototype chain,
    /// shadowed names visited once.
    fn for_in_keys(&mut self, value: &Value) -> JsResult<Vec<ObjectKey>> {
        if let Value::String(s) = value {
            return Ok((0..s.len() as u32).map(ObjectKey::Index).collect());
        }
        let mut cursor = self.as_object(value);
        let mut seen = HashSet::new();
        let mut keys = Vec::new();
        while let Some(object) = cursor {
            let own = self.own_keys(object)?;
            for key in own
                .into_iter()
                .filter(|k| !matches!(k, ObjectKey::Symbol(_)))
            {
                if !seen.insert(key) {
                    continue;
                }
                if self
                    .own_property(object, key)?
                    .is_some_and(|p| p.enumerable)
                {
                    keys.push(key);
                }
            }
            cursor = self.get_prototype(object)?;
        }
        Ok(keys)
    }

    /// `ForInNext`: `true` with the next key (still present) in `acc`.
    pub(crate) fn for_in_next(&mut self, enumerator: Reg) -> JsResult<bool> {
        let Value::Object(cell) = self.reg(enumerator).clone() else {
            return Err(self.internal("ForInNext without an enumerator"));
        };
        let cell: Gc<ForInState> = Gc::from_raw(cell);
        loop {
            let state = self.realm.heap.get_mut(cell).expect("enumerator is live");
            let Some(&key) = state.keys.get(state.next) else {
                return Ok(false);
            };
            state.next += 1;
            let object = state.object.clone();
            let is_present = match self.as_object(&object) {
                Some(o) => self.has_property(o, key)?,
                None => true,
            };
            if is_present {
                self.state.acc = self.key_to_value(key);
                return Ok(true);
            }
        }
    }
}
