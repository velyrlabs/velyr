//! Generators, async functions, async generators and `for await` (VELYR-109).
//!
//! A coroutine function starts with `CreateGenerator`: async functions before binding
//! their parameters (a throwing default rejects the promise), generators after it (it
//! throws at the call). `yield` and `await` compile to `Suspend` followed by `Resume`,
//! which throws or returns (through the finally blocks) for `throw`/`return` resumptions.
//! An async function body is covered by a catch-all handler that rejects its promise.

use oxc_ast::ast::{AwaitExpression, Expression, YieldExpression};
use velyr_engine_vm::bytecode::{CoroutineKind, Instruction as I, Reg, ResumeMode, SuspendKind};

use super::emitter::Label;
use super::function::FunctionCompiler;
use super::unsupported;
use crate::compiler::JsResult;

#[derive(Debug, Clone, Copy)]
pub struct Coroutine {
    pub kind: CoroutineKind,
    /// Resume mode of `yield`/`await`.
    pub mode: Reg,
}

pub fn coroutine_kind(is_generator: bool, is_async: bool) -> Option<CoroutineKind> {
    match (is_generator, is_async) {
        (true, true) => Some(CoroutineKind::AsyncGenerator),
        (true, false) => Some(CoroutineKind::Generator),
        (false, true) => Some(CoroutineKind::Async),
        (false, false) => None,
    }
}

