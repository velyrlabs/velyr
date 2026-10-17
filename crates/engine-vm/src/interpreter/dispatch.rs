//! The dispatch loop: one `match` per instruction over the top frame, frame push/pop for
//! calls and returns, and exception unwinding through the handler tables.

use std::rc::Rc;

use super::function::CallOutcome;
use super::ops::is_hole;
use super::state::{CodeInfo, Frame};
use super::{Abrupt, ErrorKind, Interpreter, JsResult};
use crate::bytecode::{FunctionKind, Instruction as I, Reg, RegRange, Target};
use crate::{Value, to_boolean};

/// What `step` asks the loop to do next.
pub(crate) enum Flow {
    Next,
    /// The top frame changed (call pushed a frame).
    Switch,
    Return,
}

impl Interpreter {
    pub(crate) fn top(&self) -> &Frame {
        self.state.frames.last().expect("a frame is running")
    }

    pub(crate) fn top_mut(&mut self) -> &mut Frame {
        self.state.frames.last_mut().expect("a frame is running")
    }

    pub(crate) fn reg(&self, r: Reg) -> &Value {
        &self.state.registers[self.top().base + r.0 as usize]
    }

    pub(crate) fn set_reg(&mut self, r: Reg, value: Value) {
        let index = self.top().base + r.0 as usize;
        self.state.registers[index] = value;
    }

    pub(crate) fn reg_range(&self, range: RegRange) -> Vec<Value> {
        let first = self.top().base + range.first.0 as usize;
        self.state.registers[first..first + range.count as usize].to_vec()
    }

    /// Runs frames until the entry frame pushed by the caller returns.
    pub(crate) fn execute(&mut self) -> JsResult<Value> {
        loop {
            let info = self.top().info.clone();
            match self.run_frame(&info) {
                Ok(Flow::Switch) => {}
                Ok(Flow::Next) => unreachable!("run_frame only exits on a frame change"),
                Ok(Flow::Return) => match self.pop_frame() {
                    (true, result) => return result,
                    (false, Ok(value)) => self.state.acc = value,
                    // Thrown in the caller, like every [[Construct]] result check.
                    (false, Err(abrupt)) => self.unwind(abrupt)?,
                },
                Err(abrupt) => self.unwind(abrupt)?,
            }
        }
    }

    fn run_frame(&mut self, info: &Rc<CodeInfo>) -> JsResult<Flow> {
        let instructions = &info.code.instructions;
        loop {
            let frame = self.top_mut();
            let pc = frame.pc as usize;
            frame.pc += 1;
            match self.step(info, &instructions[pc])? {
                Flow::Next => {}
                flow => return Ok(flow),
            }
        }
    }

    /// Pops the returning frame: (whether it was the entry frame, its result).
    fn pop_frame(&mut self) -> (bool, JsResult<Value>) {
        let frame = self.state.frames.pop().expect("a frame is returning");
        self.state.registers.truncate(frame.base);
        let value = std::mem::replace(&mut self.state.acc, Value::Undefined);
        let result = match (frame.constructed, &frame.env.this_cell, value) {
            (_, _, value @ Value::Object(_)) => Ok(value),
            (Some(object), _, _) => Ok(object),
            (None, Some(cell), value)
                if frame.info.code.kind == FunctionKind::DerivedConstructor =>
            {
                let this = cell.borrow().clone();
                match value {
                    Value::Undefined if is_hole(&this) => Err(self.error(
                        ErrorKind::Reference,
                        "Must call super constructor in derived class before accessing 'this' \
                         or returning from derived constructor",
                    )),
                    Value::Undefined => Ok(this),
                    _ => Err(self.error(
                        ErrorKind::Type,
                        "Derived constructors may only return object or undefined",
                    )),
                }
            }
            (_, _, value) => Ok(value),
        };
        (frame.is_entry, result)
    }

    /// Transfers a throw to the innermost handler of the frames of this run; frames
    /// without one are popped. Termination and heap failures skip every handler.
    fn unwind(&mut self, abrupt: Abrupt) -> JsResult<()> {
        loop {
            if let Abrupt::Throw(value) = &abrupt
                && self.enter_handler(value)
            {
                return Ok(());
            }
            let frame = self.state.frames.pop().expect("a frame is unwinding");
            self.state.registers.truncate(frame.base);
            if frame.is_entry {
                return Err(abrupt);
            }
        }
    }

    fn enter_handler(&mut self, value: &Value) -> bool {
        let frame = self.top();
        let offset = frame.pc - 1;
        let handler = frame
            .info
            .code
            .handlers
            .iter()
            .find(|h| h.start.0 <= offset && offset < h.end.0)
            .copied();
        let Some(handler) = handler else {
            return false;
        };
        if let Some(register) = handler.context {
            let context = self.reg(register).clone();
            self.top_mut().context = context;
        }
        self.state.acc = value.clone();
        self.top_mut().pc = handler.target.0;
        true
    }

    fn jump(&mut self, target: Target) -> JsResult<Flow> {
        let frame = self.top_mut();
        let is_back_edge = target.0 < frame.pc;
        frame.pc = target.0;
        if is_back_edge {
            self.check_termination()?;
            self.enforce_heap_limit()?;
        }
        Ok(Flow::Next)
    }

