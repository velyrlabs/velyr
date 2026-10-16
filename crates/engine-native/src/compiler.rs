//! AST (Oxc) -> stack bytecode for the bootstrap subset.

use std::rc::Rc;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, AssignmentOperator, AssignmentTarget, BinaryOperator, BindingPattern, Expression,
    Statement, VariableDeclaration, VariableDeclarationKind,
};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use velyr_engine_api::JsException;
use velyr_engine_vm::JsStr;

pub type JsResult<T> = Result<T, JsException>;

/// Builds the exception `"<kind>: <message>"` that every engine failure uses.
pub fn throw(kind: &str, message: impl std::fmt::Display) -> JsException {
    JsException {
        message: format!("{kind}: {message}"),
        stack: None,
        value: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Let,
    Const,
}

/// Stack bytecode. Additions over the ticket list, all needed by the subset:
/// `PushUndefined` (`let a;`), `Dup` (assignment is an expression), and
/// `DeclareGlobal` (`let`/`const` create a binding, unlike `StoreGlobal`).
/// `Pop` is only emitted for ExpressionStatements and records the popped value
/// as the script completion value (eval semantics).
#[derive(Debug, Clone)]
pub enum Op {
    PushNum(f64),
    PushStr(JsStr),
    PushUndefined,
    LoadGlobal(Rc<str>),
    StoreGlobal(Rc<str>),
    DeclareGlobal(Rc<str>, DeclKind),
    Add,
    Sub,
    Mul,
    Div,
    Call(usize),
    Dup,
    Pop,
}

pub fn compile(text: &str) -> JsResult<Vec<Op>> {
    let allocator = Allocator::default();
    let options = ParseOptions {
        preserve_parens: false,
        ..ParseOptions::default()
    };
    let parsed = Parser::new(&allocator, text, SourceType::script())
        .with_options(options)
        .parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(throw("SyntaxError", &error.message));
    }
    let mut ops = Vec::new();
    for statement in &parsed.program.body {
        compile_statement(statement, &mut ops)?;
    }
    Ok(ops)
}

fn unsupported(node: &dyn std::fmt::Debug) -> JsException {
    // The variant name is the leading identifier of the derived `Debug` output.
    let debug = format!("{node:?}");
    let name: String = debug
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    throw(
        "SyntaxError",
        format!("unsupported in engine-native bootstrap: {name}"),
    )
}

fn compile_statement(statement: &Statement, ops: &mut Vec<Op>) -> JsResult<()> {
    match statement {
        Statement::ExpressionStatement(stmt) => {
            compile_expression(&stmt.expression, ops)?;
            ops.push(Op::Pop);
            Ok(())
        }
        Statement::VariableDeclaration(decl) => compile_declaration(decl, ops),
        other => Err(unsupported(other)),
    }
}

fn compile_declaration(decl: &VariableDeclaration, ops: &mut Vec<Op>) -> JsResult<()> {
    let kind = match decl.kind {
        VariableDeclarationKind::Let => DeclKind::Let,
        VariableDeclarationKind::Const => DeclKind::Const,
        _ => return Err(unsupported(&decl.kind)),
    };
    for declarator in &decl.declarations {
        let BindingPattern::BindingIdentifier(id) = &declarator.id else {
            return Err(unsupported(&declarator.id));
        };
        match &declarator.init {
            Some(init) => compile_expression(init, ops)?,
            None => ops.push(Op::PushUndefined),
        }
        ops.push(Op::DeclareGlobal(id.name.as_str().into(), kind));
    }
    Ok(())
}

fn compile_expression(expr: &Expression, ops: &mut Vec<Op>) -> JsResult<()> {
    match expr {
        Expression::NumericLiteral(n) => ops.push(Op::PushNum(n.value)),
        Expression::StringLiteral(s) => ops.push(Op::PushStr(JsStr::from_str(&s.value))),
        Expression::Identifier(id) => ops.push(Op::LoadGlobal(id.name.as_str().into())),
        Expression::BinaryExpression(bin) => {
            let op = match bin.operator {
                BinaryOperator::Addition => Op::Add,
                BinaryOperator::Subtraction => Op::Sub,
                BinaryOperator::Multiplication => Op::Mul,
                BinaryOperator::Division => Op::Div,
                _ => return Err(unsupported(&bin.operator)),
            };
            compile_expression(&bin.left, ops)?;
            compile_expression(&bin.right, ops)?;
            ops.push(op);
        }
        Expression::CallExpression(call) if !call.optional => {
            compile_expression(&call.callee, ops)?;
            for arg in &call.arguments {
                match arg {
                    Argument::SpreadElement(_) => return Err(unsupported(arg)),
                    _ => compile_expression(arg.to_expression(), ops)?,
                }
            }
            ops.push(Op::Call(call.arguments.len()));
        }
        Expression::AssignmentExpression(assign) => {
            let (AssignmentOperator::Assign, AssignmentTarget::AssignmentTargetIdentifier(id)) =
                (assign.operator, &assign.left)
            else {
                return Err(unsupported(expr));
            };
            compile_expression(&assign.right, ops)?;
            ops.push(Op::Dup);
            ops.push(Op::StoreGlobal(id.name.as_str().into()));
        }
        other => return Err(unsupported(other)),
    }
    Ok(())
}
