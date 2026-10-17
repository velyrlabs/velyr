//! Explicit Resource Management (ES2026): the dispose stack behind `using` and
//! `await using`. The stack is a plain JS array held in a register, so the GC traces it;
//! it stores flat `[value, method, wraps_sync]` triples, where `wraps_sync` marks a
//! `@@dispose` fallback of `await using` (GetDisposeMethod's closure: the result is dropped
//! and a throw becomes a rejected promise).

use super::builtins::Intrinsic;
use super::builtins::suppressed_error::make_suppressed;
use super::builtins::symbol::{SYMBOL_ASYNC_DISPOSE, SYMBOL_DISPOSE};
use super::function::CallOutcome;
use super::ops::is_hole;
use super::{Abrupt, Interpreter, JsResult, object_value};
use crate::bytecode::{Instruction as I, Reg};
use crate::{ObjectKey, ObjectRef, Value};

/// Values per resource record on the stack.
const RECORD_LEN: u32 = 3;

/// One popped resource: the value, its dispose method and the `wraps_sync` flag.
type Resource = (Value, Value, bool);

impl Interpreter {
    /// `CreateDisposeStack`, `AddDisposable` and `DisposeRecordError`.
    pub(super) fn step_dispose(&mut self, instruction: &I) -> JsResult<()> {
        match instruction {
            I::CreateDisposeStack(stack) => {
                let array = self.create_array_from(&[])?;
                self.set_reg(*stack, object_value(array));
            }
            I::AddDisposable { stack, is_async } => {
                let (stack, value) = (self.reg(*stack).clone(), self.state.acc.clone());
                self.add_disposable(&stack, value, *is_async)?;
            }
            I::DisposeRecordError { error } => {
                let (thrown, pending) = (self.state.acc.clone(), self.reg(*error).clone());
                let error_value = self.record_dispose_error(thrown, pending)?;
                self.set_reg(*error, error_value);
            }
            _ => return Err(self.internal("step_dispose got a non-dispose instruction")),
        }
        Ok(())
    }

    /// `DisposeNext`: `None` when the stack is empty, else the outcome of the dispose call.
    pub(super) fn dispose_next(&mut self, stack: Reg) -> JsResult<Option<CallOutcome>> {
        let stack = self.reg(stack).clone();
        let Some((value, method, wraps_sync)) = self.pop_disposable(&stack)? else {
            return Ok(None);
        };
        if matches!(method, Value::Undefined) {
            self.state.acc = Value::Undefined;
            return Ok(Some(CallOutcome::Done));
        }
        if wraps_sync {
            self.state.acc = self.call_sync_dispose(&method, &value)?;
            return Ok(Some(CallOutcome::Done));
        }
        self.call_from_loop(&method, &value, &[]).map(Some)
    }

    /// AddDisposableResource: nullish values are skipped under `using` and kept with an
    /// `undefined` method under `await using`; other non-objects are a TypeError.
    pub(super) fn add_disposable(
        &mut self,
        stack: &Value,
        value: Value,
        is_async: bool,
    ) -> JsResult<()> {
        let (method, wraps_sync) = match value {
            Value::Undefined | Value::Null if !is_async => return Ok(()),
            Value::Undefined | Value::Null => (Value::Undefined, false),
            _ if self.as_object(&value).is_none() => {
                return self.throw_type("Disposable resource must be an object");
            }
            _ => self.dispose_method(&value, is_async)?,
        };
        let stack = self.stack_object(stack)?;
        let length = self.array_length(stack)?;
        let record = [value, method, Value::Bool(wraps_sync)];
        for (index, item) in (length..).zip(record) {
            self.create_data_property(stack, ObjectKey::Index(index), item)?;
        }
        Ok(())
    }

    /// GetDisposeMethod: `(method, wraps_sync)`; a TypeError when there is none.
    fn dispose_method(&mut self, value: &Value, is_async: bool) -> JsResult<(Value, bool)> {
        if is_async
            && let Some(method) = self.get_method(value, ObjectKey::Symbol(SYMBOL_ASYNC_DISPOSE))?
        {
            return Ok((method, false));
        }
        match self.get_method(value, ObjectKey::Symbol(SYMBOL_DISPOSE))? {
            Some(method) => Ok((method, is_async)),
            None => self.throw_type("Disposable resource has no dispose method"),
        }
    }

