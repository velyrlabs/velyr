//! Lazy function bodies (VELYR-201): a nested function compiles to a stub (its header) and
//! its body compiles on the first call. Parsing and early errors stay at load; the first
//! call parses just the function's text, wrapped as a parenthesized expression, and finds
//! what it names from around it in the outer chain the stub kept (`outer.rs`).

use std::rc::Rc;

use oxc_ast::ast::{ArrowFunctionBody, ArrowFunctionExpression, Expression, Function, Statement};
use oxc_span::Span;
use velyr_engine_vm::bytecode::{CoroutineKind, FunctionCode, FunctionKind, LazyBody};

use super::emitter::{Emitter, Header};
use super::function::{Body, FunctionCompiler, FunctionSource};
use super::generators::coroutine_kind;
use super::outer::OuterScope;
use super::scope::EvalHome;
use super::unsupported;
use crate::compiler::{JsResult, throw};

/// What a stub needs to compile its body: its text and the contexts around it.
struct LazyFunction {
    /// Script text of the function: the whole expression for function expressions and
    /// arrows; from the parameters on for declarations and methods (their wrapper is an
    /// anonymous `function`, so a declaration's name keeps naming the outer binding).
    start: u32,
    end: u32,
    /// The text from `start` is a whole function or arrow expression.
    is_expression: bool,
    outer: Option<Rc<OuterScope>>,
    module: bool,
    coroutine: Option<CoroutineKind>,
    home: Option<EvalHome>,
}

impl FunctionCompiler<'_> {
    /// A header-only `FunctionCode` whose body compiles on its first call.
    pub(super) fn lazy_stub(
        &self,
        source: &FunctionSource,
        name: &str,
        kind: FunctionKind,
        is_expression: bool,
    ) -> FunctionCode {
        let items = source.params.map_or(&[][..], |params| &params.items[..]);
        let length = items.iter().take_while(|p| p.initializer.is_none()).count();
        let mut e = Emitter::default();
        e.shift = self.e.shift;
        let start = match (is_expression, source.params) {
            (false, Some(params)) => params.span.start,
            _ => source.span.start,
        };
        let text = self.e.shifted(Span::new(start, source.end));
        let mut stub = e.finish(Header {
            name: self.e.intern(name),
            kind,
            is_strict: self.analysis.is_strict(source.scope),
            parameter_count: items.len() as u32,
            length: length as u32,
            span: source.span,
            script: Some(self.script.clone()),
        });
        stub.lazy = Some(Rc::new(LazyFunction {
            start: text.start,
            end: text.end,
            is_expression,
            outer: self.outer_chain(),
            module: self.analysis.module,
            coroutine: source.coroutine,
            home: self.stub_home(source.scope),
        }));
        stub
    }
}

impl<'b, 'a> FunctionSource<'b, 'a> {
    /// `span` is what `toString` reports when it is not the function's own.
    pub fn function(function: &'b Function<'a>, span: Option<Span>) -> JsResult<Self> {
        let Some(body) = &function.body else {
            return Err(unsupported("function without body"));
        };
        Ok(Self {
            scope: function.scope_id(),
            params: Some(&function.params),
            body: Body::Statements(&body.statements),
            span: span.unwrap_or(function.span),
            end: function.span.end,
            self_name: function.id.as_ref().filter(|_| function.is_expression()),
            coroutine: coroutine_kind(function.generator, function.r#async),
        })
    }

    pub fn arrow(arrow: &'b ArrowFunctionExpression<'a>) -> Self {
        let body = match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => Body::Statements(&body.statements),
            expression => Body::Expression(expression.to_expression()),
        };
        Self {
            scope: arrow.scope_id(),
            params: Some(&arrow.params),
            body,
            span: arrow.span,
            end: arrow.span.end,
            self_name: None,
            coroutine: coroutine_kind(false, arrow.r#async),
        }
    }
}

impl LazyBody for LazyFunction {
    fn compile(&self, stub: &FunctionCode) -> Result<FunctionCode, String> {
        self.compile_body(stub).map_err(|error| {
            let message = error.message.strip_prefix("SyntaxError: ");
            message.map_or(error.message.clone(), str::to_owned)
        })
    }

    fn coroutine(&self) -> Option<CoroutineKind> {
        self.coroutine
    }
}

impl LazyFunction {
    fn compile_body(&self, stub: &FunctionCode) -> JsResult<FunctionCode> {
        let script = stub.script.clone().ok_or_else(|| internal("no script"))?;
        let mut text = String::new();
        if stub.is_strict {
            text.push_str("'use strict';");
        }
        text.push('(');
        if !self.is_expression {
            text.push_str(match self.coroutine {
                None => "function",
                Some(CoroutineKind::Generator) => "function*",
                Some(CoroutineKind::Async) => "async function",
                Some(CoroutineKind::AsyncGenerator) => "async function*",
            });
        }
        let shift = self.start.wrapping_sub(text.len() as u32);
        text.push_str(script.slice(self.start, self.end));
        text.push(')');
        super::analyze(&text, self.module, false, |program, _, analysis| {
            let source = match program.body.first() {
                Some(Statement::ExpressionStatement(statement)) => match &statement.expression {
                    Expression::FunctionExpression(function) => {
                        FunctionSource::function(function, None)?
                    }
                    Expression::ArrowFunctionExpression(arrow) => FunctionSource::arrow(arrow),
                    _ => return Err(internal("not a function")),
                },
                _ => return Err(internal("not a function")),
            };
            if source.end.wrapping_add(shift) != self.end {
                return Err(internal("moved"));
            }
            let mut compiler =
                FunctionCompiler::new(analysis, source.scope, stub.kind, script.clone());
            compiler.outer = self.outer.clone();
            compiler.own_home = self.home;
            compiler.e.shift = shift;
            let mut code = compiler.compile_function(&source, "")?;
            code.name = stub.name.clone();
            (code.source_start, code.source_end) = (stub.source_start, stub.source_end);
            Ok(code)
        })
    }
}

fn internal(what: &str) -> velyr_engine_api::JsException {
    throw(
        "SyntaxError",
        format!("internal compiler error: lazy body ({what})"),
    )
}
