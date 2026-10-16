//! Instructions that complete in place (no jump, call or return).

use super::operators::{BinaryOp, UnaryOp};
use super::state::CodeInfo;
use super::{ErrorKind, Interpreter, JsResult};
use crate::bytecode::{ConstIndex, Constant, Instruction as I};
use crate::{JsStr, SymbolId, Value};

/// TDZ marker for uninitialized lexical bindings. JS code can never create this symbol.
pub(crate) const HOLE: Value = Value::Symbol(SymbolId(u32::MAX));

pub(crate) fn is_hole(value: &Value) -> bool {
    matches!(value, Value::Symbol(SymbolId(u32::MAX)))
}

impl Interpreter {
    pub(crate) fn constant_str(&self, info: &CodeInfo, index: ConstIndex) -> JsStr {
        match &info.code.constants[index.0 as usize] {
            Constant::String(s) => s.clone(),
            other => panic!("constant {index:?} is not a string: {other:?}"),
        }
    }

    fn constant_value(&mut self, info: &CodeInfo, index: ConstIndex) -> JsResult<Value> {
        match &info.code.constants[index.0 as usize] {
            Constant::Number(n) => Ok(Value::Number(*n)),
            Constant::String(s) => Ok(Value::String(s.clone())),
            Constant::BigInt(n) => Ok(Value::BigInt(n.clone())),
            _ => Err(self.internal("LdaConstant expects a primitive constant")),
        }
    }

    pub(crate) fn step_value(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<()> {
        let value = match instruction {
            I::LdaUndefined => Value::Undefined,
            I::LdaNull => Value::Null,
            I::LdaTrue => Value::Bool(true),
            I::LdaFalse => Value::Bool(false),
            I::LdaTheHole => HOLE,
            I::LdaSmi(n) => Value::Number(f64::from(*n)),
            I::LdaConstant(c) => self.constant_value(info, *c)?,
            I::Ldar(r) => self.reg(*r).clone(),
            I::LdaThis => self.this_value()?,
            I::LdaNewTarget => self.top().env.new_target.clone(),
            I::LdaCurrentClosure => self.top().function.clone(),
            I::Star(r) => {
                self.set_reg(*r, self.state.acc.clone());
                return Ok(());
            }
            I::Mov { from, to } => {
                self.set_reg(*to, self.reg(*from).clone());
                return Ok(());
            }
            I::ThrowIfHole(name) => {
                if is_hole(&self.state.acc) {
                    let name = self.constant_str(info, *name).to_string_lossy();
                    let message = format!("{name} is not initialized");
                    return Err(self.error(ErrorKind::Reference, &message));
                }
                return Ok(());
            }
            I::ThrowConstAssignment(name) => {
                let name = self.constant_str(info, *name).to_string_lossy();
                let message = format!("Assignment to constant variable '{name}'");
                return Err(self.error(ErrorKind::Type, &message));
            }
            I::CreateClosure(c) => self.create_closure(*c)?,
            other => return self.step_operator(info, other),
        };
        self.state.acc = value;
        Ok(())
    }

    fn step_operator(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<()> {
        let binary = |op, r: &crate::bytecode::Reg| Some((op, *r));
        let pair = match instruction {
            I::Add(r) => binary(BinaryOp::Add, r),
            I::Sub(r) => binary(BinaryOp::Sub, r),
            I::Mul(r) => binary(BinaryOp::Mul, r),
            I::Div(r) => binary(BinaryOp::Div, r),
            I::Mod(r) => binary(BinaryOp::Mod, r),
            I::Exp(r) => binary(BinaryOp::Exp, r),
            I::BitAnd(r) => binary(BinaryOp::BitAnd, r),
            I::BitOr(r) => binary(BinaryOp::BitOr, r),
            I::BitXor(r) => binary(BinaryOp::BitXor, r),
            I::Shl(r) => binary(BinaryOp::Shl, r),
            I::Sar(r) => binary(BinaryOp::Sar, r),
            I::Shr(r) => binary(BinaryOp::Shr, r),
            I::TestEqual(r) => binary(BinaryOp::Equal, r),
            I::TestNotEqual(r) => binary(BinaryOp::NotEqual, r),
            I::TestEqualStrict(r) => binary(BinaryOp::StrictEqual, r),
            I::TestNotEqualStrict(r) => binary(BinaryOp::StrictNotEqual, r),
            I::TestLessThan(r) => binary(BinaryOp::LessThan, r),
            I::TestGreaterThan(r) => binary(BinaryOp::GreaterThan, r),
            I::TestLessThanOrEqual(r) => binary(BinaryOp::LessThanOrEqual, r),
            I::TestGreaterThanOrEqual(r) => binary(BinaryOp::GreaterThanOrEqual, r),
            I::TestIn(r) => binary(BinaryOp::In, r),
            I::TestInstanceOf(r) => binary(BinaryOp::InstanceOf, r),
            _ => None,
        };
        if let Some((op, r)) = pair {
            let left = self.reg(r).clone();
            let right = self.state.acc.clone();
            self.state.acc = self.binary(op, &left, &right)?;
            return Ok(());
        }
        let unary = match instruction {
            I::Negate => UnaryOp::Negate,
            I::BitNot => UnaryOp::BitNot,
            I::LogicalNot => UnaryOp::LogicalNot,
            I::TypeOf => UnaryOp::TypeOf,
            I::ToNumber => UnaryOp::ToNumber,
            I::ToNumeric => UnaryOp::ToNumeric,
            I::Inc => UnaryOp::Inc,
            I::Dec => UnaryOp::Dec,
            I::ToString => UnaryOp::ToString,
            I::ToPropertyKey => UnaryOp::ToPropertyKey,
            I::CheckObjectCoercible => UnaryOp::CheckObjectCoercible,
            other => return self.step_environment(info, other),
        };
        let operand = self.state.acc.clone();
        self.state.acc = self.unary(unary, &operand)?;
        Ok(())
    }
}
