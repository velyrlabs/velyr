//! Generators, async functions and async generators (VELYR-109). A suspended frame lives
//! on the heap, in the generator object's internal slot: its registers, pc and context.
//! Resuming pushes it back as an entry frame and runs the dispatch loop until the next
//! `Suspend` or the end of the body. Async functions are generators driven by promise
//! reactions; async generators add a queue of `next`/`throw`/`return` requests.
//!
//! Lives in `interpreter` (as `interpreter::generator`) for access to the frame internals.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use velyr_engine_gc::{Trace, Tracer};

use super::builtins::promise_ops::Capability;
use super::builtins::slots::Slot;
use super::builtins::{Intrinsic, arg};
use super::dispatch::Flow;
use super::function::{FunctionEnv, NativeFn};
use super::state::{CodeInfo, Frame};
use super::{Abrupt, Interpreter, JsResult, MAX_FRAMES, object_value};
use crate::bytecode::{CoroutineKind, Instruction as I, Reg, ResumeMode, SuspendKind};
use crate::{ObjectRef, SymbolId, Value};

#[path = "generator/async_queue.rs"]
mod async_queue;
#[path = "generator/intrinsics.rs"]
mod intrinsics;
#[path = "generator/iteration.rs"]
mod iteration;

pub(crate) use intrinsics::install;
use iteration::YieldStarStep;

const SYMBOL_ASYNC_ITERATOR: SymbolId = SymbolId(3);

struct SavedFrame {
    info: Rc<CodeInfo>,
    pc: u32,
    context: Value,
    env: FunctionEnv,
    function: Value,
    arguments: Vec<Value>,
    registers: Vec<Value>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    SuspendedStart,
    SuspendedYield,
    Executing,
    /// Async generator awaiting the operand of a `return` request after completion.
    AwaitingReturn,
    Completed,
}

/// AsyncGeneratorRequest Record.
struct Request {
    mode: ResumeMode,
    value: Value,
    capability: Capability,
}

pub(crate) struct GeneratorState {
    kind: CoroutineKind,
    status: Status,
    /// The suspended frame; `None` while it runs and once completed.
    frame: Option<SavedFrame>,
    /// `mode` register of the `Suspend` the frame is parked at.
    mode: Option<Reg>,
    /// The frame is parked at an `await` (the body is still executing).
    awaiting: bool,
    /// The frame yielded a `yield*` delegate's iterator result, to return as is.
    delegated: bool,
    /// Async functions: the promise their call returned.
    promise: Option<Capability>,
    /// Async generators: pending requests, oldest first.
    queue: VecDeque<Request>,
}

/// Never borrowed across an allocation or a call: the GC borrows it while tracing.
pub(crate) type GeneratorCell = RefCell<GeneratorState>;

impl Trace for GeneratorState {
    fn trace(&self, tracer: &mut Tracer) {
        if let Some(frame) = &self.frame {
            frame.context.trace(tracer);
            frame.env.trace(tracer);
            frame.function.trace(tracer);
            frame.arguments.trace(tracer);
            frame.registers.trace(tracer);
        }
        self.promise.trace(tracer);
        for request in &self.queue {
            request.value.trace(tracer);
            request.capability.trace(tracer);
        }
    }
}

/// What a promise reaction of a suspended coroutine does with the settled value.
#[derive(Clone, Copy)]
enum Continuation {
    /// Resumes the body (an `await`).
    Body(ResumeMode),
    /// Settles the front request of an async generator awaiting a `return` operand.
    Return { rejects: bool },
    /// The awaited `return()` result of a `yield*` delegate without `throw`: resumes the
    /// body with a TypeError thrown.
    CloseDelegate,
}

fn mode_of(value: &Value) -> ResumeMode {
    match value {
        Value::Number(n) if *n == ResumeMode::Throw as i32 as f64 => ResumeMode::Throw,
        Value::Number(n) if *n == ResumeMode::Return as i32 as f64 => ResumeMode::Return,
        _ => ResumeMode::Next,
    }
}

