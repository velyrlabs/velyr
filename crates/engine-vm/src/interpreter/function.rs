//! Function objects, frames and calls. A closure call from the dispatch loop only pushes
//! a frame; a call that starts in Rust also runs the loop until that frame returns.

use std::cell::RefCell;
use std::rc::Rc;

use velyr_engine_gc::{Gc, Trace, Tracer};

use super::ops::HOLE;
use super::state::{CodeInfo, Frame, FunctionTable};
use super::{Abrupt, ErrorKind, Interpreter, JsResult, MAX_FRAMES, MAX_REENTRY, object_value};
use crate::bytecode::{ConstIndex, Constant, FunctionKind};
use crate::{InternalMethods, JsStr, ObjectRef, PropertyDescriptor, Value};

/// Native function body: `(interpreter, this, args) -> result`.
pub type NativeFn = Rc<dyn Fn(&mut Interpreter, &Value, &[Value]) -> Result<Value, Abrupt>>;

/// The `this`-related part of a function Environment Record. Arrows (and script code) keep
/// the one of the frame that created them; other functions get a fresh one per call.
#[derive(Clone)]
pub(crate) struct FunctionEnv {
    pub this: Value,
    /// Late-bound `this` of a derived constructor (set by `super()`), shared with arrows.
    pub this_cell: Option<Rc<RefCell<Value>>>,
    /// The non-arrow function: its [[HomeObject]] and, for `super()`, its prototype.
    pub function: Value,
    pub new_target: Value,
}

impl FunctionEnv {
    pub fn new(this: Value) -> Self {
        Self {
            this,
            this_cell: None,
            function: Value::Undefined,
            new_target: Value::Undefined,
        }
    }
}

impl Trace for FunctionEnv {
    fn trace(&self, tracer: &mut Tracer) {
        self.this.trace(tracer);
        if let Some(cell) = &self.this_cell {
            cell.borrow().trace(tracer);
        }
        self.function.trace(tracer);
        self.new_target.trace(tracer);
    }
}

/// Instance element recorded on a class constructor: a field (`ClassField`, with its
/// initializer closure or `undefined`) or a private method/accessor.
#[derive(Clone)]
pub(crate) struct Element {
    pub key: Value,
    pub kind: FunctionKind,
    pub value: Value,
}

#[derive(Clone)]
pub(crate) struct Closure {
    pub info: Rc<CodeInfo>,
    pub context: Value,
    /// Used by arrows and script code only.
    pub env: FunctionEnv,
    pub home: Value,
    /// Class constructors: [[PrivateMethods]] and [[Fields]].
    pub elements: Option<Rc<RefCell<Vec<Element>>>>,
}

pub(crate) struct BoundFunction {
    pub target: Value,
    pub this: Value,
    pub args: Vec<Value>,
}

#[derive(Clone)]
pub(crate) enum Callable {
    Closure(Rc<Closure>),
    Native { call: NativeFn, constructor: bool },
    Bound(Rc<BoundFunction>),
}

impl Trace for Callable {
    fn trace(&self, tracer: &mut Tracer) {
        match self {
            Callable::Closure(closure) => {
                closure.context.trace(tracer);
                closure.env.trace(tracer);
                closure.home.trace(tracer);
                for element in closure.elements.iter().flat_map(|e| e.borrow().clone()) {
                    element.key.trace(tracer);
                    element.value.trace(tracer);
                }
            }
            Callable::Native { .. } => {}
            Callable::Bound(bound) => {
                bound.target.trace(tracer);
                bound.this.trace(tracer);
                bound.args.trace(tracer);
            }
        }
    }
}

pub(crate) fn is_class_constructor(kind: FunctionKind) -> bool {
    matches!(
        kind,
        FunctionKind::ClassConstructor | FunctionKind::DerivedConstructor
    )
}

/// What the dispatch loop does after a call instruction.
pub(crate) enum CallOutcome {
    /// A frame was pushed; the loop continues in the callee.
    Pushed,
    /// The call completed (native function); the result is in `acc`.
    Done,
}

