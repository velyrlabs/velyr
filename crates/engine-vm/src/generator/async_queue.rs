//! Async generator request queue: AsyncGeneratorEnqueue, Drain, AwaitReturn, CompleteStep.

use std::rc::Rc;

use super::{Continuation, GeneratorCell, Request, Status};
use crate::Value;
use crate::bytecode::{CoroutineKind, ResumeMode};
use crate::interpreter::{Abrupt, Interpreter, JsResult};

impl Interpreter {
    /// AsyncGeneratorEnqueue (after AsyncGeneratorValidate) for `next`/`throw`/`return`.
    pub(super) fn async_generator_enqueue(
        &mut self,
        this: &Value,
        mode: ResumeMode,
        value: Value,
    ) -> JsResult<Value> {
        let capability = self.new_result_capability()?;
        let Some(cell) = self.generator_cell(this, CoroutineKind::AsyncGenerator) else {
            let message = "AsyncGenerator method called on incompatible receiver";
            let error = self.throw_type::<()>(message);
            return self.reject_if_abrupt(error.map(|()| Value::Undefined), &capability);
        };
        let request = Request {
            mode,
            value,
            capability: capability.clone(),
        };
        cell.borrow_mut().queue.push_back(request);
        let status = cell.borrow().status;
        if !matches!(status, Status::Executing | Status::AwaitingReturn) {
            self.drain_async_generator(&cell)?;
        }
        Ok(capability.promise)
    }

    /// Serves queued requests until the body runs, awaits, or the queue is empty.
    pub(super) fn drain_async_generator(&mut self, cell: &Rc<GeneratorCell>) -> JsResult<()> {
        loop {
            let (status, front) = {
                let state = cell.borrow();
                let front = state.queue.front().map(|r| (r.mode, r.value.clone()));
                (state.status, front)
            };
            let Some((mode, value)) = front else {
                return Ok(());
            };
            match (status, mode) {
                (Status::Executing | Status::AwaitingReturn, _) => return Ok(()),
                (Status::SuspendedYield, _) | (Status::SuspendedStart, ResumeMode::Next) => {
                    return self.run_async_generator(cell, mode, value);
                }
                (_, ResumeMode::Next) => {
                    self.settle_request(cell, Value::Undefined, false, true)?
                }
                (_, ResumeMode::Throw) => {
                    Self::complete(cell);
                    self.settle_request(cell, value, true, true)?;
                }
                (_, ResumeMode::Return) => {
                    Self::complete(cell);
                    return self.await_return(cell, value);
                }
            }
        }
    }

    /// AsyncGeneratorAwaitReturn.
    pub(super) fn await_return(&mut self, cell: &Rc<GeneratorCell>, value: Value) -> JsResult<()> {
        cell.borrow_mut().status = Status::AwaitingReturn;
        match self.resolve_intrinsic_promise(value) {
            Ok(promise) => {
                let settle = [false, true].map(|rejects| Continuation::Return { rejects });
                self.await_promise(cell, &promise, settle)
            }
            Err(Abrupt::Throw(error)) => {
                Self::complete(cell);
                self.settle_request(cell, error, true, true)?;
                self.drain_async_generator(cell)
            }
            Err(abrupt) => Err(abrupt),
        }
    }

    pub(super) fn run_async_generator(
        &mut self,
        cell: &Rc<GeneratorCell>,
        mode: ResumeMode,
        value: Value,
    ) -> JsResult<()> {
        let result = self.resume(cell, mode, value);
        let is_suspended = Self::is_suspended(cell);
        match result {
            Ok(_) if is_suspended && cell.borrow().awaiting => return Ok(()),
            Ok(value) if is_suspended => self.settle_request(cell, value, false, false)?,
            Ok(value) => {
                Self::complete(cell);
                self.settle_request(cell, value, false, true)?;
            }
            Err(Abrupt::Throw(error)) => {
                Self::complete(cell);
                self.settle_request(cell, error, true, true)?;
            }
            Err(abrupt) => return Err(abrupt),
        }
        self.drain_async_generator(cell)
    }

    /// AsyncGeneratorCompleteStep: settles (and dequeues) the oldest request.
    pub(super) fn settle_request(
        &mut self,
        cell: &GeneratorCell,
        value: Value,
        rejects: bool,
        done: bool,
    ) -> JsResult<()> {
        let Some(request) = cell.borrow_mut().queue.pop_front() else {
            return Ok(());
        };
        let capability = request.capability;
        for part in [&capability.promise, &capability.resolve, &capability.reject] {
            self.keep(part);
        }
        let (settle, argument) = match rejects {
            true => (capability.reject, value),
            false => (capability.resolve, self.iter_result(value, done)?),
        };
        self.call_value(&settle, &Value::Undefined, &[argument])?;
        Ok(())
    }
}
