//! Jumps out of nested constructs (break/continue/return) and try/catch/finally.
//!
//! A jump walks the control stack from the innermost entry to its target, emitting the
//! cleanup of each entry it leaves: `PopContext` for scopes, `IteratorClose` for for-of
//! loops. A `finally` entry stops the walk: the jump records a token, runs the finally
//! block, and the dispatch after the block resumes the jump from the outer level.

use oxc_ast::ast::{CatchClause, TryStatement};
use velyr_engine_vm::bytecode::{Instruction as I, Reg};

use super::emitter::Label;
use super::function::FunctionCompiler;
use crate::compiler::{JsResult, throw};

const TOKEN_FALLTHROUGH: i32 = 0;
const TOKEN_THROW: i32 = 1;
const FIRST_JUMP_TOKEN: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakableKind {
    Loop,
    Switch,
    Block,
}

pub struct Breakable {
    pub labels: Vec<String>,
    pub kind: BreakableKind,
    pub break_label: Label,
    pub continue_label: Option<Label>,
    /// Iterator record of a for-of loop (and whether it is `for await`), closed when a
    /// jump leaves the loop.
    pub iterator: Option<(Reg, bool)>,
}

pub struct Finally {
    token: Reg,
    value: Reg,
    entry: Label,
    jumps: Vec<Jump>,
}

pub enum Control {
    /// A pushed context; the register holds the outer one.
    Context(Reg),
    Breakable(Breakable),
    Finally(Finally),
}

#[derive(Debug, Clone, Copy)]
enum Jump {
    Break(usize),
    Continue(usize),
    /// Return of the value held in the register.
    Return(Reg),
}

