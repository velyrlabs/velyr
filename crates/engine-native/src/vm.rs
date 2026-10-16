//! Stack VM for the bootstrap bytecode over `velyr-engine-vm` values.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, JsException, JsString, ResolverId, Value,
};
use velyr_engine_gc::GcRef;
use velyr_engine_vm::Value as Val;
use velyr_runtime_errors::{ErrorCode, VelyrError};

use crate::compiler::{DeclKind, JsResult, Op, throw};
use crate::number::{add, to_num};
use crate::value_bridge::{export, import};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutability {
    /// Host functions: writable like a `var`/global property.
    Writable,
    Let,
    Const,
}

struct Binding {
    value: Val,
    kind: Mutability,
}

pub type Slots = HashMap<TypeId, Box<dyn Any>>;

/// Per-isolate state that outlives a single script.
pub struct State {
    globals: HashMap<Rc<str>, Binding>,
    pub hosts: Vec<HostFunctionDef>,
    pub slots: Slots,
}

impl State {
    pub fn new() -> Self {
        let mut globals = HashMap::new();
        for (name, value) in [
            ("undefined", Val::Undefined),
            ("NaN", Val::Number(f64::NAN)),
            ("Infinity", Val::Number(f64::INFINITY)),
        ] {
            globals.insert(
                name.into(),
                Binding {
                    value,
                    kind: Mutability::Const,
                },
            );
        }
        Self {
            globals,
            hosts: Vec::new(),
            slots: Slots::new(),
        }
    }

    pub fn register_host(&mut self, def: HostFunctionDef) -> Result<(), VelyrError> {
        if self.globals.contains_key(def.name) {
            return Err(VelyrError::already_exists(format!(
                "host function {} is already registered",
                def.name
            )));
        }
        let index = self.hosts.len() as u32;
        self.hosts.push(def);
        self.globals.insert(
            def.name.into(),
            Binding {
                value: Val::Object(GcRef::new(index, 0)),
                kind: Mutability::Writable,
            },
        );
        Ok(())
    }

    /// Runs `ops` and returns the completion value.
    pub fn run(&mut self, ops: &[Op], terminated: &AtomicBool) -> JsResult<Val> {
        let mut stack: Vec<Val> = Vec::new();
        let mut completion = Val::Undefined;
        for op in ops {
            if terminated.load(Ordering::Relaxed) {
                return Err(throw("Error", "execution terminated"));
            }
            self.step(op, &mut stack, &mut completion)?;
        }
        Ok(completion)
    }

    fn step(&mut self, op: &Op, stack: &mut Vec<Val>, completion: &mut Val) -> JsResult<()> {
        match op {
            Op::PushNum(n) => stack.push(Val::Number(*n)),
            Op::PushStr(s) => stack.push(Val::String(s.clone())),
            Op::PushUndefined => stack.push(Val::Undefined),
            Op::LoadGlobal(name) => {
                let binding = self.globals.get(name);
                let binding = binding
                    .ok_or_else(|| throw("ReferenceError", format!("{name} is not defined")))?;
                stack.push(binding.value.clone());
            }
            Op::StoreGlobal(name) => {
                let value = pop(stack)?;
                match self.globals.get_mut(name) {
                    None => return Err(throw("ReferenceError", format!("{name} is not defined"))),
                    Some(b) if b.kind == Mutability::Const => {
                        return Err(throw("TypeError", "Assignment to constant variable."));
                    }
                    Some(b) => b.value = value,
                }
            }
            Op::DeclareGlobal(name, kind) => {
                let value = pop(stack)?;
                if let Some(existing) = self.globals.get(name)
                    && existing.kind != Mutability::Writable
                {
                    return Err(throw(
                        "SyntaxError",
                        format!("Identifier '{name}' has already been declared"),
                    ));
                }
                let kind = match kind {
                    DeclKind::Let => Mutability::Let,
                    DeclKind::Const => Mutability::Const,
                };
                self.globals.insert(name.clone(), Binding { value, kind });
            }
            Op::Add => {
                let (a, b) = pop2(stack)?;
                stack.push(add(&a, &b)?);
            }
            Op::Sub => binary_number(stack, |a, b| a - b)?,
            Op::Mul => binary_number(stack, |a, b| a * b)?,
            Op::Div => binary_number(stack, |a, b| a / b)?,
            Op::Call(argc) => {
                let args = stack.split_off(stack.len().saturating_sub(*argc));
                let callee = pop(stack)?;
                stack.push(self.call(&callee, &args)?);
            }
            Op::Dup => {
                let top = stack.last().cloned().ok_or_else(underflow)?;
                stack.push(top);
            }
            Op::Pop => *completion = pop(stack)?,
        }
        Ok(())
    }

