//! Iterator protocol steps of coroutines: `yield*` delegation, `for await` and
//! %AsyncFromSyncIteratorPrototype%.

use std::rc::Rc;

use super::{Continuation, SYMBOL_ASYNC_ITERATOR};
use crate::bytecode::{Instruction as I, Reg, ResumeMode};
use crate::interpreter::builtins::promise_ops::Capability;
use crate::interpreter::builtins::slots::Slot;
use crate::interpreter::builtins::{Intrinsic, arg};
use crate::interpreter::dispatch::Flow;
use crate::interpreter::function::NativeFn;
use crate::interpreter::state::CodeInfo;
use crate::interpreter::{Abrupt, Interpreter, JsResult, object_value};
use crate::{ObjectKey, Value, to_boolean};

pub(super) const MISSING_THROW: &str = "The iterator does not provide a 'throw' method";

/// Outcome of `YieldStarCall`.
pub(super) enum YieldStarStep {
    /// `acc` = the delegate's result.
    Called,
    /// A `return` resumption and the delegate has no `return`: the generator returns.
    MissingReturn,
    /// Async generators: the delegate lacks `throw`; await this `return()` result, then throw.
    AwaitClose(Value),
}

impl Interpreter {
    /// `(done, value)` of an iterator result object.
    pub(super) fn unpack_iter_result(&mut self, result: &Value) -> JsResult<(bool, Value)> {
        let Some(object) = self.as_object(result) else {
            return self.throw_type("Iterator result is not an object");
        };
        let done = to_boolean(&self.get_named(object, "done")?);
        Ok((done, self.get_named(object, "value")?))
    }

    /// One `yield*` delegation call (`acc` = its unchecked result).
    pub(super) fn yield_star_call(
        &mut self,
        record: Reg,
        mode: ResumeMode,
        is_async: bool,
    ) -> JsResult<YieldStarStep> {
        let iterator = self.reg(record).clone();
        let received = self.state.acc.clone();
        let (throw_key, return_key) = (self.realm.key_str("throw"), self.realm.key_str("return"));
        let method = match mode {
            ResumeMode::Next => Some(self.reg(Reg(record.0 + 1)).clone()),
            ResumeMode::Throw => self.get_method(&iterator, throw_key)?,
            ResumeMode::Return => self.get_method(&iterator, return_key)?,
        };
        let Some(method) = method else {
            if mode == ResumeMode::Return {
                return Ok(YieldStarStep::MissingReturn);
            }
            // A missing `throw` closes the delegate (awaited when async), then throws.
            if let Some(close) = self.get_method(&iterator, return_key)? {
                let result = self.call_value(&close, &iterator, &[])?;
                if is_async {
                    return Ok(YieldStarStep::AwaitClose(result));
                }
                if !matches!(result, Value::Object(_)) {
                    return self.throw_type("Iterator result is not an object");
                }
            }
            return self.throw_type(MISSING_THROW);
        };
        self.state.acc = self.call_value(&method, &iterator, &[received])?;
        Ok(YieldStarStep::Called)
    }

    /// Awaits a delegate's `return()` result, then throws at the `yield*`. The compiler
    /// emits the await right after `YieldStarCall` in async generators, so the frame parks
    /// at its `Resume`, which throws the error the continuation resumes it with.
    pub(super) fn await_delegate_close(
        &mut self,
        info: &CodeInfo,
        result: Value,
    ) -> JsResult<Flow> {
        let cell = self.frame_generator(info)?;
        let pc = self.top().pc as usize;
        let Some(I::Suspend { mode, .. }) = info.code.instructions.get(pc).cloned() else {
            return Err(self.internal("yield* in an async generator without its await"));
        };
        self.top_mut().pc += 1;
        self.scoped(|vm| {
            let promise = vm.resolve_intrinsic_promise(result)?;
            vm.save_frame(&cell, Some(mode));
            cell.borrow_mut().awaiting = true;
            let resume = [
                Continuation::CloseDelegate,
                Continuation::Body(ResumeMode::Throw),
            ];
            vm.await_promise(&cell, &promise, resume)
        })?;
        self.state.acc = Value::Undefined;
        Ok(Flow::Return)
    }

    /// `YieldStarUnpack`: `true` when the delegate is done (`acc` = its value).
    pub(super) fn yield_star_unpack(&mut self, is_async: bool) -> JsResult<bool> {
        let result = self.state.acc.clone();
        let Some(object) = self.as_object(&result) else {
            return self.throw_type("Iterator result is not an object");
        };
        let done = to_boolean(&self.get_named(object, "done")?);
        if done || is_async {
            self.state.acc = self.get_named(object, "value")?;
        }
        Ok(done)
    }

    /// `AsyncIteratorClose`: `false` when there is nothing to call (no await follows).
    pub(super) fn async_iterator_close(&mut self, record: Reg) -> JsResult<bool> {
        let done_flag = Reg(record.0 + 2);
        if to_boolean(self.reg(done_flag)) {
            return Ok(false);
        }
        self.set_reg(done_flag, Value::Bool(true));
        let iterator = self.reg(record).clone();
        let key = self.realm.key_str("return");
        let Some(method) = self.get_method(&iterator, key)? else {
            return Ok(false);
        };
        self.state.acc = self.call_value(&method, &iterator, &[])?;
        Ok(true)
    }

