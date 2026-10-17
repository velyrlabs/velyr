//! Statements other than loops and try.

use oxc_ast::ast::{
    BlockStatement, IfStatement, LabeledStatement, Statement, VariableDeclaration,
    VariableDeclarationKind,
};
use oxc_span::GetSpan;
use velyr_engine_vm::bytecode::Instruction as I;

use super::control::{Breakable, BreakableKind, Control};
use super::function::{FunctionCompiler, binding_name};
use super::unsupported;
use crate::compiler::JsResult;

impl FunctionCompiler<'_> {
    pub fn statement(&mut self, statement: &Statement) -> JsResult<()> {
        let mark = self.e.mark();
        self.e.position(statement.span());
        let result = self.statement_inner(statement);
        self.e.release(mark);
        result
    }

    fn statement_inner(&mut self, statement: &Statement) -> JsResult<()> {
        match statement {
            Statement::BlockStatement(block) => self.block(block),
            Statement::BreakStatement(s) => {
                self.break_statement(s.label.as_ref().map(|l| l.name.as_str()))
            }
            Statement::ContinueStatement(s) => {
                self.continue_statement(s.label.as_ref().map(|l| l.name.as_str()))
            }
            Statement::DebuggerStatement(_) | Statement::EmptyStatement(_) => Ok(()),
            Statement::ExpressionStatement(s) => {
                self.expression(&s.expression)?;
                if let Some(completion) = self.completion {
                    self.e.emit(I::Star(completion));
                }
                Ok(())
            }
            Statement::IfStatement(s) => self.if_statement(s),
            Statement::LabeledStatement(s) => self.labeled(s),
            Statement::ReturnStatement(s) => {
                match &s.argument {
                    Some(argument) => {
                        self.expression(argument)?;
                        self.await_in_async_generator()?;
                    }
                    None => self.e.emit(I::LdaUndefined),
                }
                self.return_value();
                Ok(())
            }
            Statement::ThrowStatement(s) => {
                self.expression(&s.argument)?;
                self.e.emit_at(s.span, I::Throw);
                Ok(())
            }
            Statement::TryStatement(s) => self.try_statement(s),
            Statement::VariableDeclaration(declaration) => self.variable_declaration(declaration),
            // Hoisted by `hoist_functions` on scope entry; only Annex B.3.2 has work left.
            Statement::FunctionDeclaration(function) => match &function.id {
                Some(id) => self.copy_block_function(id.symbol_id()),
                None => Ok(()),
            },
            Statement::ClassDeclaration(class) => self.class(class, ""),
            Statement::WithStatement(_) => Err(unsupported("with statement")),
            Statement::DoWhileStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
            | Statement::ForStatement(_)
            | Statement::SwitchStatement(_)
            | Statement::WhileStatement(_) => self.breakable_statement(statement, Vec::new()),
            _ => Err(unsupported("module or TypeScript syntax")),
        }
    }

    fn breakable_statement(&mut self, statement: &Statement, labels: Vec<String>) -> JsResult<()> {
        match statement {
            Statement::DoWhileStatement(s) => self.do_while(s, labels),
            Statement::ForInStatement(s) => self.for_in(s, labels),
            Statement::ForOfStatement(s) => self.for_of(s, labels),
            Statement::ForStatement(s) => self.for_statement(s, labels),
            Statement::SwitchStatement(s) => self.switch(s, labels),
            Statement::WhileStatement(s) => self.while_loop(s, labels),
            other => self.labeled_block(other, labels),
        }
    }

    /// Script code: statements that produce `undefined` when their body is empty.
    pub fn reset_completion(&mut self) {
        if let Some(completion) = self.completion {
            self.e.emit(I::LdaUndefined);
            self.e.emit(I::Star(completion));
        }
    }

    pub fn block(&mut self, block: &BlockStatement) -> JsResult<()> {
        let mark = self.e.mark();
        let saved = self.enter_scope(block.scope_id());
        self.hoist_functions(&block.body)?;
        for statement in &block.body {
            self.statement(statement)?;
        }
        self.exit_scope(saved);
        self.e.release(mark);
        Ok(())
    }

    fn if_statement(&mut self, statement: &IfStatement) -> JsResult<()> {
        self.reset_completion();
        self.expression(&statement.test)?;
        let otherwise = self.e.new_label();
        self.e.jump(I::JumpIfFalse, otherwise);
        self.if_clause(&statement.consequent)?;
        match &statement.alternate {
            Some(alternate) => {
                let end = self.e.new_label();
                self.e.jump(I::Jump, end);
                self.e.bind(otherwise);
                self.if_clause(alternate)?;
                self.e.bind(end);
            }
            None => self.e.bind(otherwise),
        }
        Ok(())
    }

    /// B.3.3: a function declaration clause behaves as if wrapped in a block (Oxc gives it no
    /// scope of its own), so it is created when the clause runs.
    fn if_clause(&mut self, clause: &Statement) -> JsResult<()> {
        if matches!(clause, Statement::FunctionDeclaration(_)) {
            self.hoist_functions(std::slice::from_ref(clause))?;
        }
        self.statement(clause)
    }

    fn labeled(&mut self, statement: &LabeledStatement) -> JsResult<()> {
        let mut labels = vec![statement.label.name.to_string()];
        let mut body = &statement.body;
        while let Statement::LabeledStatement(inner) = body {
            labels.push(inner.label.name.to_string());
            body = &inner.body;
        }
        if matches!(body, Statement::FunctionDeclaration(_)) {
            // B.3.1: hoisted with the enclosing statement list.
            return self.statement(body);
        }
        self.breakable_statement(body, labels)
    }

    fn labeled_block(&mut self, body: &Statement, labels: Vec<String>) -> JsResult<()> {
        let end = self.e.new_label();
        self.control.push(Control::Breakable(Breakable {
            labels,
            kind: BreakableKind::Block,
            break_label: end,
            continue_label: None,
            iterator: None,
        }));
        let result = self.statement(body);
        self.control.pop();
        self.e.bind(end);
        result
    }

    pub fn variable_declaration(&mut self, declaration: &VariableDeclaration) -> JsResult<()> {
        if matches!(
            declaration.kind,
            VariableDeclarationKind::Using | VariableDeclarationKind::AwaitUsing
        ) {
            return Err(unsupported("using declaration"));
        }
        for declarator in &declaration.declarations {
            match &declarator.init {
                Some(init) => self.expression_named(init, binding_name(&declarator.id))?,
                None if declaration.kind == VariableDeclarationKind::Var => continue,
                None => self.e.emit(I::LdaUndefined),
            }
            self.bind_pattern(&declarator.id)?;
        }
        Ok(())
    }
}