    pub fn call(&mut self, callee: &Val, args: &[Val]) -> JsResult<Val> {
        let host = match callee {
            Val::Object(r) if r.generation() == 0 => self.hosts.get(r.index() as usize),
            _ => None,
        };
        let Some(host) = host else {
            return Err(throw("TypeError", "value is not a function"));
        };
        let callback = host.callback;
        let args = args.iter().map(export).collect::<JsResult<Vec<Value>>>()?;
        let result = callback(&mut Ctx(&mut self.slots), &args).map_err(host_exception)?;
        self.import_value(&result)
    }

    pub fn import_value(&self, value: &Value) -> JsResult<Val> {
        let imported = import(value);
        let known = match &imported {
            Val::Object(r) => r.generation() == 0 && (r.index() as usize) < self.hosts.len(),
            Val::BigInt(_) => false,
            _ => true,
        };
        if !known {
            return Err(throw(
                "TypeError",
                "value is not supported by engine-native bootstrap",
            ));
        }
        Ok(imported)
    }
}

fn underflow() -> JsException {
    throw(
        "Error",
        format!("{}: bytecode stack underflow", ErrorCode::Internal),
    )
}

fn pop(stack: &mut Vec<Val>) -> JsResult<Val> {
    stack.pop().ok_or_else(underflow)
}

fn pop2(stack: &mut Vec<Val>) -> JsResult<(Val, Val)> {
    let b = pop(stack)?;
    Ok((pop(stack)?, b))
}

fn binary_number(stack: &mut Vec<Val>, f: fn(f64, f64) -> f64) -> JsResult<()> {
    let (a, b) = pop2(stack)?;
    let (a, b) = (to_num(&a)?, to_num(&b)?);
    stack.push(Val::Number(f(a, b)));
    Ok(())
}

pub fn host_exception(err: HostError) -> JsException {
    match err {
        HostError::TypeError(message) => throw("TypeError", message),
        HostError::Thrown(value) => JsException {
            message: "Uncaught exception".into(),
            stack: None,
            value: Some(value),
        },
        HostError::Velyr(err) => throw("Error", format!("{}: {}", err.code(), err.message())),
    }
}

fn unsupported_host(what: &str) -> HostError {
    HostError::Velyr(VelyrError::unsupported(format!(
        "{what} is not supported by engine-native bootstrap"
    )))
}

/// `CallContext` for the bootstrap subset: strings, slots and `TypeError`s.
/// Objects, arrays, buffers and promises need the M10 object model; the
/// infallible constructors return `undefined`, the fallible ones `VELYR_UNSUPPORTED`.
struct Ctx<'a>(&'a mut Slots);

impl CallContext for Ctx<'_> {
    fn create_string(&mut self, value: JsString) -> Value {
        Value::String(value)
    }
    fn create_object(&mut self) -> Value {
        Value::Undefined
    }
    fn create_array(&mut self, _items: &[Value]) -> Value {
        Value::Undefined
    }
    fn create_array_buffer(&mut self, _bytes: &[u8]) -> Value {
        Value::Undefined
    }
    fn get_property(&mut self, _object: &Value, _key: &str) -> Result<Value, HostError> {
        Err(unsupported_host("property access"))
    }
    fn set_property(
        &mut self,
        _object: &Value,
        _key: &str,
        _value: Value,
    ) -> Result<(), HostError> {
        Err(unsupported_host("property access"))
    }
    fn create_promise(&mut self) -> (Value, ResolverId) {
        (Value::Undefined, ResolverId(0))
    }
    fn resolve(&mut self, _resolver: ResolverId, _value: Value) {}
    fn reject(&mut self, _resolver: ResolverId, _reason: Value) {}
    fn throw_type_error(&mut self, message: &str) -> HostError {
        HostError::TypeError(message.into())
    }
    fn slot(&mut self, type_id: TypeId) -> Option<&mut dyn Any> {
        self.0.get_mut(&type_id).map(|slot| &mut **slot)
    }
}