impl FunctionCompiler<'_> {
    /// Emits `CreateGenerator` when `kind` starts at this point of the prologue
    /// (`at_entry`: before the parameters). Returns where the async catch-all starts.
    pub fn start_coroutine(
        &mut self,
        kind: Option<CoroutineKind>,
        at_entry: bool,
    ) -> Option<Label> {
        let kind = kind.filter(|kind| (*kind == CoroutineKind::Async) == at_entry)?;
        let generator = self.e.alloc();
        let mode = self.e.alloc();
        self.coroutine = Some(Coroutine { kind, mode });
        self.e.emit(I::CreateGenerator { generator, kind });
        Some(self.e.bound_label())
    }

    /// Closes the async catch-all opened by `start_coroutine`.
    pub fn finish_async_function(&mut self, start: Option<Label>) {
        let Some(start) = start else { return };
        let end = self.e.bound_label();
        let handler = self.e.bound_label();
        self.e.add_handler(start, end, handler, None);
        self.e.emit(I::AsyncFunctionReject);
    }

    fn is_async_generator(&self) -> bool {
        self.coroutine
            .is_some_and(|c| c.kind == CoroutineKind::AsyncGenerator)
    }

    /// Async generators await the operand of `yield` and `return`.
    pub fn await_in_async_generator(&mut self) -> JsResult<()> {
        if self.is_async_generator() {
            self.emit_await()?;
        }
        Ok(())
    }

    /// Await(acc): `acc` = the fulfillment value, or throws the rejection reason.
    pub fn emit_await(&mut self) -> JsResult<()> {
        let mode = match self.coroutine {
            Some(c) if c.kind != CoroutineKind::Generator => c.mode,
            _ => return Err(unsupported("await outside an async function")),
        };
        self.e.emit(I::Suspend {
            mode,
            kind: SuspendKind::Await,
        });
        let next = self.e.new_label();
        self.e.jump(|on_return| I::Resume { mode, on_return }, next);
        self.e.bind(next);
        Ok(())
    }

    pub fn await_expression(&mut self, expression: &AwaitExpression) -> JsResult<()> {
        self.expression(&expression.argument)?;
        self.emit_await()
    }

    pub fn yield_expression(&mut self, expression: &YieldExpression) -> JsResult<()> {
        let Some(coroutine) = self.coroutine.filter(|c| c.kind != CoroutineKind::Async) else {
            return Err(unsupported("yield outside a generator"));
        };
        if expression.delegate {
            let argument = expression.argument.as_ref();
            return self.yield_star(argument.expect("yield* has an operand"));
        }
        match &expression.argument {
            Some(argument) => self.expression(argument)?,
            None => self.e.emit(I::LdaUndefined),
        }
        self.await_in_async_generator()?;
        let mode = coroutine.mode;
        self.e.emit(I::Suspend {
            mode,
            kind: SuspendKind::Yield,
        });
        let (on_return, after) = (self.e.new_label(), self.e.new_label());
        self.e
            .jump(|on_return| I::Resume { mode, on_return }, on_return);
        self.e.jump(I::Jump, after);
        self.e.bind(on_return);
        self.return_resumption()?;
        self.e.bind(after);
        Ok(())
    }

    /// A `return` resumption with the value in `acc` (awaited by async generators).
    fn return_resumption(&mut self) -> JsResult<()> {
        self.await_in_async_generator()?;
        self.return_value();
        Ok(())
    }

    /// `yield*`: forwards next/throw/return to the delegate until it is done; the loop
    /// keeps its own resume mode so the `await`s inside it do not clobber it.
    fn yield_star(&mut self, argument: &Expression) -> JsResult<()> {
        let is_async = self.is_async_generator();
        self.expression(argument)?;
        let record = self.e.alloc_range(3).first;
        let mode = self.e.alloc();
        let value = self.e.alloc();
        self.open_loop_iterator(record, is_async);
        self.e.emit(I::LdaSmi(ResumeMode::Next as i32));
        self.e.emit(I::Star(mode));
        self.e.emit(I::LdaUndefined);
        let head = self.e.bound_label();
        let (done, on_return, exit) = (self.e.new_label(), self.e.new_label(), self.e.new_label());
        self.e.jump(
            |on_return| I::YieldStarCall {
                record,
                mode,
                on_return,
            },
            on_return,
        );
        if is_async {
            // `YieldStarCall` relies on this await following it (a delegate without `throw`).
            self.emit_await()?;
        }
        self.e
            .jump(|done| I::YieldStarUnpack { record, done }, done);
        let kind = match is_async {
            true => SuspendKind::Yield,
            false => SuspendKind::Delegate,
        };
        self.e.emit(I::Suspend { mode, kind });
        if is_async {
            self.await_return_resumption(mode, value, head)?;
        }
        self.e.jump(I::Jump, head);
        self.e.bind(done);
        self.e.emit(I::Star(value));
        self.e.emit(I::LdaSmi(ResumeMode::Return as i32));
        self.e.emit(I::TestEqualStrict(mode));
        self.e.jump(I::JumpIfFalse, exit);
        self.e.emit(I::Ldar(value));
        self.e.bind(on_return);
        self.return_resumption()?;
        self.e.bind(exit);
        self.e.emit(I::Ldar(value));
        Ok(())
    }

    /// Async `yield*` resumed with `return(v)`: awaits `v` first; a rejection turns the
    /// resumption into a `throw` of the reason (AsyncGeneratorUnwrapYieldResumption).
    fn await_return_resumption(&mut self, mode: Reg, value: Reg, head: Label) -> JsResult<()> {
        let skip = self.e.new_label();
        self.e.emit(I::Star(value));
        self.e.emit(I::LdaSmi(ResumeMode::Return as i32));
        self.e.emit(I::TestEqualStrict(mode));
        self.e.jump(I::JumpIfFalse, skip);
        self.e.emit(I::Ldar(value));
        let context = self.save_context_for_handler();
        let start = self.e.bound_label();
        self.emit_await()?;
        let end = self.e.bound_label();
        self.e.jump(I::Jump, head);
        let handler = self.e.bound_label();
        self.e.add_handler(start, end, handler, context);
        self.e.emit(I::Star(value));
        self.e.emit(I::LdaSmi(ResumeMode::Throw as i32));
        self.e.emit(I::Star(mode));
        self.e.emit(I::Ldar(value));
        self.e.jump(I::Jump, head);
        self.e.bind(skip);
        self.e.emit(I::Ldar(value));
        Ok(())
    }

    /// GetIterator(acc, sync or async) into the 3-register `record`.
    pub fn open_loop_iterator(&mut self, record: Reg, is_async: bool) {
        self.e.emit(match is_async {
            true => I::GetAsyncIterator { record },
            false => I::GetIterator { record },
        });
    }

    /// One loop step: `acc` = the next value, or jumps to `done`.
    pub fn step_loop_iterator(&mut self, record: Reg, done: Label, is_async: bool) -> JsResult<()> {
        if !is_async {
            self.e.jump(|done| I::IteratorStep { record, done }, done);
            return Ok(());
        }
        self.e.emit(I::IteratorNext { record });
        self.emit_await()?;
        self.e.jump(|done| I::IteratorUnpack { record, done }, done);
        Ok(())
    }
}