impl Interpreter {
    pub(crate) fn step_generator(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<Flow> {
        match instruction {
            I::CreateGenerator { generator, kind } => {
                return self.create_generator(*generator, *kind);
            }
            I::Suspend { mode, kind } => return self.suspend(info, *mode, *kind),
            I::AsyncFunctionReject => return self.settle_async_function(info, true),
            I::Resume { mode, on_return } => match mode_of(self.reg(*mode)) {
                ResumeMode::Throw => return Err(Abrupt::Throw(self.state.acc.clone())),
                ResumeMode::Return => self.top_mut().pc = on_return.0,
                ResumeMode::Next => {}
            },
            I::GetAsyncIterator { record } => self.scoped(|vm| vm.open_async_iterator(*record))?,
            I::IteratorNext { record } => {
                self.set_reg(Reg(record.0 + 2), Value::Bool(true));
                let (iterator, next) = (
                    self.reg(*record).clone(),
                    self.reg(Reg(record.0 + 1)).clone(),
                );
                self.state.acc = self.call_value(&next, &iterator, &[])?;
            }
            I::IteratorUnpack { record, done } => {
                let (is_done, value) = self.unpack_iter_result(&self.state.acc.clone())?;
                self.set_reg(Reg(record.0 + 2), Value::Bool(is_done));
                self.state.acc = value;
                if is_done {
                    self.top_mut().pc = done.0;
                }
            }
            I::YieldStarCall {
                record,
                mode,
                on_return,
            } => {
                let mode = mode_of(self.reg(*mode));
                let is_async = matches!(info.coroutine, Some((CoroutineKind::AsyncGenerator, _)));
                match self.yield_star_call(*record, mode, is_async)? {
                    YieldStarStep::Called => {}
                    YieldStarStep::MissingReturn => self.top_mut().pc = on_return.0,
                    YieldStarStep::AwaitClose(result) => {
                        return self.await_delegate_close(info, result);
                    }
                }
            }
            I::YieldStarUnpack { done, .. } => {
                let is_async = matches!(info.coroutine, Some((CoroutineKind::AsyncGenerator, _)));
                if self.yield_star_unpack(is_async)? {
                    self.top_mut().pc = done.0;
                }
            }
            I::AsyncIteratorClose { record, done } => {
                if !self.async_iterator_close(*record)? {
                    self.top_mut().pc = done.0;
                }
            }
            I::CheckIteratorResult => {
                if !matches!(self.state.acc, Value::Object(_)) {
                    return self.throw_type("Iterator result is not an object");
                }
            }
            other => return Err(self.internal(&format!("unexpected instruction {other:?}"))),
        }
        Ok(Flow::Next)
    }

    /// `Return` of a coroutine frame: an async function settles and returns its promise.
    pub(crate) fn coroutine_return(&mut self, info: &CodeInfo) -> JsResult<Flow> {
        match info.coroutine {
            Some((CoroutineKind::Async, _)) => self.settle_async_function(info, false),
            _ => Ok(Flow::Return),
        }
    }

    /// Runs `f` with the temporaries it roots (`keep`) dropped afterwards, for instructions.
    pub(crate) fn scoped<T>(&mut self, f: impl FnOnce(&mut Self) -> JsResult<T>) -> JsResult<T> {
        let mark = self.state.scratch.len();
        let result = f(self);
        self.state.scratch.truncate(mark);
        result
    }

    fn create_generator(&mut self, register: Reg, kind: CoroutineKind) -> JsResult<Flow> {
        let cell = self.scoped(|vm| {
            let object = match kind {
                CoroutineKind::Async => vm.new_object_with(None)?,
                CoroutineKind::Generator => vm.instance_object(Intrinsic::GeneratorPrototype)?,
                CoroutineKind::AsyncGenerator => {
                    vm.instance_object(Intrinsic::AsyncGeneratorPrototype)?
                }
            };
            let promise = match kind {
                CoroutineKind::Async => Some(vm.new_result_capability()?),
                _ => None,
            };
            let cell = Rc::new(RefCell::new(GeneratorState {
                kind,
                status: Status::Executing,
                frame: None,
                mode: None,
                awaiting: false,
                delegated: false,
                promise,
                queue: VecDeque::new(),
            }));
            vm.set_slot(object, Slot::Generator(cell.clone()));
            vm.set_reg(register, object_value(object));
            Ok(cell)
        })?;
        if kind == CoroutineKind::Async {
            return Ok(Flow::Next);
        }
        cell.borrow_mut().status = Status::SuspendedStart;
        self.save_frame(&cell, None);
        self.state.acc = self.reg(register).clone();
        Ok(Flow::Return)
    }

    /// OrdinaryCreateFromConstructor(running function, `fallback`), rooted.
    fn instance_object(&mut self, fallback: Intrinsic) -> JsResult<ObjectRef> {
        let function = self.top().function.clone();
        let key = self.realm.key_str("prototype");
        let proto = self.get_value(&function, key, None)?;
        let proto = self.as_object(&proto).unwrap_or(self.intrinsic(fallback));
        self.new_object_with(Some(proto))
    }

    /// A pending `%Promise%` with its resolving functions, rooted.
    fn new_result_capability(&mut self) -> JsResult<Capability> {
        let (promise, record) = self.new_promise(self.intrinsic(Intrinsic::PromisePrototype))?;
        let (resolve, reject) = self.resolving_functions(&record)?;
        Ok(Capability {
            promise,
            resolve,
            reject,
        })
    }

    fn frame_generator(&mut self, info: &CodeInfo) -> JsResult<Rc<GeneratorCell>> {
        let register = info.coroutine.map(|(_, register)| register);
        match register.and_then(|r| self.value_slot(self.reg(r))) {
            Some(Slot::Generator(cell)) => Ok(cell),
            _ => Err(self.internal("coroutine frame without a generator")),
        }
    }

    fn save_frame(&mut self, cell: &GeneratorCell, mode: Option<Reg>) {
        let frame = self.top();
        let registers = frame.base..frame.base + frame.info.code.register_count as usize;
        let saved = SavedFrame {
            info: frame.info.clone(),
            pc: frame.pc,
            context: frame.context.clone(),
            env: frame.env.clone(),
            function: frame.function.clone(),
            arguments: frame.arguments.clone(),
            registers: self.state.registers[registers].to_vec(),
        };
        let mut state = cell.borrow_mut();
        state.frame = Some(saved);
        state.mode = mode;
    }

    fn suspend(&mut self, info: &CodeInfo, mode: Reg, kind: SuspendKind) -> JsResult<Flow> {
        let cell = self.frame_generator(info)?;
        let value = self.state.acc.clone();
        self.scoped(|vm| {
            if kind == SuspendKind::Await {
                // Await: a throwing PromiseResolve throws at the `await`, before suspending.
                let promise = vm.resolve_intrinsic_promise(value.clone())?;
                vm.save_frame(&cell, Some(mode));
                cell.borrow_mut().awaiting = true;
                let resume = [ResumeMode::Next, ResumeMode::Throw].map(Continuation::Body);
                return vm.await_promise(&cell, &promise, resume);
            }
            vm.save_frame(&cell, Some(mode));
            let mut state = cell.borrow_mut();
            state.awaiting = false;
            state.delegated = kind == SuspendKind::Delegate;
            state.status = Status::SuspendedYield;
            Ok(())
        })?;
        let promise = cell.borrow().promise.as_ref().map(|c| c.promise.clone());
        self.state.acc = promise.unwrap_or(value);
        Ok(Flow::Return)
    }

    /// PromiseResolve(%Promise%, value), rooted.
    fn resolve_intrinsic_promise(&mut self, value: Value) -> JsResult<Value> {
        let constructor = object_value(self.intrinsic(Intrinsic::PromiseConstructor));
        let promise = self.promise_resolve(&constructor, value)?;
        self.keep(&promise);
        Ok(promise)
    }

    /// PerformPromiseThen(promise, continuations) without a result capability.
    fn await_promise(
        &mut self,
        cell: &Rc<GeneratorCell>,
        promise: &Value,
        continuations: [Continuation; 2],
    ) -> JsResult<()> {
        let Some(record) = self.promise_record(promise) else {
            return Err(self.internal("PromiseResolve returned a non-promise"));
        };
        let [on_fulfilled, on_rejected] = continuations;
        let on_fulfilled = self.continuation_function(cell, on_fulfilled)?;
        let on_rejected = self.continuation_function(cell, on_rejected)?;
        // No result capability: settling goes to a no-op callable (%Function.prototype%).
        let noop = object_value(self.state.intrinsics.function_prototype);
        let none = Capability {
            promise: Value::Undefined,
            resolve: noop.clone(),
            reject: noop,
        };
        self.perform_then(&record, &on_fulfilled, &on_rejected, none);
        Ok(())
    }

    /// A native reaction running `continuation`; its slot keeps the generator traced.
    fn continuation_function(
        &mut self,
        cell: &Rc<GeneratorCell>,
        continuation: Continuation,
    ) -> JsResult<Value> {
        let captured = cell.clone();
        let call: NativeFn = Rc::new(move |vm, _, args| {
            vm.continue_coroutine(&captured, continuation, arg(args, 0))
        });
        let function = self.create_native_function("", 1, call, false)?;
        self.set_slot(function, Slot::Generator(cell.clone()));
        let function = object_value(function);
        self.keep(&function);
        Ok(function)
    }

    fn continue_coroutine(
        &mut self,
        cell: &Rc<GeneratorCell>,
        continuation: Continuation,
        value: Value,
    ) -> JsResult<Value> {
        let kind = cell.borrow().kind;
        match (continuation, kind) {
            (Continuation::Body(mode), CoroutineKind::AsyncGenerator) => {
                self.run_async_generator(cell, mode, value)?;
            }
            (Continuation::Body(mode), _) => {
                if let Err(Abrupt::Throw(error)) = self.resume(cell, mode, value) {
                    // Only reachable when the frame could not be pushed (stack overflow).
                    let reject = cell.borrow().promise.as_ref().map(|c| c.reject.clone());
                    let reject = reject.unwrap_or(Value::Undefined);
                    self.call_value(&reject, &Value::Undefined, &[error])?;
                }
            }
            (Continuation::CloseDelegate, _) => {
                let message = match value {
                    Value::Object(_) => iteration::MISSING_THROW,
                    _ => "Iterator result is not an object",
                };
                let error = self.create_error(super::ErrorKind::Type, message)?;
                self.run_async_generator(cell, ResumeMode::Throw, error)?;
            }
            (Continuation::Return { rejects }, _) => {
                Self::complete(cell);
                self.settle_request(cell, value, rejects, true)?;
                self.drain_async_generator(cell)?;
            }
        }
        Ok(Value::Undefined)
    }

    /// Settles an async function's promise with `acc` and returns the promise.
    fn settle_async_function(&mut self, info: &CodeInfo, rejects: bool) -> JsResult<Flow> {
        let cell = self.frame_generator(info)?;
        Self::complete(&cell);
        let Some(capability) = cell.borrow().promise.clone() else {
            return Err(self.internal("async function without a promise"));
        };
        let settle = if rejects {
            &capability.reject
        } else {
            &capability.resolve
        };
        let value = self.state.acc.clone();
        self.call_value(settle, &Value::Undefined, &[value])?;
        self.state.acc = capability.promise;
        Ok(Flow::Return)
    }

    fn complete(cell: &GeneratorCell) {
        let mut state = cell.borrow_mut();
        state.status = Status::Completed;
        state.frame = None;
    }

    /// Pushes the suspended frame as an entry frame and runs it until it suspends again
    /// (the frame is saved) or completes (the result is its return value).
    fn resume(&mut self, cell: &GeneratorCell, mode: ResumeMode, value: Value) -> JsResult<Value> {
        self.check_termination()?;
        if self.state.frames.len() >= MAX_FRAMES {
            return Err(self.stack_overflow());
        }
        self.enter_reentry()?;
        let (saved, mode_register) = {
            let mut state = cell.borrow_mut();
            state.status = Status::Executing;
            (
                state.frame.take().expect("a suspended frame"),
                state.mode.take(),
            )
        };
        let base = self.state.registers.len();
        self.state.registers.extend(saved.registers);
        self.state.frames.push(Frame {
            info: saved.info,
            pc: saved.pc,
            base,
            context: saved.context,
            env: saved.env,
            function: saved.function,
            arguments: saved.arguments,
            constructed: None,
            is_entry: true,
        });
        if let Some(register) = mode_register {
            self.set_reg(register, Value::Number(f64::from(mode as i32)));
        }
        self.state.acc = value;
        let result = self.execute();
        self.reentry -= 1;
        result
    }

    fn is_suspended(cell: &GeneratorCell) -> bool {
        cell.borrow().frame.is_some()
    }

    fn generator_cell(&self, this: &Value, kind: CoroutineKind) -> Option<Rc<GeneratorCell>> {
        match self.value_slot(this) {
            Some(Slot::Generator(cell)) if cell.borrow().kind == kind => Some(cell),
            _ => None,
        }
    }

    /// GeneratorResume / GeneratorResumeAbrupt.
    fn generator_resume(
        &mut self,
        this: &Value,
        mode: ResumeMode,
        value: Value,
    ) -> JsResult<Value> {
        let Some(cell) = self.generator_cell(this, CoroutineKind::Generator) else {
            return self.throw_type("Generator method called on incompatible receiver");
        };
        let status = cell.borrow().status;
        match status {
            Status::Executing => return self.throw_type("Generator is already running"),
            Status::SuspendedStart if mode != ResumeMode::Next => Self::complete(&cell),
            _ => {}
        }
        if cell.borrow().status == Status::Completed {
            return match mode {
                ResumeMode::Next => self.iter_result(Value::Undefined, true),
                ResumeMode::Throw => Err(Abrupt::Throw(value)),
                ResumeMode::Return => self.iter_result(value, true),
            };
        }
        let result = self.resume(&cell, mode, value);
        if result.is_ok() && Self::is_suspended(&cell) {
            if cell.borrow().delegated {
                return result;
            }
            return self.iter_result(result?, false);
        }
        Self::complete(&cell);
        self.iter_result(result?, true)
    }
}
