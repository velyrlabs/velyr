//! `Promise.all`, `allSettled`, `any` and `race` (ECMA-262 §27.2.4).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use velyr_engine_gc::{Trace, Tracer};

use super::promise_ops::{Capability, Captures};
use super::{Interpreter, JsResult, arg, object_value};
use crate::{ObjectRef, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Combinator {
    All,
    AllSettled,
    Any,
    Race,
}

/// The captures of a Promise.all Resolve Element Function and its siblings.
pub(crate) struct ElementState {
    combinator: Combinator,
    /// allSettled's onRejected, which records `{ status: "rejected", reason }`.
    rejected: bool,
    index: usize,
    /// `values` (or `errors` for `any`), shared by every element function of one call.
    list: Rc<RefCell<Vec<Value>>>,
    remaining: Rc<Cell<usize>>,
    capability: Capability,
    already: Rc<Cell<bool>>,
}

impl Trace for ElementState {
    fn trace(&self, tracer: &mut Tracer) {
        self.list.borrow().as_slice().trace(tracer);
        self.capability.trace(tracer);
    }
}

pub(super) fn install(vm: &mut Interpreter, constructor: ObjectRef) -> JsResult<()> {
    vm.method(constructor, "all", 1, promise_all)?;
    vm.method(constructor, "allSettled", 1, promise_all_settled)?;
    vm.method(constructor, "any", 1, promise_any)?;
    vm.method(constructor, "race", 1, promise_race)
}

fn promise_all(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    combine(vm, this, args, Combinator::All)
}

fn promise_all_settled(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    combine(vm, this, args, Combinator::AllSettled)
}

fn promise_any(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    combine(vm, this, args, Combinator::Any)
}

fn promise_race(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    combine(vm, this, args, Combinator::Race)
}

fn combine(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    combinator: Combinator,
) -> JsResult<Value> {
    let capability = vm.new_capability(this)?;
    let result = vm.perform_combinator(this, &capability, &arg(args, 0), combinator);
    vm.reject_if_abrupt(result, &capability)
}

/// The shared state of one combinator call.
struct Run {
    combinator: Combinator,
    constructor: Value,
    resolve: Value,
    capability: Capability,
    list: Rc<RefCell<Vec<Value>>>,
    remaining: Rc<Cell<usize>>,
}

impl Interpreter {
    /// GetPromiseResolve, GetIterator and PerformPromiseAll/AllSettled/Any/Race.
    fn perform_combinator(
        &mut self,
        constructor: &Value,
        capability: &Capability,
        iterable: &Value,
        combinator: Combinator,
    ) -> JsResult<Value> {
        let object = self
            .as_object(constructor)
            .expect("constructors are objects");
        let resolve = self.get_named(object, "resolve")?;
        if !self.is_callable(&resolve) {
            return self.throw_type("Promise resolve is not a function");
        }
        self.keep(&resolve);
        let (iterator, next) = self.open_iterator(iterable)?;
        self.keep(&iterator);
        self.keep(&next);
        let run = Run {
            combinator,
            constructor: constructor.clone(),
            resolve,
            capability: capability.clone(),
            list: Rc::new(RefCell::new(Vec::new())),
            remaining: Rc::new(Cell::new(1)),
        };
        let mut index = 0;
        // IteratorStepValue errors mark the iterator done, so they skip IteratorClose.
        while let Some(value) = self.next_value(&iterator, &next)? {
            self.keep(&value);
            if let Err(abrupt) = self.combine_element(&run, index, value) {
                return Err(self.close_after_abrupt(&iterator, abrupt));
            }
            index += 1;
        }
        if combinator != Combinator::Race {
            run.remaining.set(run.remaining.get() - 1);
            if run.remaining.get() == 0 {
                self.finish_combinator(combinator, &run.list, capability)?;
            }
        }
        Ok(capability.promise.clone())
    }

    fn combine_element(&mut self, run: &Run, index: usize, value: Value) -> JsResult<()> {
        if run.combinator != Combinator::Race {
            run.list.borrow_mut().push(Value::Undefined);
        }
        let next_promise = self.call_value(&run.resolve, &run.constructor, &[value])?;
        self.keep(&next_promise);
        let already = Rc::new(Cell::new(false));
        let element = |vm: &mut Self, rejected| {
            let state = ElementState {
                combinator: run.combinator,
                rejected,
                index,
                list: run.list.clone(),
                remaining: run.remaining.clone(),
                capability: run.capability.clone(),
                already: already.clone(),
            };
            vm.promise_function(1, Rc::new(Captures::Element(state)))
        };
        let capability = &run.capability;
        let (on_fulfilled, on_rejected) = match run.combinator {
            Combinator::All => (element(self, false)?, capability.reject.clone()),
            Combinator::AllSettled => (element(self, false)?, element(self, true)?),
            Combinator::Any => (capability.resolve.clone(), element(self, true)?),
            Combinator::Race => (capability.resolve.clone(), capability.reject.clone()),
        };
        if run.combinator != Combinator::Race {
            run.remaining.set(run.remaining.get() + 1);
        }
        self.invoke(&next_promise, "then", &[on_fulfilled, on_rejected])?;
        Ok(())
    }

    /// The body of an element function.
    pub(crate) fn run_element(&mut self, element: &ElementState, x: Value) -> JsResult<Value> {
        if element.already.replace(true) {
            return Ok(Value::Undefined);
        }
        let entry = if element.combinator == Combinator::AllSettled {
            let (status, key) = match element.rejected {
                false => ("fulfilled", "value"),
                true => ("rejected", "reason"),
            };
            let object = self.new_object()?;
            let status_key = self.realm.key_str("status");
            self.create_data_property(object, status_key, Value::str(status))?;
            let key = self.realm.key_str(key);
            self.create_data_property(object, key, x)?;
            object_value(object)
        } else {
            x
        };
        element.list.borrow_mut()[element.index] = entry;
        element.remaining.set(element.remaining.get() - 1);
        if element.remaining.get() == 0 {
            self.finish_combinator(element.combinator, &element.list, &element.capability)?;
        }
        Ok(Value::Undefined)
    }

    /// Resolves with the values array, or (for `any`) rejects with an `AggregateError`.
    fn finish_combinator(
        &mut self,
        combinator: Combinator,
        list: &Rc<RefCell<Vec<Value>>>,
        capability: &Capability,
    ) -> JsResult<()> {
        let values = list.borrow().clone();
        for value in &values {
            self.keep(value);
        }
        let array = self.array_from_list(&values)?;
        // The spec throws at the end of PerformPromiseAny and IfAbruptRejectPromise calls
        // reject; calling reject directly is the same observable step.
        let (settle, value) = match combinator {
            Combinator::Any => {
                let error = self.aggregate_error(array, "All promises were rejected")?;
                (&capability.reject, error)
            }
            _ => (&capability.resolve, array),
        };
        self.call_value(settle, &Value::Undefined, &[value])?;
        Ok(())
    }
}
