//! Loops and switch.

use oxc_ast::ast::{
    DoWhileStatement, ForInStatement, ForOfStatement, ForStatement, ForStatementInit,
    ForStatementLeft, Statement, SwitchStatement, VariableDeclarationKind, WhileStatement,
};
use oxc_semantic::ScopeId;
use velyr_engine_vm::bytecode::{Instruction as I, Reg};

use super::control::{Breakable, BreakableKind, Control};
use super::emitter::Label;
use super::function::{FunctionCompiler, binding_name};
use crate::compiler::JsResult;

impl FunctionCompiler<'_> {
    pub fn while_loop(&mut self, statement: &WhileStatement, labels: Vec<String>) -> JsResult<()> {
        self.reset_completion();
        let head = self.e.bound_label();
        let end = self.e.new_label();
        self.expression(&statement.test)?;
        self.e.jump(I::JumpIfFalse, end);
        self.loop_body(&statement.body, labels, end, head)?;
        self.e.jump(I::Jump, head);
        self.e.bind(end);
        Ok(())
    }

    pub fn do_while(&mut self, statement: &DoWhileStatement, labels: Vec<String>) -> JsResult<()> {
        self.reset_completion();
        let head = self.e.bound_label();
        let next = self.e.new_label();
        let end = self.e.new_label();
        self.loop_body(&statement.body, labels, end, next)?;
        self.e.bind(next);
        self.expression(&statement.test)?;
        self.e.jump(I::JumpIfTrue, head);
        self.e.bind(end);
        Ok(())
    }

    pub fn for_statement(&mut self, statement: &ForStatement, labels: Vec<String>) -> JsResult<()> {
        self.reset_completion();
        let scope = statement.scope_id();
        let saved = self.enter_scope(scope);
        let mut per_iteration = false;
        match &statement.init {
            Some(ForStatementInit::VariableDeclaration(declaration)) => {
                per_iteration = declaration.kind != VariableDeclarationKind::Var;
                self.variable_declaration(declaration)?;
            }
            Some(init) => self.expression(init.to_expression())?,
            None => {}
        }
        let copy = saved.filter(|_| per_iteration);
        self.copy_context(scope, copy);
        let head = self.e.bound_label();
        let next = self.e.new_label();
        let end = self.e.new_label();
        if let Some(test) = &statement.test {
            self.expression(test)?;
            self.e.jump(I::JumpIfFalse, end);
        }
        self.loop_body(&statement.body, labels, end, next)?;
        self.e.bind(next);
        self.copy_context(scope, copy);
        if let Some(update) = &statement.update {
            self.expression(update)?;
        }
        self.e.jump(I::Jump, head);
        self.e.bind(end);
        self.exit_scope(saved);
        Ok(())
    }

    /// CreatePerIterationEnvironment: a fresh context holding copies of the loop bindings.
    fn copy_context(&mut self, scope: ScopeId, saved: Option<Reg>) {
        let Some(saved) = saved else { return };
        let size = self.analysis.context_size(scope);
        let mark = self.e.mark();
        let copies = self.e.alloc_range(size);
        for slot in 0..size {
            self.e.emit(I::LdaContextSlot { depth: 0, slot });
            self.e.emit(I::Star(Reg(copies.first.0 + slot)));
        }
        self.e.emit(I::PopContext(saved));
        self.e.emit(I::CreateContext(size));
        self.e.emit(I::PushContext(saved));
        for slot in 0..size {
            self.e.emit(I::Ldar(Reg(copies.first.0 + slot)));
            self.e.emit(I::StaContextSlot { depth: 0, slot });
        }
        self.e.release(mark);
    }

    fn loop_body(
        &mut self,
        body: &Statement,
        labels: Vec<String>,
        end: Label,
        next: Label,
    ) -> JsResult<()> {
        self.control.push(Control::Breakable(Breakable {
            labels,
            kind: BreakableKind::Loop,
            break_label: end,
            continue_label: Some(next),
            iterator: None,
        }));
        let result = self.statement(body);
        self.control.pop();
        result
    }

    /// Evaluates the right-hand side with the loop bindings in their TDZ.
    fn iteration_subject(
        &mut self,
        scope: ScopeId,
        right: &oxc_ast::ast::Expression,
    ) -> JsResult<()> {
        let saved = self.enter_scope(scope);
        self.expression(right)?;
        self.exit_scope(saved);
        Ok(())
    }

    pub fn for_in(&mut self, statement: &ForInStatement, labels: Vec<String>) -> JsResult<()> {
        if let ForStatementLeft::VariableDeclaration(declaration) = &statement.left
            && let Some(declarator) = declaration.declarations.first()
            && let Some(init) = &declarator.init
        {
            // Annex B.3.5: `for (var x = init in obj)` assigns before the head is evaluated.
            self.expression_named(init, binding_name(&declarator.id))?;
            self.bind_pattern(&declarator.id)?;
        }
        self.reset_completion();
        self.iteration_subject(statement.scope_id(), &statement.right)?;
        let enumerator = self.e.alloc();
        self.e.emit(I::ForInPrepare { enumerator });
        let head = self.e.bound_label();
        let end = self.e.new_label();
        self.e.jump(|done| I::ForInNext { enumerator, done }, end);
        let iteration = Iteration {
            labels,
            end,
            head,
            iterator: None,
        };
        self.iteration_body(
            statement.scope_id(),
            &statement.left,
            &statement.body,
            iteration,
        )?;
        self.e.jump(I::Jump, head);
        self.e.bind(end);
        Ok(())
    }

    pub fn for_of(&mut self, statement: &ForOfStatement, labels: Vec<String>) -> JsResult<()> {
        self.reset_completion();
        self.iteration_subject(statement.scope_id(), &statement.right)?;
        let record = self.e.alloc_range(3).first;
        self.open_loop_iterator(record, statement.r#await);
        let context = self.save_context_for_handler();
        let head = self.e.bound_label();
        let end = self.e.new_label();
        self.step_loop_iterator(record, end, statement.r#await)?;
        let iteration = Iteration {
            labels,
            end,
            head,
            iterator: Some((record, statement.r#await)),
        };
        self.iteration_body(
            statement.scope_id(),
            &statement.left,
            &statement.body,
            iteration,
        )?;
        self.e.jump(I::Jump, head);
        let region_end = self.e.bound_label();
        let handler = self.e.bound_label();
        self.e.add_handler(head, region_end, handler, context);
        let exception = self.e.alloc();
        self.e.emit(I::Star(exception));
        if statement.r#await {
            // AsyncIteratorClose with a throw completion: its errors give way to the original.
            let rethrow = self.e.new_label();
            let start = self.e.bound_label();
            self.e
                .jump(|done| I::AsyncIteratorClose { record, done }, rethrow);
            self.emit_await()?;
            let end = self.e.bound_label();
            self.e.bind(rethrow);
            self.e.add_handler(start, end, rethrow, context);
        } else {
            self.e.emit(I::IteratorCloseAfterThrow { record });
        }
        self.e.emit(I::Ldar(exception));
        self.e.emit(I::ReThrow);
        self.e.bind(end);
        Ok(())
    }

    /// Binds the value in `acc` to the loop head in a fresh per-iteration scope, then runs
    /// the body.
    fn iteration_body(
        &mut self,
        scope: ScopeId,
        left: &ForStatementLeft,
        body: &Statement,
        iteration: Iteration,
    ) -> JsResult<()> {
        let mark = self.e.mark();
        let value = self.e.alloc();
        self.e.emit(I::Star(value));
        self.control.push(Control::Breakable(Breakable {
            labels: iteration.labels,
            kind: BreakableKind::Loop,
            break_label: iteration.end,
            continue_label: Some(iteration.head),
            iterator: iteration.iterator,
        }));
        let saved = self.enter_scope(scope);
        let result = self
            .bind_loop_head(left, value)
            .and_then(|()| self.statement(body));
        self.exit_scope(saved);
        self.control.pop();
        self.e.release(mark);
        result
    }

    fn bind_loop_head(&mut self, left: &ForStatementLeft, value: Reg) -> JsResult<()> {
        match left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                self.e.emit(I::Ldar(value));
                self.bind_pattern(&declaration.declarations[0].id)
            }
            target => self.assign_from_register(target.to_assignment_target(), value),
        }
    }

    pub fn switch(&mut self, statement: &SwitchStatement, labels: Vec<String>) -> JsResult<()> {
        self.reset_completion();
        self.expression(&statement.discriminant)?;
        let discriminant = self.e.alloc();
        self.e.emit(I::Star(discriminant));
        let scope = statement.scope_id();
        let saved = self.enter_scope(scope);
        self.switch_scopes.insert(scope);
        for case in &statement.cases {
            self.hoist_functions(&case.consequent)?;
        }
        let case_labels: Vec<Label> = statement.cases.iter().map(|_| self.e.new_label()).collect();
        for (case, &label) in statement.cases.iter().zip(&case_labels) {
            if let Some(test) = &case.test {
                self.expression(test)?;
                self.e.emit(I::TestEqualStrict(discriminant));
                self.e.jump(I::JumpIfTrue, label);
            }
        }
        let end = self.e.new_label();
        let default = statement.cases.iter().position(|case| case.test.is_none());
        self.e
            .jump(I::Jump, default.map_or(end, |index| case_labels[index]));
        self.control.push(Control::Breakable(Breakable {
            labels,
            kind: BreakableKind::Switch,
            break_label: end,
            continue_label: None,
            iterator: None,
        }));
        let result = statement
            .cases
            .iter()
            .zip(&case_labels)
            .try_for_each(|(case, &label)| {
                self.e.bind(label);
                case.consequent.iter().try_for_each(|s| self.statement(s))
            });
        self.control.pop();
        result?;
        self.e.bind(end);
        self.exit_scope(saved);
        Ok(())
    }
}

struct Iteration {
    labels: Vec<String>,
    end: Label,
    head: Label,
    iterator: Option<(Reg, bool)>,
}
