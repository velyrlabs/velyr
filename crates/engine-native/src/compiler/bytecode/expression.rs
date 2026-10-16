//! Expression dispatch, operators and simple literals.

use oxc_ast::ast::{
    BinaryExpression, BinaryOperator, ConditionalExpression, Expression, LogicalExpression,
    LogicalOperator, UnaryExpression, UnaryOperator,
};
use oxc_span::GetSpan;
use velyr_engine_vm::bytecode::{Constant, FunctionKind, Instruction as I, Reg};
use velyr_engine_vm::interpreter::validate_regexp;

use super::function::FunctionCompiler;
use super::unsupported;
use crate::compiler::{JsResult, throw};

impl FunctionCompiler<'_> {
    /// Compiles `expression` into `acc`, releasing its temporaries.
    pub fn expression(&mut self, expression: &Expression) -> JsResult<()> {
        let mark = self.e.mark();
        self.e.position(expression.span());
        let result = self.expression_inner(expression);
        self.e.release(mark);
        result
    }

    /// NamedEvaluation: an anonymous function gets `name` when it is known statically.
    pub fn expression_named(
        &mut self,
        expression: &Expression,
        name: Option<&str>,
    ) -> JsResult<()> {
        match (expression, name) {
            (Expression::FunctionExpression(function), Some(name)) if function.id.is_none() => {
                self.closure(function, name, FunctionKind::Normal)
            }
            (Expression::ArrowFunctionExpression(arrow), Some(name)) => self.arrow(arrow, name),
            (Expression::ClassExpression(class), Some(name)) if class.id.is_none() => {
                self.class(class, name)
            }
            _ => self.expression(expression),
        }
    }

    fn expression_inner(&mut self, expression: &Expression) -> JsResult<()> {
        match expression {
            Expression::BooleanLiteral(b) => {
                self.e.emit(if b.value { I::LdaTrue } else { I::LdaFalse })
            }
            Expression::NullLiteral(_) => self.e.emit(I::LdaNull),
            Expression::NumericLiteral(n) => self.e.load_number(n.value),
            Expression::StringLiteral(s) => self.e.load_literal(&s.value, s.lone_surrogates),
            Expression::BigIntLiteral(b) => {
                let constant = Constant::bigint_from_decimal(&b.value)
                    .ok_or_else(|| unsupported("bigint literal"))?;
                let index = self.e.constant(constant);
                self.e.emit(I::LdaConstant(index));
            }
            Expression::RegExpLiteral(r) => {
                let flags = r.regex.flags.to_string();
                // Early error: an invalid literal fails the whole script.
                validate_regexp(&r.regex.pattern.text, &flags)
                    .map_err(|message| throw("SyntaxError", message))?;
                let pattern = self.e.string(&r.regex.pattern.text);
                let flags = self.e.string(&flags);
                self.e.emit(I::CreateRegExp { pattern, flags });
            }
            Expression::TemplateLiteral(t) => self.template(t)?,
            Expression::Identifier(id) => self.load_identifier(id)?,
            Expression::ThisExpression(_) => self.e.emit(I::LdaThis),
            Expression::ArrayExpression(a) => self.array_literal(a)?,
            Expression::ObjectExpression(o) => self.object_literal(o)?,
            Expression::FunctionExpression(f) => self.closure(f, "", FunctionKind::Normal)?,
            Expression::ArrowFunctionExpression(a) => self.arrow(a, "")?,
            Expression::AssignmentExpression(a) => self.assignment(a)?,
            Expression::BinaryExpression(b) => self.binary(b)?,
            Expression::LogicalExpression(l) => self.logical(l)?,
            Expression::ConditionalExpression(c) => self.conditional(c)?,
            Expression::UnaryExpression(u) => self.unary(u)?,
            Expression::UpdateExpression(u) => self.update(u)?,
            Expression::SequenceExpression(s) => {
                for item in &s.expressions {
                    self.expression(item)?;
                }
            }
            Expression::ParenthesizedExpression(p) => self.expression_inner(&p.expression)?,
            Expression::CallExpression(c) => self.call(c)?,
            Expression::NewExpression(n) => self.new_expression(n)?,
            Expression::ChainExpression(c) => self.chain(c)?,
            Expression::TaggedTemplateExpression(t) => self.tagged_template(t)?,
            Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_) => {
                self.member(expression.to_member_expression())?
            }
            Expression::Super(_) => return Err(unsupported("super")),
            Expression::ClassExpression(c) => self.class(c, "")?,
            Expression::AwaitExpression(a) => self.await_expression(a)?,
            Expression::YieldExpression(y) => self.yield_expression(y)?,
            Expression::ImportExpression(i) => {
                self.expression(&i.source)?;
                let options = match &i.options {
                    Some(options) => {
                        let (specifier, options_reg) = (self.e.alloc(), self.e.alloc());
                        self.e.emit(I::Star(specifier));
                        self.expression(options)?;
                        self.e.emit(I::Star(options_reg));
                        self.e.emit(I::Ldar(specifier));
                        Some(options_reg)
                    }
                    None => None,
                };
                self.e.emit_at(i.span, I::DynamicImport { options });
            }
            Expression::ImportMeta(_) => self.import_meta()?,
            Expression::NewTarget(_) => self.e.emit(I::LdaNewTarget),
            Expression::PrivateInExpression(p) => {
                self.load_private_name(&p.left.name)?;
                let key = self.e.alloc();
                self.e.emit(I::Star(key));
                self.expression(&p.right)?;
                self.e.emit_at(p.span, I::TestIn(key));
            }
            _ => return Err(unsupported("JSX or TypeScript syntax")),
        }
        Ok(())
    }

    fn binary(&mut self, binary: &BinaryExpression) -> JsResult<()> {
        self.expression(&binary.left)?;
        let left = self.e.alloc();
        self.e.emit(I::Star(left));
        self.expression(&binary.right)?;
        self.e
            .emit_at(binary.span, binary_instruction(binary.operator, left));
        Ok(())
    }

    fn logical(&mut self, logical: &LogicalExpression) -> JsResult<()> {
        self.expression(&logical.left)?;
        let end = self.e.new_label();
        self.e.jump(short_circuit(logical.operator), end);
        self.expression(&logical.right)?;
        self.e.bind(end);
        Ok(())
    }

    fn conditional(&mut self, conditional: &ConditionalExpression) -> JsResult<()> {
        let otherwise = self.e.new_label();
        let end = self.e.new_label();
        self.expression(&conditional.test)?;
        self.e.jump(I::JumpIfFalse, otherwise);
        self.expression(&conditional.consequent)?;
        self.e.jump(I::Jump, end);
        self.e.bind(otherwise);
        self.expression(&conditional.alternate)?;
        self.e.bind(end);
        Ok(())
    }

    fn unary(&mut self, unary: &UnaryExpression) -> JsResult<()> {
        let operation = match unary.operator {
            UnaryOperator::Delete => return self.delete(&unary.argument),
            UnaryOperator::Typeof => {
                match &unary.argument {
                    Expression::Identifier(id) => self.load_identifier_for_typeof(id)?,
                    argument => self.expression(argument)?,
                }
                I::TypeOf
            }
            operator => {
                self.expression(&unary.argument)?;
                match operator {
                    UnaryOperator::UnaryPlus => I::ToNumber,
                    UnaryOperator::UnaryNegation => I::Negate,
                    UnaryOperator::LogicalNot => I::LogicalNot,
                    UnaryOperator::BitwiseNot => I::BitNot,
                    UnaryOperator::Void => I::LdaUndefined,
                    UnaryOperator::Delete | UnaryOperator::Typeof => {
                        unreachable!("handled by the outer match")
                    }
                }
            }
        };
        self.e.emit_at(unary.span, operation);
        Ok(())
    }
}