impl Interpreter {
    fn function_table(&self) -> Gc<FunctionTable> {
        self.state.functions
    }

    pub(crate) fn callable(&self, value: &Value) -> Option<Callable> {
        let Value::Object(r) = value else {
            return None;
        };
        let table = self.realm.heap.get(self.function_table())?;
        table.0.get(*r).cloned()
    }

    /// `CreateClosure`: a function object over the current context.
    pub(crate) fn create_closure(&mut self, index: ConstIndex) -> JsResult<Value> {
        let code = match &self.top().info.code.constants[index.0 as usize] {
            Constant::Function(code) => code.clone(),
            _ => return Err(self.internal("CreateClosure expects a function constant")),
        };
        let info = self.code_info(&code);
        let coroutine = info.coroutine.map(|(kind, _)| kind);
        let frame = self.top();
        let env = match code.kind {
            FunctionKind::Arrow => frame.env.clone(),
            _ => FunctionEnv::new(Value::Undefined),
        };
        let closure = Closure {
            info,
            context: frame.context.clone(),
            env,
            home: Value::Undefined,
            elements: None,
        };
        let is_constructor = code.kind == FunctionKind::Normal;
        let name = code.name.clone();
        let callable = Callable::Closure(Rc::new(closure));
        let function = self.create_function_object(callable, &name, code.length)?;
        if let Some(kind) = coroutine {
            self.init_coroutine_function(function, kind)?;
        }
        if is_constructor {
            self.add_prototype_property(function)?;
        }
        if is_constructor && !code.is_strict {
            // Implementation-defined `caller`/`arguments` of sloppy functions (as in V8), so
            // they do not reach the throwing accessors of `Function.prototype`.
            for key in ["caller", "arguments"] {
                let key = self.realm.key_str(key);
                let desc = PropertyDescriptor::data(Value::Null, false, false, false);
                let result = self.realm.define_own_property(function, key, &desc);
                result.map_err(|e| self.object_error(e))?;
            }
        }
        Ok(object_value(function))
    }

    pub(crate) fn create_native_function(
        &mut self,
        name: &str,
        arity: u32,
        call: NativeFn,
        constructor: bool,
    ) -> JsResult<ObjectRef> {
        let callable = Callable::Native { call, constructor };
        self.create_function_object(callable, &JsStr::from_str(name), arity)
    }

    pub(crate) fn create_function_object(
        &mut self,
        callable: Callable,
        name: &JsStr,
        length: u32,
    ) -> JsResult<ObjectRef> {
        let proto = Some(self.state.intrinsics.function_prototype);
        let function = self.alloc_object(proto)?;
        self.make_function(function, callable, name, length)?;
        Ok(function)
    }

    /// Turns the ordinary object `function` into a function with `length` and `name`.
    pub(crate) fn make_function(
        &mut self,
        function: ObjectRef,
        callable: Callable,
        name: &JsStr,
        length: u32,
    ) -> JsResult<()> {
        let table = self.function_table();
        let table = self
            .realm
            .heap
            .get_mut(table)
            .expect("function table is rooted");
        table.0.insert(function.raw(), callable);
        self.define_hidden(function, "length", Value::Number(f64::from(length)))?;
        self.set_function_name(function, Value::String(name.clone()))
    }

    /// Replaces the behavior behind a function object (e.g. to set its [[HomeObject]]).
    pub(crate) fn replace_callable(&mut self, function: ObjectRef, callable: Callable) {
        let table = self.function_table();
        let table = self.realm.heap.get_mut(table);
        table
            .expect("function table is rooted")
            .0
            .insert(function.raw(), callable);
    }

    /// SetFunctionName: `name` is a non-writable, configurable own property.
    pub(crate) fn set_function_name(&mut self, function: ObjectRef, name: Value) -> JsResult<()> {
        self.define_hidden(function, "name", name)
    }