    /// Removes and returns the last resource, or `None` when the stack is empty.
    pub(super) fn pop_disposable(&mut self, stack: &Value) -> JsResult<Option<Resource>> {
        let stack = self.stack_object(stack)?;
        let length = self.array_length(stack)?;
        let Some(first) = length.checked_sub(RECORD_LEN) else {
            return Ok(None);
        };
        let value = self.get_key(stack, ObjectKey::Index(first))?;
        let method = self.get_key(stack, ObjectKey::Index(first + 1))?;
        let wraps_sync = self.get_key(stack, ObjectKey::Index(first + 2))?;
        self.set_length(stack, f64::from(first))?;
        Ok(Some((
            value,
            method,
            matches!(wraps_sync, Value::Bool(true)),
        )))
    }

    /// DisposeResources step 3.e.iii: the first error is kept; a later one wraps it.
    pub(super) fn record_dispose_error(
        &mut self,
        thrown: Value,
        pending: Value,
    ) -> JsResult<Value> {
        if is_hole(&pending) {
            return Ok(thrown);
        }
        make_suppressed(self, thrown, pending)
    }

    /// The GetDisposeMethod closure around a `@@dispose` fallback: `undefined`, or a promise
    /// rejected with what the method threw.
    fn call_sync_dispose(&mut self, method: &Value, value: &Value) -> JsResult<Value> {
        let reason = match self.call(method, value, &[]) {
            Ok(_) => return Ok(Value::Undefined),
            Err(Abrupt::Throw(reason)) => reason,
            Err(abrupt) => return Err(abrupt),
        };
        let proto = self.intrinsic(Intrinsic::PromisePrototype);
        let (promise, record) = self.new_promise(proto)?;
        self.reject_promise(&record, reason);
        Ok(promise)
    }

