//! Object, array and template literals.

use oxc_ast::ast::{
    ArrayExpression, ArrayExpressionElement, Expression, Function, ObjectExpression,
    ObjectProperty, ObjectPropertyKind, PropertyKey, PropertyKind, TemplateLiteral,
};
use oxc_span::Span;
use velyr_engine_vm::bytecode::{FunctionKind, Instruction as I, Reg, RegRange};
use velyr_engine_vm::number_to_string;

use super::function::FunctionCompiler;
use super::unsupported;
use crate::compiler::JsResult;

const PROTO: &str = "__proto__";

impl FunctionCompiler<'_> {
    pub fn object_literal(&mut self, object: &ObjectExpression) -> JsResult<()> {
        self.e.emit(I::CreateObject);
        let target = self.e.alloc();
        self.e.emit(I::Star(target));
        for property in &object.properties {
            let mark = self.e.mark();
            match property {
                ObjectPropertyKind::SpreadProperty(spread) => {
                    self.expression(&spread.argument)?;
                    let excluded = RegRange {
                        first: target,
                        count: 0,
                    };
                    self.e
                        .emit_at(spread.span, I::CopyDataProperties { target, excluded });
                }
                ObjectPropertyKind::ObjectProperty(property) => {
                    self.object_property(target, property)?;
                }
            }
            self.e.release(mark);
        }
        self.e.emit(I::Ldar(target));
        Ok(())
    }

    fn object_property(&mut self, object: Reg, property: &ObjectProperty) -> JsResult<()> {
        let name = static_key(&property.key, property.computed)?;
        if property.kind != PropertyKind::Init {
            let key = match &name {
                Some(name) => {
                    self.e.load_string(name);
                    let key = self.e.alloc();
                    self.e.emit(I::Star(key));
                    key
                }
                None => self.computed_key(&property.key)?,
            };
            let kind = if property.kind == PropertyKind::Get {
                FunctionKind::Getter
            } else {
                FunctionKind::Setter
            };
            let Expression::FunctionExpression(function) = &property.value else {
                return Err(unsupported("accessor without function"));
            };
            self.home_method(function, "", kind, object, property.span)?;
            let instruction = match kind {
                FunctionKind::Getter => I::DefineGetter { object, key },
                _ => I::DefineSetter { object, key },
            };
            self.e.emit(instruction);
            return Ok(());
        }
        let is_proto_setter =
            name.as_deref() == Some(PROTO) && !property.shorthand && !property.method;
        if is_proto_setter {
            self.expression(&property.value)?;
            self.e.emit(I::SetLiteralPrototype { object });
            return Ok(());
        }
        match name {
            Some(name) => {
                self.property_value(object, property, Some(&name))?;
                let name = self.e.string(&name);
                self.e.emit(I::DefineNamedOwn { object, name });
            }
            None => {
                let key = self.computed_key(&property.key)?;
                self.property_value(object, property, None)?;
                let set_function_name = is_anonymous_function(&property.value);
                self.e.emit(I::DefineKeyedOwn {
                    object,
                    key,
                    set_function_name,
                });
            }
        }
        Ok(())
    }

    fn property_value(
        &mut self,
        object: Reg,
        property: &ObjectProperty,
        name: Option<&str>,
    ) -> JsResult<()> {
        match &property.value {
            Expression::FunctionExpression(function) if property.method => {
                let kind = FunctionKind::Method;
                self.home_method(function, name.unwrap_or(""), kind, object, property.span)
            }
            value => self.expression_named(value, name),
        }
    }

    /// A method closure whose [[HomeObject]] is `object` when it uses `super`.
    fn home_method(
        &mut self,
        function: &Function,
        name: &str,
        kind: FunctionKind,
        object: Reg,
        source: Span,
    ) -> JsResult<()> {
        if self.method_closure(function, name, kind, Some(source))? {
            self.e.emit(I::SetHomeObject(object));
        }
        Ok(())
    }

    /// Evaluates a computed key, applies ToPropertyKey and keeps it in a register.
    pub fn computed_key(&mut self, key: &PropertyKey) -> JsResult<Reg> {
        let expression = key
            .as_expression()
            .ok_or_else(|| unsupported("private name"))?;
        self.expression(expression)?;
        self.e.emit(I::ToPropertyKey);
        let register = self.e.alloc();
        self.e.emit(I::Star(register));
        Ok(register)
    }

    pub fn array_literal(&mut self, array: &ArrayExpression) -> JsResult<()> {
        self.e.emit(I::CreateArray);
        let target = self.e.alloc();
        self.e.emit(I::Star(target));
        for element in &array.elements {
            match element {
                ArrayExpressionElement::Elision(_) => {
                    self.e.emit(I::ArrayPushHole { array: target })
                }
                ArrayExpressionElement::SpreadElement(spread) => {
                    self.expression(&spread.argument)?;
                    self.e
                        .emit_at(spread.span, I::SpreadIntoArray { array: target });
                }
                other => {
                    self.expression(other.to_expression())?;
                    self.e.emit(I::ArrayPush { array: target });
                }
            }
        }
        self.e.emit(I::Ldar(target));
        Ok(())
    }

    /// Untagged template: string concatenation with ToString on each substitution.
    pub fn template(&mut self, template: &TemplateLiteral) -> JsResult<()> {
        let cooked = |index: usize| -> JsResult<&str> {
            template.quasis[index]
                .value
                .cooked
                .as_deref()
                .ok_or_else(|| unsupported("invalid escape in template"))
        };
        let lone = |index: usize| template.quasis[index].lone_surrogates;
        if template.expressions.is_empty() {
            self.e.load_literal(cooked(0)?, lone(0));
            return Ok(());
        }
        let result = self.e.alloc();
        let mut has_part = false;
        let mut append = |compiler: &mut Self| {
            if has_part {
                compiler.e.emit(I::Add(result));
            }
            compiler.e.emit(I::Star(result));
            has_part = true;
        };
        for (index, expression) in template.expressions.iter().enumerate() {
            let quasi = cooked(index)?;
            if !quasi.is_empty() {
                self.e.load_literal(quasi, lone(index));
                append(self);
            }
            self.expression(expression)?;
            self.e.emit(I::ToString);
            append(self);
        }
        let tail = cooked(template.expressions.len())?;
        if !tail.is_empty() {
            self.e.load_literal(tail, lone(template.expressions.len()));
            append(self);
        }
        self.e.emit(I::Ldar(result));
        Ok(())
    }
}

/// The property name of a non-computed key.
pub fn static_key(key: &PropertyKey, computed: bool) -> JsResult<Option<String>> {
    if computed {
        return Ok(None);
    }
    Ok(Some(match key {
        PropertyKey::StaticIdentifier(id) => id.name.to_string(),
        PropertyKey::StringLiteral(s) => s.value.to_string(),
        PropertyKey::NumericLiteral(n) => number_to_string(n.value),
        PropertyKey::BigIntLiteral(b) => b.value.to_string(),
        PropertyKey::PrivateIdentifier(_) => return Err(unsupported("private name")),
        _ => return Ok(None),
    }))
}

pub(super) fn is_anonymous_function(value: &Expression) -> bool {
    match value {
        Expression::FunctionExpression(function) => function.id.is_none(),
        Expression::ArrowFunctionExpression(_) => true,
        Expression::ClassExpression(class) => class.id.is_none(),
        _ => false,
    }
}