    pub(crate) fn define_hidden(
        &mut self,
        object: ObjectRef,
        key: &str,
        value: Value,
    ) -> JsResult<()> {
        let key = self.realm.key_str(key);
        let desc = PropertyDescriptor::data(value, false, false, true);
        self.realm
            .define_own_property(object, key, &desc)
            .map_err(|e| self.object_error(e))?;
        Ok(())
    }

    /// `F.prototype = { constructor: F }` for constructors.
    fn add_prototype_property(&mut self, function: ObjectRef) -> JsResult<()> {
        self.state.scratch.push(object_value(function));
        let proto = self.alloc_object(Some(self.state.intrinsics.object_prototype));
        self.state.scratch.pop();
        let proto = proto?;
        let key = self.realm.key_str("constructor");
        let desc = PropertyDescriptor::data(object_value(function), true, false, true);
        self.realm
            .define_own_property(proto, key, &desc)
            .map_err(|e| self.object_error(e))?;
        let key = self.realm.key_str("prototype");
        let desc = PropertyDescriptor::data(object_value(proto), true, false, false);
        self.realm
            .define_own_property(function, key, &desc)
            .map_err(|e| self.object_error(e))?;
        Ok(())
    }

    /// `Call` from Rust: runs a closure to completion in a nested dispatch run.
    pub(crate) fn call_value(
        &mut self,
        f: &Value,
        this: &Value,
        args: &[Value],
    ) -> JsResult<Value> {
        match self.callable(f) {
            Some(Callable::Closure(closure)) => {
                self.check_callable(&closure)?;
                let this = self.sloppy_this(&closure, f, this, args)?;
                let env = self.frame_env(&closure, f, &this, Value::Undefined);
                self.run_closure(&closure, f, env, args, None)
            }
            Some(Callable::Native { call, .. }) => self.call_native(&call, this, args),
            Some(Callable::Bound(bound)) => {
                let args: Vec<Value> = bound.args.iter().chain(args).cloned().collect();
                self.call_value(&bound.target, &bound.this, &args)
            }
            None => Err(self.not_callable(f)),
        }
    }

    /// Pushes an entry frame for `closure` and runs it to completion.
    pub(crate) fn run_closure(
        &mut self,
        closure: &Closure,
        function: &Value,
        env: FunctionEnv,
        args: &[Value],
        constructed: Option<Value>,
    ) -> JsResult<Value> {
        self.enter_reentry()?;
        let result = self
            .push_frame(closure, function, env, args, constructed, true)
            .and_then(|()| self.execute());
        self.reentry -= 1;
        result
    }

    /// Class constructors throw when called without `new`.
    fn check_callable(&mut self, closure: &Closure) -> JsResult<()> {
        let code = &closure.info.code;
        if !is_class_constructor(code.kind) {
            return Ok(());
        }
        let message = format!(
            "Class constructor {} cannot be invoked without 'new'",
            code.name.to_string_lossy()
        );
        Err(self.error(ErrorKind::Type, &message))
    }

    pub(crate) fn call_native(
        &mut self,
        call: &NativeFn,
        this: &Value,
        args: &[Value],
    ) -> JsResult<Value> {
        self.call_native_with_target(call, this, args, Value::Undefined)
    }

    /// Runs a native function with `NewTarget` set (`Undefined` for a plain call). `this`,
    /// the arguments and whatever the native `keep`s stay rooted until it returns.
    pub(crate) fn call_native_with_target(
        &mut self,
        call: &NativeFn,
        this: &Value,
        args: &[Value],
        new_target: Value,
    ) -> JsResult<Value> {
        self.enter_reentry()?;
        let base = self.state.scratch.len();
        self.state.scratch.push(this.clone());
        self.state.scratch.extend(args.iter().cloned());
        let outer_target = std::mem::replace(&mut self.state.new_target, new_target);
        let result = call(self, this, args);
        self.state.new_target = outer_target;
        self.state.scratch.truncate(base);
        self.reentry -= 1;
        result
    }

    pub(crate) fn enter_reentry(&mut self) -> JsResult<()> {
        if self.reentry >= MAX_REENTRY {
            return Err(self.stack_overflow());
        }
        self.reentry += 1;
        Ok(())
    }