    fn stack_object(&mut self, stack: &Value) -> JsResult<ObjectRef> {
        match self.as_object(stack) {
            Some(object) => Ok(object),
            None => Err(self.internal("dispose stack register does not hold an array")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::interpreter::ops::HOLE;

    /// An object whose `@@dispose` logs `id` and throws `id` when `throws`.
    fn resource(vm: &mut Interpreter, log: &Rc<RefCell<Vec<u32>>>, id: u32, throws: bool) -> Value {
        let log = log.clone();
        let dispose = vm
            .create_native_function(
                "dispose",
                0,
                Rc::new(move |_, _, _| {
                    log.borrow_mut().push(id);
                    match throws {
                        true => Err(Abrupt::Throw(Value::Number(f64::from(id)))),
                        false => Ok(Value::Undefined),
                    }
                }),
                false,
            )
            .unwrap();
        let object = vm.new_object().unwrap();
        let key = ObjectKey::Symbol(SYMBOL_DISPOSE);
        assert!(
            vm.create_data_property(object, key, object_value(dispose))
                .unwrap()
        );
        object_value(object)
    }

    fn new_stack(vm: &mut Interpreter) -> Value {
        object_value(vm.create_array_from(&[]).unwrap())
    }

    /// Drains the stack like the compiled `finally` of a `using` block.
    fn drain(vm: &mut Interpreter, stack: &Value, mut error: Value) -> Value {
        while let Some((value, method, _)) = vm.pop_disposable(stack).unwrap() {
            if let Err(Abrupt::Throw(thrown)) = vm.call(&method, &value, &[]) {
                error = vm.record_dispose_error(thrown, error).unwrap();
            }
        }
        error
    }

    fn property(vm: &mut Interpreter, target: &Value, name: &str) -> Value {
        let object = vm.as_object(target).expect("object");
        let key = vm.realm.key_str(name);
        vm.get_key(object, key).unwrap()
    }

    #[test]
    fn disposes_in_reverse_order() {
        let mut vm = Interpreter::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let stack = new_stack(&mut vm);
        for (id, throws) in [(1, false), (2, true)] {
            let value = resource(&mut vm, &log, id, throws);
            vm.add_disposable(&stack, value, false).unwrap();
        }
        let error = drain(&mut vm, &stack, HOLE);
        assert_eq!(*log.borrow(), [2, 1]);
        assert!(matches!(error, Value::Number(n) if n == 2.0));
        assert!(vm.pop_disposable(&stack).unwrap().is_none());
    }

    #[test]
    fn dispose_error_suppresses_the_pending_one() {
        let mut vm = Interpreter::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let stack = new_stack(&mut vm);
        let value = resource(&mut vm, &log, 7, true);
        vm.add_disposable(&stack, value, false).unwrap();
        let error = drain(&mut vm, &stack, Value::str("body"));
        let proto = vm.intrinsic(Intrinsic::SuppressedErrorPrototype);
        let object = vm.as_object(&error).expect("SuppressedError");
        assert_eq!(vm.get_prototype(object).unwrap(), Some(proto));
        assert!(matches!(property(&mut vm, &error, "error"), Value::Number(n) if n == 7.0));
        let suppressed = property(&mut vm, &error, "suppressed");
        assert!(vm.to_js_string(&suppressed).unwrap() == crate::JsStr::from_str("body"));
    }

    #[test]
    fn non_object_resource_is_a_type_error() {
        let mut vm = Interpreter::new();
        let stack = new_stack(&mut vm);
        let result = vm.add_disposable(&stack, Value::Number(1.0), false);
        let Err(Abrupt::Throw(error)) = result else {
            panic!("expected a TypeError");
        };
        assert!(vm.exception_message(&error).starts_with("TypeError"));
    }

    #[test]
    fn nullish_resources_only_count_under_await_using() {
        let mut vm = Interpreter::new();
        let stack = new_stack(&mut vm);
        vm.add_disposable(&stack, Value::Null, false).unwrap();
        assert!(vm.pop_disposable(&stack).unwrap().is_none());
        vm.add_disposable(&stack, Value::Undefined, true).unwrap();
        let popped = vm.pop_disposable(&stack).unwrap();
        assert!(matches!(
            popped,
            Some((Value::Undefined, Value::Undefined, false))
        ));
    }

    #[test]
    fn await_using_falls_back_to_sync_dispose() {
        let mut vm = Interpreter::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let stack = new_stack(&mut vm);
        let value = resource(&mut vm, &log, 3, false);
        vm.add_disposable(&stack, value, true).unwrap();
        let popped = vm.pop_disposable(&stack).unwrap();
        assert!(matches!(popped, Some((_, _, true))));
    }

    #[test]
    fn object_without_dispose_method_is_a_type_error() {
        let mut vm = Interpreter::new();
        let stack = new_stack(&mut vm);
        let object = object_value(vm.new_object().unwrap());
        assert!(vm.add_disposable(&stack, object, true).is_err());
    }

    /// `using a = A, b = B` drained by the instructions, the throwing dispose caught by a
    /// handler that records it, the recorded error as the completion.
    fn drain_script() -> std::rc::Rc<crate::bytecode::FunctionCode> {
        use crate::bytecode::{
            ConstIndex, Constant, FunctionCode, FunctionKind, HandlerEntry, Target,
        };
        use velyr_engine_gc::external::{Charge, Kind};
        let (stack, error, done) = (Reg(0), Reg(1), Target(11));
        let instructions = vec![
            I::CreateDisposeStack(stack),
            I::LdaTheHole,
            I::Star(error),
            I::LdaGlobal(ConstIndex(0)),
            I::AddDisposable {
                stack,
                is_async: false,
            },
            I::LdaGlobal(ConstIndex(1)),
            I::AddDisposable {
                stack,
                is_async: false,
            },
            I::DisposeNext { stack, done },
            I::Jump(Target(7)),
            I::DisposeRecordError { error },
            I::Jump(Target(7)),
            I::Ldar(error),
            I::Return,
        ];
        let handler = HandlerEntry {
            start: Target(7),
            end: Target(8),
            target: Target(9),
            context: None,
        };
        Rc::new(FunctionCode {
            name: crate::JsStr::from_str(""),
            kind: FunctionKind::Script,
            is_strict: false,
            parameter_count: 0,
            length: 0,
            register_count: 2,
            instructions,
            constants: vec![
                Constant::String(crate::JsStr::from_str("a")),
                Constant::String(crate::JsStr::from_str("b")),
            ],
            handlers: vec![handler],
            positions: Vec::new(),
            source_start: 0,
            source_end: 0,
            script: None,
            charge: Charge::new(Kind::Code, 0),
            lazy: None,
        })
    }

    #[test]
    fn instructions_drain_the_stack_through_a_handler() {
        let mut vm = Interpreter::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let global = vm.state.intrinsics.global;
        for (name, id, throws) in [("a", 1, false), ("b", 2, true)] {
            let value = resource(&mut vm, &log, id, throws);
            vm.define_method(global, name, value).unwrap();
        }
        let completion = vm.run_script(&drain_script()).unwrap();
        assert_eq!(*log.borrow(), [2, 1]);
        assert!(matches!(completion, Value::Number(n) if n == 2.0));
    }

    #[test]
    fn sync_fallback_turns_a_throw_into_a_rejected_promise() {
        let mut vm = Interpreter::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        for (throws, rejects) in [(false, false), (true, true)] {
            let value = resource(&mut vm, &log, 4, throws);
            let method = property_symbol(&mut vm, &value);
            let result = vm.call_sync_dispose(&method, &value).unwrap();
            assert_eq!(vm.promise_record(&result).is_some(), rejects);
        }
    }

    fn property_symbol(vm: &mut Interpreter, target: &Value) -> Value {
        let object = vm.as_object(target).expect("object");
        vm.get_key(object, ObjectKey::Symbol(SYMBOL_DISPOSE))
            .unwrap()
    }
}