    /// GetIterator(acc, async): sync iterables get an %AsyncFromSyncIteratorPrototype% wrapper.
    pub(super) fn open_async_iterator(&mut self, record: Reg) -> JsResult<()> {
        let iterable = self.state.acc.clone();
        let method = self.get_method(&iterable, ObjectKey::Symbol(SYMBOL_ASYNC_ITERATOR))?;
        let (iterator, next) = match method {
            Some(method) => self.open_iterator_with(&iterable, &method)?,
            None => {
                let (iterator, next) = self.open_iterator(&iterable)?;
                let proto = self.intrinsic(Intrinsic::AsyncFromSyncIteratorPrototype);
                let slot = Slot::AsyncFromSyncIterator(Rc::new([iterator, next]));
                let wrapper = self.new_slotted(proto, slot)?;
                (object_value(wrapper), self.get_named(wrapper, "next")?)
            }
        };
        self.set_reg(record, iterator);
        self.set_reg(Reg(record.0 + 1), next);
        self.set_reg(Reg(record.0 + 2), Value::Bool(false));
        Ok(())
    }

    /// %AsyncFromSyncIteratorPrototype% next/return/throw.
    pub(super) fn async_from_sync(
        &mut self,
        this: &Value,
        mode: ResumeMode,
        args: &[Value],
    ) -> JsResult<Value> {
        let capability = self.new_result_capability()?;
        let Some(Slot::AsyncFromSyncIterator(pair)) = self.value_slot(this) else {
            return self.throw_type("not an async-from-sync iterator");
        };
        let [iterator, next] = (*pair).clone();
        // `return`/`throw` forward their argument only when one was passed.
        let passed = &args[..args.len().min(1)];
        let result = match mode {
            ResumeMode::Next => self.call_value(&next, &iterator, passed),
            _ => self.async_from_sync_abrupt(&iterator, mode, passed),
        };
        let close_on_rejection = mode != ResumeMode::Return;
        let continued = result.and_then(|result| {
            self.async_from_sync_continuation(&result, &capability, &iterator, close_on_rejection)
        });
        self.reject_if_abrupt(continued.map(|()| capability.promise.clone()), &capability)
    }

    /// The sync `return`/`throw` call; a missing `throw` closes the iterator, then throws.
    fn async_from_sync_abrupt(
        &mut self,
        iterator: &Value,
        mode: ResumeMode,
        passed: &[Value],
    ) -> JsResult<Value> {
        let (throw_key, return_key) = (self.realm.key_str("throw"), self.realm.key_str("return"));
        let key = if mode == ResumeMode::Return {
            return_key
        } else {
            throw_key
        };
        if let Some(method) = self.get_method(iterator, key)? {
            return self.call_value(&method, iterator, passed);
        }
        if mode == ResumeMode::Return {
            return self.iter_result(arg(passed, 0), true);
        }
        if let Some(close) = self.get_method(iterator, return_key)? {
            let result = self.call_value(&close, iterator, &[])?;
            if !matches!(result, Value::Object(_)) {
                return self.throw_type("Iterator result is not an object");
            }
        }
        self.throw_type(MISSING_THROW)
    }

    /// AsyncFromSyncIteratorContinuation: resolves `capability` with the awaited value; with
    /// `close_on_rejection`, a rejected (or unresolvable) value closes the sync iterator.
    fn async_from_sync_continuation(
        &mut self,
        result: &Value,
        capability: &Capability,
        iterator: &Value,
        close_on_rejection: bool,
    ) -> JsResult<()> {
        self.keep(result);
        let (done, value) = self.unpack_iter_result(result)?;
        let wrapper = match self.resolve_intrinsic_promise(value) {
            Err(abrupt) if !done && close_on_rejection => {
                return Err(self.close_after_abrupt(iterator, abrupt));
            }
            wrapper => wrapper?,
        };
        let unwrap: NativeFn = Rc::new(move |vm, _, args| vm.iter_result(arg(args, 0), done));
        let on_fulfilled = object_value(self.create_native_function("", 1, unwrap, false)?);
        self.keep(&on_fulfilled);
        let on_rejected = if !done && close_on_rejection {
            let captured = iterator.clone();
            let close: NativeFn = Rc::new(move |vm, _, args| {
                Err(vm.close_after_abrupt(&captured, Abrupt::Throw(arg(args, 0))))
            });
            let function = self.create_native_function("", 1, close, false)?;
            // The slot keeps the captured iterator traced while the promise is pending.
            let slot = Slot::AsyncFromSyncIterator(Rc::new([iterator.clone(), Value::Undefined]));
            self.set_slot(function, slot);
            let function = object_value(function);
            self.keep(&function);
            function
        } else {
            Value::Undefined
        };
        let record = self
            .promise_record(&wrapper)
            .expect("PromiseResolve returns a promise");
        self.perform_then(&record, &on_fulfilled, &on_rejected, capability.clone());
        Ok(())
    }
}
