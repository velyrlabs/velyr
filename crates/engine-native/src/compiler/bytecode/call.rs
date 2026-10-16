//! Property reads, calls, `new`, optional chains and tagged templates.

use std::rc::Rc;

use oxc_ast::ast::{
    Argument, CallExpression, ChainElement, ChainExpression, Expression, MemberExpression,
    NewExpression, TaggedTemplateExpression,
};
use oxc_span::{GetSpan, Span};
use velyr_engine_vm::JsStr;
use velyr_engine_vm::bytecode::{Constant, Instruction as I, Reg, RegRange, TemplateObject};

use super::emitter::Label;
use super::function::FunctionCompiler;
use super::unsupported;
use crate::compiler::JsResult;

impl FunctionCompiler<'_> {
    pub fn member(&mut self, member: &MemberExpression) -> JsResult<()> {
        if member.object().is_super() {
            self.super_key(member)?;
            self.e.emit_at(member.span(), I::GetSuper);
            return Ok(());
        }
        match member {
            MemberExpression::StaticMemberExpression(m) => {
                let object = self.member_object(&m.object, m.optional)?;
                let name = self.e.string(&m.property.name);
                let cache = 0;
                self.e.emit_at(
                    m.span,
                    I::GetNamed {
                        object,
                        name,
                        cache,
                    },
                );
            }
            MemberExpression::ComputedMemberExpression(m) => {
                let object = self.member_object(&m.object, m.optional)?;
                self.expression(&m.expression)?;
                self.e.emit_at(m.span, I::GetKeyed { object });
            }
            MemberExpression::PrivateFieldExpression(m) => {
                let object = self.member_object(&m.object, m.optional)?;
                self.load_private_name(&m.field.name)?;
                self.e.emit_at(m.span, I::GetKeyed { object });
            }
        }
        Ok(())
    }

    /// The key of `super.x`/`super[x]` in `acc`.
    pub fn super_key(&mut self, member: &MemberExpression) -> JsResult<()> {
        match member {
            MemberExpression::StaticMemberExpression(m) => self.e.load_string(&m.property.name),
            MemberExpression::ComputedMemberExpression(m) => {
                self.expression(&m.expression)?;
                self.e.emit(I::ToPropertyKey);
            }
            MemberExpression::PrivateFieldExpression(_) => return Err(unsupported("super.#x")),
        }
        Ok(())
    }

    /// Evaluates a member's object into a register, short-circuiting an optional link.
    fn member_object(&mut self, object: &Expression, optional: bool) -> JsResult<Reg> {
        self.expression(object)?;
        if optional {
            let nullish = self.chain_label()?;
            self.e.jump(I::JumpIfNullish, nullish);
        }
        let register = self.e.alloc();
        self.e.emit(I::Star(register));
        Ok(register)
    }

    fn chain_label(&self) -> JsResult<Label> {
        self.optional_chain
            .ok_or_else(|| unsupported("optional link outside a chain"))
    }

    pub fn chain(&mut self, chain: &ChainExpression) -> JsResult<()> {
        let nullish = self.e.new_label();
        let outer = self.optional_chain.replace(nullish);
        let result = match &chain.expression {
            ChainElement::CallExpression(call) => self.call(call),
            ChainElement::TSNonNullExpression(_) => Err(unsupported("TypeScript syntax")),
            element => self.member(element.to_member_expression()),
        };
        self.optional_chain = outer;
        result?;
        let end = self.e.new_label();
        self.e.jump(I::Jump, end);
        self.e.bind(nullish);
        self.e.emit(I::LdaUndefined);
        self.e.bind(end);
        Ok(())
    }

    /// The callee in a register, plus the receiver for method calls.
    fn callee(&mut self, callee: &Expression) -> JsResult<(Reg, Option<Reg>)> {
        let receiver = match callee {
            member
                if member
                    .as_member_expression()
                    .is_some_and(|m| m.object().is_super()) =>
            {
                let this = self.e.alloc();
                self.e.emit(I::LdaThis);
                self.e.emit(I::Star(this));
                self.member(member.to_member_expression())?;
                Some(this)
            }
            Expression::StaticMemberExpression(m) => {
                let object = self.member_object(&m.object, m.optional)?;
                let name = self.e.string(&m.property.name);
                let cache = 0;
                self.e.emit_at(
                    m.span,
                    I::GetNamed {
                        object,
                        name,
                        cache,
                    },
                );
                Some(object)
            }
            Expression::ComputedMemberExpression(m) => {
                let object = self.member_object(&m.object, m.optional)?;
                self.expression(&m.expression)?;
                self.e.emit_at(m.span, I::GetKeyed { object });
                Some(object)
            }
            Expression::PrivateFieldExpression(m) => {
                let object = self.member_object(&m.object, m.optional)?;
                self.load_private_name(&m.field.name)?;
                self.e.emit_at(m.span, I::GetKeyed { object });
                Some(object)
            }
            other => {
                self.expression(other)?;
                None
            }
        };
        let register = self.e.alloc();
        self.e.emit(I::Star(register));
        Ok((register, receiver))
    }

    pub fn call(&mut self, call: &CallExpression) -> JsResult<()> {
        if call.callee.is_super() {
            let args = self.spread_arguments(&call.arguments)?;
            self.e.emit_at(call.span, I::SuperCall(args));
            return Ok(());
        }
        let (callee, receiver) = self.callee(&call.callee)?;
        if call.optional {
            let nullish = self.chain_label()?;
            self.e.jump(I::JumpIfNullish, nullish);
        }
        let position = call_position(&call.callee, call.span);
        if call.arguments.iter().any(is_spread) {
            let args = self.spread_arguments(&call.arguments)?;
            let this = match receiver {
                Some(this) => this,
                None => {
                    let this = self.e.alloc();
                    self.e.emit(I::LdaUndefined);
                    self.e.emit(I::Star(this));
                    this
                }
            };
            self.e
                .emit_at(position, I::CallSpread { callee, this, args });
            return Ok(());
        }
        let args = self.arguments_in_registers(&call.arguments)?;
        self.emit_call(position, callee, receiver, args);
        Ok(())
    }

    fn emit_call(&mut self, span: Span, callee: Reg, receiver: Option<Reg>, args: RegRange) {
        let instruction = match receiver {
            Some(this) => I::Call { callee, this, args },
            None => I::CallUndefinedReceiver { callee, args },
        };
        self.e.emit_at(span, instruction);
    }

    pub fn new_expression(&mut self, new: &NewExpression) -> JsResult<()> {
        self.expression(&new.callee)?;
        let callee = self.e.alloc();
        self.e.emit(I::Star(callee));
        let instruction = if new.arguments.iter().any(is_spread) {
            let args = self.spread_arguments(&new.arguments)?;
            I::ConstructSpread { callee, args }
        } else {
            let args = self.arguments_in_registers(&new.arguments)?;
            I::Construct { callee, args }
        };
        self.e.emit_at(new.span, instruction);
        Ok(())
    }

    fn arguments_in_registers(&mut self, arguments: &[Argument]) -> JsResult<RegRange> {
        let range = self.e.alloc_range(arguments.len() as u32);
        for (index, argument) in arguments.iter().enumerate() {
            self.expression(argument.to_expression())?;
            self.e.emit(I::Star(Reg(range.first.0 + index as u32)));
        }
        Ok(range)
    }

    /// Builds the argument list as an array when any argument is spread.
    fn spread_arguments(&mut self, arguments: &[Argument]) -> JsResult<Reg> {
        self.e.emit(I::CreateArray);
        let array = self.e.alloc();
        self.e.emit(I::Star(array));
        for argument in arguments {
            match argument {
                Argument::SpreadElement(spread) => {
                    self.expression(&spread.argument)?;
                    self.e.emit_at(spread.span, I::SpreadIntoArray { array });
                }
                other => {
                    self.expression(other.to_expression())?;
                    self.e.emit(I::ArrayPush { array });
                }
            }
        }
        Ok(array)
    }

    pub fn tagged_template(&mut self, tagged: &TaggedTemplateExpression) -> JsResult<()> {
        let (callee, receiver) = self.callee(&tagged.tag)?;
        let quasis = &tagged.quasi.quasis;
        let template = TemplateObject {
            cooked: quasis
                .iter()
                .map(|q| {
                    q.value
                        .cooked
                        .as_ref()
                        .map(|c| super::emitter::literal_str(c, q.lone_surrogates))
                })
                .collect(),
            raw: quasis
                .iter()
                .map(|q| JsStr::from_str(&q.value.raw))
                .collect(),
        };
        let index = self.e.constant(Constant::TemplateObject(Rc::new(template)));
        let expressions = &tagged.quasi.expressions;
        let args = self.e.alloc_range(1 + expressions.len() as u32);
        self.e.emit(I::GetTemplateObject(index));
        self.e.emit(I::Star(args.first));
        for (offset, expression) in expressions.iter().enumerate() {
            self.expression(expression)?;
            self.e.emit(I::Star(Reg(args.first.0 + 1 + offset as u32)));
        }
        self.emit_call(tagged.span, callee, receiver, args);
        Ok(())
    }
}

/// V8 places a method call at its property name (stack traces, error positions).
fn call_position(callee: &Expression, call: Span) -> Span {
    match callee {
        Expression::StaticMemberExpression(member) => member.property.span,
        _ => call,
    }
}

fn is_spread(argument: &Argument) -> bool {
    matches!(argument, Argument::SpreadElement(_))
}