pub fn short_circuit(operator: LogicalOperator) -> fn(velyr_engine_vm::bytecode::Target) -> I {
    match operator {
        LogicalOperator::Or => I::JumpIfTrue,
        LogicalOperator::And => I::JumpIfFalse,
        LogicalOperator::Coalesce => I::JumpIfNotNullish,
    }
}

pub fn binary_instruction(operator: BinaryOperator, left: Reg) -> I {
    match operator {
        BinaryOperator::Addition => I::Add(left),
        BinaryOperator::Subtraction => I::Sub(left),
        BinaryOperator::Multiplication => I::Mul(left),
        BinaryOperator::Division => I::Div(left),
        BinaryOperator::Remainder => I::Mod(left),
        BinaryOperator::Exponential => I::Exp(left),
        BinaryOperator::BitwiseAnd => I::BitAnd(left),
        BinaryOperator::BitwiseOR => I::BitOr(left),
        BinaryOperator::BitwiseXOR => I::BitXor(left),
        BinaryOperator::ShiftLeft => I::Shl(left),
        BinaryOperator::ShiftRight => I::Sar(left),
        BinaryOperator::ShiftRightZeroFill => I::Shr(left),
        BinaryOperator::Equality => I::TestEqual(left),
        BinaryOperator::Inequality => I::TestNotEqual(left),
        BinaryOperator::StrictEquality => I::TestEqualStrict(left),
        BinaryOperator::StrictInequality => I::TestNotEqualStrict(left),
        BinaryOperator::LessThan => I::TestLessThan(left),
        BinaryOperator::GreaterThan => I::TestGreaterThan(left),
        BinaryOperator::LessEqualThan => I::TestLessThanOrEqual(left),
        BinaryOperator::GreaterEqualThan => I::TestGreaterThanOrEqual(left),
        BinaryOperator::In => I::TestIn(left),
        BinaryOperator::Instanceof => I::TestInstanceOf(left),
    }
}
