//! Assignment, compound/logical assignment, update and `delete`.

use oxc_ast::ast::{
    AssignmentExpression, AssignmentOperator, AssignmentTarget, Expression, IdentifierReference,
    SimpleAssignmentTarget, UpdateExpression, UpdateOperator,
};
use velyr_engine_vm::bytecode::{ConstIndex, Instruction as I, Reg};

use super::expression::{binary_instruction, short_circuit};
use super::function::FunctionCompiler;
use super::unsupported;
use crate::compiler::JsResult;

/// An evaluated reference: object and key are already in registers.
pub enum Place<'b, 'a> {
    Binding(&'b IdentifierReference<'a>),
    Named {
        object: Reg,
        name: ConstIndex,
    },
    Keyed {
        object: Reg,
        key: Reg,
    },
    /// `super[key]`, with `this` and the home object resolved at each access.
    Super {
        key: Reg,
    },
}

impl FunctionCompiler<'_> {
    /// Evaluates the object and key of a member target. `convert_key` applies
    /// ToPropertyKey once, for targets that are both read and written.
    pub fn prepare_place<'b, 'a>(
        &mut self,
        target: &'b SimpleAssignmentTarget<'a>,
        convert_key: bool,
    ) -> JsResult<Place<'b, 'a>> {
        if let Some(member) = target.as_member_expression()
            && member.object().is_super()
        {
            self.super_key(member)?;
            let key = self.e.alloc();
            self.e.emit(I::Star(key));
            return Ok(Place::Super { key });
        }
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => Ok(Place::Binding(id)),
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                let object = self.object_register(&member.object)?;
                let name = self.e.string(&member.property.name);
                Ok(Place::Named { object, name })
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                let object = self.object_register(&member.object)?;
                self.expression(&member.expression)?;
                if convert_key {
                    // RequireObjectCoercible(base) precedes ToPropertyKey(key).
                    let raw = self.e.alloc();
                    self.e.emit(I::Star(raw));
                    self.e.emit(I::Ldar(object));
                    self.e.emit(I::CheckObjectCoercible);
                    self.e.emit(I::Ldar(raw));
                    self.e.emit(I::ToPropertyKey);
                }
                let key = self.e.alloc();
                self.e.emit(I::Star(key));
                Ok(Place::Keyed { object, key })
            }
            SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                let object = self.object_register(&member.object)?;
                self.load_private_name(&member.field.name)?;
                let key = self.e.alloc();
                self.e.emit(I::Star(key));
                Ok(Place::Keyed { object, key })
            }
            _ => Err(unsupported("TypeScript syntax")),
        }
    }

    fn object_register(&mut self, object: &Expression) -> JsResult<Reg> {
        self.expression(object)?;
        let register = self.e.alloc();
        self.e.emit(I::Star(register));
        Ok(register)
    }

    pub fn load_place(&mut self, place: &Place) -> JsResult<()> {
        match *place {
            Place::Binding(id) => return self.load_identifier(id),
            Place::Named { object, name } => self.e.emit(I::GetNamed {
                object,
                name,
                cache: 0,
            }),
            Place::Keyed { object, key } => {
                self.e.emit(I::Ldar(key));
                self.e.emit(I::GetKeyed { object });
            }
            Place::Super { key } => {
                self.e.emit(I::Ldar(key));
                self.e.emit(I::GetSuper);
            }
        }
        Ok(())
    }

    /// Stores `acc` (preserved) into the place.
    pub fn store_place(&mut self, place: &Place) -> JsResult<()> {
        match *place {
            Place::Binding(id) => return self.store_identifier(id),
            Place::Named { object, name } => self.e.emit(I::SetNamed {
                object,
                name,
                cache: 0,
            }),
            Place::Keyed { object, key } => self.e.emit(I::SetKeyed { object, key }),
            Place::Super { key } => self.e.emit(I::SetSuper(key)),
        }
        Ok(())
    }

    pub fn assignment(&mut self, assignment: &AssignmentExpression) -> JsResult<()> {
        let operator = assignment.operator;
        let Some(target) = assignment.left.as_simple_assignment_target() else {
            if operator != AssignmentOperator::Assign {
                return Err(unsupported("compound assignment to a pattern"));
            }
            let pattern = assignment.left.to_assignment_target_pattern();
            self.expression(&assignment.right)?;
            let value = self.e.alloc();
            self.e.emit(I::Star(value));
            self.assign_pattern(pattern, value)?;
            self.e.emit(I::Ldar(value));
            return Ok(());
        };
        let name = simple_target_name(target);
        if operator == AssignmentOperator::Assign {
            let place = self.prepare_place(target, false)?;
            self.expression_named(&assignment.right, name)?;
            return self.store_place(&place);
        }
        let place = self.prepare_place(target, true)?;
        self.load_place(&place)?;
        if let Some(logical) = operator.to_logical_operator() {
            let end = self.e.new_label();
            self.e.jump(short_circuit(logical), end);
            self.expression_named(&assignment.right, name)?;
            self.store_place(&place)?;
            self.e.bind(end);
            return Ok(());
        }
        let binary = operator
            .to_binary_operator()
            .ok_or_else(|| unsupported("assignment operator"))?;
        let left = self.e.alloc();
        self.e.emit(I::Star(left));
        self.expression(&assignment.right)?;
        self.e
            .emit_at(assignment.span, binary_instruction(binary, left));
        self.store_place(&place)
    }

    pub fn update(&mut self, update: &UpdateExpression) -> JsResult<()> {
        let step = match update.operator {
            UpdateOperator::Increment => I::Inc,
            UpdateOperator::Decrement => I::Dec,
        };
        let place = self.prepare_place(&update.argument, true)?;
        self.load_place(&place)?;
        if update.prefix {
            self.e.emit_at(update.span, step);
            return self.store_place(&place);
        }
        self.e.emit_at(update.span, I::ToNumeric);
        let old = self.e.alloc();
        self.e.emit(I::Star(old));
        self.e.emit(step);
        self.store_place(&place)?;
        self.e.emit(I::Ldar(old));
        Ok(())
    }

    pub fn delete(&mut self, argument: &Expression) -> JsResult<()> {
        match argument {
            Expression::Identifier(id) => {
                let is_unresolved = self.analysis.symbol_of(id.reference_id()).is_none()
                    && self.outer_binding(&id.name).is_none();
                if is_unresolved && !self.is_strict {
                    let index = self.e.string(&id.name);
                    self.e.emit(I::DeleteGlobal(index));
                } else {
                    self.e.emit(I::LdaFalse);
                }
            }
            Expression::StaticMemberExpression(member) => {
                let object = self.object_register(&member.object)?;
                self.e.load_string(&member.property.name);
                self.e.emit_at(member.span, I::DeleteProperty { object });
            }
            Expression::ComputedMemberExpression(member) => {
                let object = self.object_register(&member.object)?;
                self.expression(&member.expression)?;
                self.e.emit_at(member.span, I::DeleteProperty { object });
            }
            Expression::ChainExpression(_) => return Err(unsupported("delete of optional chain")),
            other => {
                self.expression(other)?;
                self.e.emit(I::LdaTrue);
            }
        }
        Ok(())
    }

    /// PutValue of the value in `value` on a loop-head target.
    pub fn assign_from_register(&mut self, target: &AssignmentTarget, value: Reg) -> JsResult<()> {
        let place = self.prepare_target(target)?;
        self.e.emit(I::Ldar(value));
        self.finish_target(&place)
    }
}

pub fn simple_target_name<'b>(target: &'b SimpleAssignmentTarget) -> Option<&'b str> {
    match target {
        SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => Some(id.name.as_str()),
        _ => None,
    }
}