    fn jump_if(&mut self, condition: bool, target: Target) -> JsResult<Flow> {
        if condition {
            return self.jump(target);
        }
        Ok(Flow::Next)
    }

    fn call_outcome(outcome: CallOutcome) -> Flow {
        match outcome {
            CallOutcome::Pushed => Flow::Switch,
            CallOutcome::Done => Flow::Next,
        }
    }

    fn step(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<Flow> {
        let acc = &self.state.acc;
        match instruction {
            I::Jump(t) => return self.jump(*t),
            I::JumpIfTrue(t) => return self.jump_if(to_boolean(acc), *t),
            I::JumpIfFalse(t) => return self.jump_if(!to_boolean(acc), *t),
            I::JumpIfNullish(t) => return self.jump_if(is_nullish(acc), *t),
            I::JumpIfNotNullish(t) => return self.jump_if(!is_nullish(acc), *t),
            I::JumpIfUndefined(t) => return self.jump_if(matches!(acc, Value::Undefined), *t),
            I::JumpIfNotUndefined(t) => {
                return self.jump_if(!matches!(acc, Value::Undefined), *t);
            }
            I::Return if info.coroutine.is_some() => return self.coroutine_return(info),
            I::Return => return Ok(Flow::Return),
            I::CreateGenerator { .. }
            | I::Suspend { .. }
            | I::Resume { .. }
            | I::AsyncFunctionReject
            | I::GetAsyncIterator { .. }
            | I::IteratorNext { .. }
            | I::IteratorUnpack { .. }
            | I::YieldStarCall { .. }
            | I::YieldStarUnpack { .. }
            | I::AsyncIteratorClose { .. }
            | I::CheckIteratorResult => return self.step_generator(info, instruction),
            I::LdaModuleImport { .. } | I::DynamicImport { .. } => self.step_module(instruction)?,
            I::CreateContextWithNames { .. }
            | I::PushWithContext(_)
            | I::LdaLookup(_)
            | I::LdaLookupForTypeof(_)
            | I::StaLookup { .. }
            | I::DeleteLookup(_)
            | I::LdaLookupCallee { .. }
            | I::ResolveLookup { .. }
            | I::LdaLookupIn { .. }
            | I::StaLookupIn { .. }
            | I::DeclareEvalVar { .. } => self.step_lookup(info, instruction)?,
            I::CreateDisposeStack(_) | I::AddDisposable { .. } | I::DisposeRecordError { .. } => {
                self.step_dispose(instruction)?;
            }
            I::DisposeNext { stack, done } => match self.dispose_next(*stack)? {
                Some(outcome) => return Ok(Self::call_outcome(outcome)),
                None => return self.jump(*done),
            },
            I::Throw | I::ReThrow => return Err(Abrupt::Throw(acc.clone())),
            I::Call { callee, this, args } => {
                let (f, this, args) = (
                    self.reg(*callee).clone(),
                    self.reg(*this).clone(),
                    self.reg_range(*args),
                );
                return self
                    .call_from_loop(&f, &this, &args)
                    .map(Self::call_outcome);
            }
            I::CallUndefinedReceiver { callee, args } => {
                let (f, args) = (self.reg(*callee).clone(), self.reg_range(*args));
                return self
                    .call_from_loop(&f, &Value::Undefined, &args)
                    .map(Self::call_outcome);
            }
            I::CallDirectEval { callee, args, site } => {
                let (f, args) = (self.reg(*callee).clone(), self.reg_range(*args));
                let eval = self.intrinsic(super::builtins::Intrinsic::Eval);
                if self.as_object(&f) != Some(eval) {
                    return self
                        .call_from_loop(&f, &Value::Undefined, &args)
                        .map(Self::call_outcome);
                }
                self.state.acc = self.direct_eval(info, *site, args.first())?;
            }
            I::CallSpread { callee, this, args } => {
                let (f, this) = (self.reg(*callee).clone(), self.reg(*this).clone());
                let args = self.array_elements(self.reg(*args).clone())?;
                return self
                    .call_from_loop(&f, &this, &args)
                    .map(Self::call_outcome);
            }
            I::Construct { callee, args } => {
                let (f, args) = (self.reg(*callee).clone(), self.reg_range(*args));
                return self.construct_from_loop(&f, &args).map(Self::call_outcome);
            }
            I::ConstructSpread { callee, args } => {
                let f = self.reg(*callee).clone();
                let args = self.array_elements(self.reg(*args).clone())?;
                return self.construct_from_loop(&f, &args).map(Self::call_outcome);
            }
            I::IteratorStep { record, done } => {
                if !self.iterator_step(*record)? {
                    return self.jump(*done);
                }
            }
            I::ForInNext { enumerator, done } => {
                if !self.for_in_next(*enumerator)? {
                    return self.jump(*done);
                }
            }
            other => self.step_value(info, other)?,
        }
        Ok(Flow::Next)
    }
}

pub(crate) fn is_nullish(value: &Value) -> bool {
    matches!(value, Value::Undefined | Value::Null)
}