    /// `Call` from the dispatch loop: closures get a frame, natives run now.
    pub(crate) fn call_from_loop(
        &mut self,
        f: &Value,
        this: &Value,
        args: &[Value],
    ) -> JsResult<CallOutcome> {
        if let Some((f, this, args)) = self.redirect_call(f, this, args)? {
            return self.call_from_loop(&f, &this, &args);
        }
        match self.callable(f) {
            Some(Callable::Closure(closure)) => {
                self.check_callable(&closure)?;
                let this = self.sloppy_this(&closure, f, this, args)?;
                let env = self.frame_env(&closure, f, &this, Value::Undefined);
                self.push_frame(&closure, f, env, args, None, false)?;
                Ok(CallOutcome::Pushed)
            }
            Some(Callable::Native { call, .. }) => {
                self.state.acc = self.call_native(&call, this, args)?;
                Ok(CallOutcome::Done)
            }
            Some(Callable::Bound(bound)) => {
                let args: Vec<Value> = bound.args.iter().chain(args).cloned().collect();
                self.call_from_loop(&bound.target, &bound.this, &args)
            }
            None => Err(self.not_callable(f)),
        }
    }

    /// The environment of a new frame of `closure` called with `this` and `new_target`.
    pub(crate) fn frame_env(
        &self,
        closure: &Closure,
        function: &Value,
        this: &Value,
        new_target: Value,
    ) -> FunctionEnv {
        let code = &closure.info.code;
        let this = match code.kind {
            FunctionKind::Arrow | FunctionKind::Script => return closure.env.clone(),
            FunctionKind::DerivedConstructor => Value::Undefined,
            _ if code.is_strict => this.clone(),
            _ if matches!(this, Value::Undefined | Value::Null) => {
                object_value(self.state.intrinsics.global)
            }
            // Primitives were already boxed by `sloppy_this`.
            _ => this.clone(),
        };
        let is_derived = code.kind == FunctionKind::DerivedConstructor;
        FunctionEnv {
            this,
            this_cell: is_derived.then(|| Rc::new(RefCell::new(HOLE))),
            function: function.clone(),
            new_target,
        }
    }

    pub(crate) fn push_frame(
        &mut self,
        closure: &Closure,
        function: &Value,
        env: FunctionEnv,
        args: &[Value],
        constructed: Option<Value>,
        is_entry: bool,
    ) -> JsResult<()> {
        self.check_termination()?;
        if self.state.frames.len() >= MAX_FRAMES {
            return Err(self.stack_overflow());
        }
        let code = &closure.info.code;
        let base = self.state.registers.len();
        let parameters = code.parameter_count as usize;
        let registers = code.register_count as usize;
        self.state
            .registers
            .resize(base + registers, Value::Undefined);
        for (slot, arg) in self.state.registers[base..base + parameters.min(registers)]
            .iter_mut()
            .zip(args)
        {
            *slot = arg.clone();
        }
        let arguments = match closure.info.keeps_arguments {
            true => args.to_vec(),
            false => Vec::new(),
        };
        self.state.frames.push(Frame {
            info: closure.info.clone(),
            pc: 0,
            base,
            context: closure.context.clone(),
            env,
            function: function.clone(),
            arguments,
            constructed,
            is_entry,
        });
        Ok(())
    }

    pub(crate) fn stack_overflow(&mut self) -> Abrupt {
        self.error(ErrorKind::Range, "Maximum call stack size exceeded")
    }

    pub(crate) fn not_callable(&mut self, f: &Value) -> Abrupt {
        let what = self.describe_callee(f);
        self.error(ErrorKind::Type, &format!("{what} is not a function"))
    }

    pub(crate) fn describe_callee(&mut self, f: &Value) -> String {
        match f {
            Value::String(s) => format!("\"{}\"", s.to_string_lossy()),
            Value::Object(_) => "object".into(),
            other => self
                .coerce_string(other)
                .map(|s| s.to_string_lossy())
                .unwrap_or_else(|_| "value".into()),
        }
    }
}