impl FunctionCompiler<'_> {
    pub fn break_statement(&mut self, label: Option<&str>) -> JsResult<()> {
        let target = self.jump_target(label, false)?;
        self.unwind(Jump::Break(target));
        Ok(())
    }

    pub fn continue_statement(&mut self, label: Option<&str>) -> JsResult<()> {
        let target = self.jump_target(label, true)?;
        self.unwind(Jump::Continue(target));
        Ok(())
    }

    /// Returns the value in `acc`, running enclosing finally blocks and closing iterators.
    pub fn return_value(&mut self) {
        let needs_unwind = self.control.iter().any(|entry| match entry {
            Control::Finally(_) => true,
            Control::Breakable(breakable) => breakable.iterator.is_some(),
            Control::Context(_) => false,
        });
        if !needs_unwind {
            self.e.emit(I::Return);
            return;
        }
        let value = self.e.alloc();
        self.e.emit(I::Star(value));
        self.unwind(Jump::Return(value));
    }

    fn jump_target(&self, label: Option<&str>, is_continue: bool) -> JsResult<usize> {
        self.control
            .iter()
            .rposition(|entry| {
                let Control::Breakable(breakable) = entry else {
                    return false;
                };
                let kind_matches = match (is_continue, breakable.kind) {
                    (true, kind) => kind == BreakableKind::Loop,
                    (false, kind) => label.is_some() || kind != BreakableKind::Block,
                };
                kind_matches && label.is_none_or(|name| breakable.labels.iter().any(|l| l == name))
            })
            .ok_or_else(|| throw("SyntaxError", "illegal break or continue target"))
    }

    fn unwind(&mut self, jump: Jump) {
        let target = match jump {
            Jump::Break(index) | Jump::Continue(index) => Some(index),
            Jump::Return(_) => None,
        };
        for index in (0..self.control.len()).rev() {
            if Some(index) == target {
                self.jump_to_breakable(index, jump);
                return;
            }
            match &mut self.control[index] {
                Control::Context(saved) => {
                    // A return leaves the frame, and its contexts with it.
                    if !matches!(jump, Jump::Return(_)) {
                        let saved = *saved;
                        self.e.emit(I::PopContext(saved));
                    }
                }
                Control::Breakable(breakable) => {
                    if let Some((record, is_async)) = breakable.iterator {
                        self.close_loop_iterator(record, is_async);
                    }
                }
                Control::Finally(finally) => {
                    let token = FIRST_JUMP_TOKEN + finally.jumps.len() as i32;
                    let (token_register, value, entry) =
                        (finally.token, finally.value, finally.entry);
                    let resumed = match jump {
                        Jump::Return(_) => Jump::Return(value),
                        other => other,
                    };
                    finally.jumps.push(resumed);
                    if let Jump::Return(register) = jump {
                        self.e.emit(I::Mov {
                            from: register,
                            to: value,
                        });
                    }
                    self.e.emit(I::LdaSmi(token));
                    self.e.emit(I::Star(token_register));
                    self.e.jump(I::Jump, entry);
                    return;
                }
            }
        }
        if let Jump::Return(register) = jump {
            self.e.emit(I::Ldar(register));
            self.e.emit(I::Return);
        }
    }

    fn jump_to_breakable(&mut self, index: usize, jump: Jump) {
        let Control::Breakable(breakable) = &self.control[index] else {
            unreachable!("jump targets are breakable entries");
        };
        let (break_label, continue_label, iterator) = (
            breakable.break_label,
            breakable.continue_label,
            breakable.iterator,
        );
        match (jump, continue_label) {
            (Jump::Continue(_), Some(label)) => self.e.jump(I::Jump, label),
            _ => {
                if let Some((record, is_async)) = iterator {
                    self.close_loop_iterator(record, is_async);
                }
                self.e.jump(I::Jump, break_label);
            }
        }
    }

    /// IteratorClose, or AsyncIteratorClose (awaiting the `return()` result) for `for await`.
    fn close_loop_iterator(&mut self, record: Reg, is_async: bool) {
        if !is_async {
            self.e.emit(I::IteratorClose { record });
            return;
        }
        let closed = self.e.new_label();
        self.e
            .jump(|done| I::AsyncIteratorClose { record, done }, closed);
        // The loop's own step already awaited, so this frame is a coroutine.
        self.emit_await()
            .expect("for await compiles only inside async functions");
        self.e.emit(I::CheckIteratorResult);
        self.e.bind(closed);
    }

    /// Saves the current context for a handler when this function can push contexts.
    pub fn save_context_for_handler(&mut self) -> Option<Reg> {
        if !self.analysis.has_inner_contexts(self.scope) {
            return None;
        }
        let register = self.e.alloc();
        self.e.emit(I::SaveContext(register));
        Some(register)
    }

    pub fn try_statement(&mut self, statement: &TryStatement) -> JsResult<()> {
        self.reset_completion();
        let Some(finalizer) = &statement.finalizer else {
            return self.try_catch(statement);
        };
        let token = self.e.alloc();
        let value = self.e.alloc();
        let context = self.save_context_for_handler();
        let entry = self.e.new_label();
        self.control.push(Control::Finally(Finally {
            token,
            value,
            entry,
            jumps: Vec::new(),
        }));
        let start = self.e.bound_label();
        let protected = self.try_catch(statement);
        let end = self.e.bound_label();
        let Some(Control::Finally(finally)) = self.control.pop() else {
            unreachable!("finally entry pushed above");
        };
        protected?;
        self.e.emit(I::LdaSmi(TOKEN_FALLTHROUGH));
        self.e.emit(I::Star(token));
        self.e.jump(I::Jump, entry);
        let handler = self.e.bound_label();
        self.e.add_handler(start, end, handler, context);
        self.e.emit(I::Star(value));
        self.e.emit(I::LdaSmi(TOKEN_THROW));
        self.e.emit(I::Star(token));
        self.e.bind(entry);
        // A normally completing finally block keeps the try/catch completion value.
        let completion = self.completion.take();
        let result = self.block(finalizer);
        self.completion = completion;
        result?;
        self.finally_dispatch(token, value, &finally.jumps);
        Ok(())
    }

    fn finally_dispatch(&mut self, token: Reg, value: Reg, jumps: &[Jump]) {
        let resumptions = jumps
            .iter()
            .enumerate()
            .map(|(index, jump)| (FIRST_JUMP_TOKEN + index as i32, Some(*jump)));
        for (token_value, jump) in resumptions.chain([(TOKEN_THROW, None)]) {
            let next = self.e.new_label();
            self.e.emit(I::LdaSmi(token_value));
            self.e.emit(I::TestEqualStrict(token));
            self.e.jump(I::JumpIfFalse, next);
            match jump {
                Some(jump) => self.unwind(jump),
                None => {
                    self.e.emit(I::Ldar(value));
                    self.e.emit(I::ReThrow);
                }
            }
            self.e.bind(next);
        }
    }

    /// The try block plus its catch clause, if any.
    fn try_catch(&mut self, statement: &TryStatement) -> JsResult<()> {
        let Some(clause) = &statement.handler else {
            return self.block(&statement.block);
        };
        let context = self.save_context_for_handler();
        let start = self.e.bound_label();
        self.block(&statement.block)?;
        let end = self.e.bound_label();
        let after = self.e.new_label();
        self.e.jump(I::Jump, after);
        let handler = self.e.bound_label();
        self.e.add_handler(start, end, handler, context);
        self.catch_clause(clause)?;
        self.e.bind(after);
        Ok(())
    }

    /// Oxc moves the parameter bindings into the body block's scope, so the parameter is
    /// bound inside that scope, before the body's statements.
    fn catch_clause(&mut self, clause: &CatchClause) -> JsResult<()> {
        let mark = self.e.mark();
        let exception = self.e.alloc();
        self.e.emit(I::Star(exception));
        let saved = self.enter_scope(clause.body.scope_id());
        if let Some(param) = &clause.param {
            self.e.emit(I::Ldar(exception));
            self.bind_pattern(&param.pattern)?;
        }
        self.hoist_functions(&clause.body.body)?;
        for statement in &clause.body.body {
            self.statement(statement)?;
        }
        self.exit_scope(saved);
        self.e.release(mark);
        Ok(())
    }
}
