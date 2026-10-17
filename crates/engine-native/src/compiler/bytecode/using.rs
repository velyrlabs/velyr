//! `using` and `await using` (Explicit Resource Management): a statement list declaring
//! resources runs as `try { ... } finally { DisposeResources }`, with the completion
//! carried through the finally.

use oxc_ast::ast::{Statement, VariableDeclarationKind};
use velyr_engine_vm::bytecode::{Instruction as I, Reg};

use super::control::{Control, TOKEN_THROW};
use super::function::FunctionCompiler;
use super::unsupported;
use crate::compiler::JsResult;

impl FunctionCompiler<'_> {
    /// Compiles a statement list, disposing its `using` resources when it completes.
    pub fn statement_list(&mut self, statements: &[Statement]) -> JsResult<()> {
        let compile = |this: &mut Self| statements.iter().try_for_each(|s| this.statement(s));
        match using_kind(statements) {
            Some(is_async) => self.dispose_scope(is_async, compile),
            None => compile(self),
        }
    }

    /// Runs `body` with a fresh dispose stack, disposing it whatever the completion.
    pub fn dispose_scope(
        &mut self,
        is_async: bool,
        body: impl FnOnce(&mut Self) -> JsResult<()>,
    ) -> JsResult<()> {
        let stack = self.e.alloc();
        self.e.emit(I::CreateDisposeStack(stack));
        self.with_finally(
            |this| {
                this.control.push(Control::Disposal(stack));
                let result = body(this);
                this.control.pop();
                result
            },
            |this, token, value| this.dispose_resources(stack, is_async, token, value),
        )
    }

    /// AddDisposableResource of the value in `acc` (kept there) for a `using` binding.
    pub fn add_disposable(&mut self, kind: VariableDeclarationKind) -> JsResult<()> {
        if !kind.is_using() {
            return Ok(());
        }
        let stack = self.control.iter().rev().find_map(|entry| match entry {
            Control::Disposal(stack) => Some(*stack),
            _ => None,
        });
        let stack = stack.ok_or_else(|| unsupported("using declaration outside a block"))?;
        self.e.emit(I::AddDisposable {
            stack,
            is_async: kind.is_await(),
        });
        Ok(())
    }

    /// DisposeResources: each dispose error is recorded (wrapped in a SuppressedError over
    /// the earlier one) and replaces the completion once the stack is empty.
    fn dispose_resources(
        &mut self,
        stack: Reg,
        is_async: bool,
        token: Reg,
        value: Reg,
    ) -> JsResult<()> {
        let error = self.e.alloc();
        self.e.emit(I::LdaTheHole);
        self.e.emit(I::Star(error));
        let normal = self.e.new_label();
        self.e.emit(I::LdaSmi(TOKEN_THROW));
        self.e.emit(I::TestEqualStrict(token));
        self.e.jump(I::JumpIfFalse, normal);
        self.e.emit(I::Mov {
            from: value,
            to: error,
        });
        self.e.bind(normal);
        self.dispose_loop(stack, is_async, error)?;
        let no_error = self.e.new_label();
        self.e.emit(I::LdaTheHole);
        self.e.emit(I::TestEqualStrict(error));
        self.e.jump(I::JumpIfTrue, no_error);
        self.e.emit(I::Ldar(error));
        self.e.emit(I::ReThrow);
        self.e.bind(no_error);
        Ok(())
    }

    fn dispose_loop(&mut self, stack: Reg, is_async: bool, error: Reg) -> JsResult<()> {
        let context = self.save_context_for_handler();
        let head = self.e.bound_label();
        let done = self.e.new_label();
        self.e.jump(|done| I::DisposeNext { stack, done }, done);
        if is_async {
            self.emit_await()?;
        }
        let end = self.e.bound_label();
        self.e.jump(I::Jump, head);
        let handler = self.e.bound_label();
        self.e.add_handler(head, end, handler, context);
        self.e.emit(I::DisposeRecordError { error });
        self.e.jump(I::Jump, head);
        self.e.bind(done);
        Ok(())
    }
}

/// `Some(is_async)` when the list declares `using` resources, `is_async` for any `await using`.
pub(super) fn using_kind(statements: &[Statement]) -> Option<bool> {
    let kinds = statements.iter().filter_map(|statement| match statement {
        Statement::VariableDeclaration(d) if d.kind.is_using() => Some(d.kind.is_await()),
        _ => None,
    });
    kinds.reduce(|any_async, is_async| any_async || is_async)
}
