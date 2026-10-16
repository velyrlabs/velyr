//! `[[Construct]]` (ordinary functions, classes, natives, bound functions) and
//! `Function.prototype.call/apply/bind`. `call`/`apply` invoked from the dispatch loop are
//! redirected to their target there, so recursion through them does not nest Rust frames.

use std::rc::Rc;

use super::function::{BoundFunction, CallOutcome, Callable, Closure, is_class_constructor};
use super::{ErrorKind, Interpreter, JsResult, object_value};
use crate::bytecode::FunctionKind;
use crate::{InternalMethods, JsStr, ObjectRef, PropertyDescriptor, Value, strict_equals};

impl Interpreter {
    /// `Construct(F, args)` from the dispatch loop.
    pub(crate) fn construct_from_loop(
        &mut self,
        f: &Value,
        args: &[Value],
    ) -> JsResult<CallOutcome> {
        match self.construct(f, args, f, false)? {
            Some(result) => {
                self.state.acc = result;
                Ok(CallOutcome::Done)
            }
            None => Ok(CallOutcome::Pushed),
        }
    }

    /// `Construct(F, args, newTarget)` from Rust, run to completion.
    pub(crate) fn construct_value(
        &mut self,
        f: &Value,
        args: &[Value],
        new_target: &Value,
    ) -> JsResult<Value> {
        let result = self.construct(f, args, new_target, true)?;
        Ok(result.expect("nested constructs complete"))
    }

    /// `None` when a frame was pushed for the dispatch loop (`nested` is false).
    fn construct(
        &mut self,
        f: &Value,
        args: &[Value],
        new_target: &Value,
        nested: bool,
    ) -> JsResult<Option<Value>> {
        match self.callable(f) {
            Some(Callable::Closure(closure)) if is_constructor_kind(closure.info.code.kind) => {
                let constructed = match closure.info.code.kind {
                    FunctionKind::DerivedConstructor => None,
                    _ => Some(self.construct_this(f, &closure, new_target)?),
                };
                let this = constructed.clone().unwrap_or(Value::Undefined);
                let env = self.frame_env(&closure, f, &this, new_target.clone());
                if nested {
                    return self
                        .run_closure(&closure, f, env, args, constructed)
                        .map(Some);
                }
                self.push_frame(&closure, f, env, args, constructed, false)?;
                Ok(None)
            }
            Some(Callable::Native {
                call,
                constructor: true,
            }) => {
                let new_target = new_target.clone();
                let result =
                    self.call_native_with_target(&call, &Value::Undefined, args, new_target);
                Ok(Some(result?))
            }
            Some(Callable::Bound(bound)) if self.is_constructor(&bound.target) => {
                let new_target = match strict_equals(f, new_target) {
                    true => bound.target.clone(),
                    false => new_target.clone(),
                };
                let args: Vec<Value> = bound.args.iter().chain(args).cloned().collect();
                self.construct(&bound.target, &args, &new_target, nested)
            }
            _ => {
                let what = self.describe_callee(f);
                Err(self.error(ErrorKind::Type, &format!("{what} is not a constructor")))
            }
        }
    }

    /// OrdinaryCreateFromConstructor(newTarget), then the class's instance elements.
    fn construct_this(
        &mut self,
        f: &Value,
        closure: &Closure,
        new_target: &Value,
    ) -> JsResult<Value> {
        let proto = self.prototype_from_constructor(new_target)?;
        let this = object_value(self.alloc_object(Some(proto))?);
        if closure.elements.is_some() {
            self.state.scratch.push(this.clone());
            let result = self.initialize_instance_elements(&this, f);
            self.state.scratch.pop();
            result?;
        }
        Ok(this)
    }

    /// `constructor.prototype` when it is an object, else `Object.prototype`.
    pub(crate) fn prototype_from_constructor(
        &mut self,
        constructor: &Value,
    ) -> JsResult<ObjectRef> {
        let object = self
            .as_object(constructor)
            .expect("constructors are objects");
        let key = self.realm.key_str("prototype");
        let proto = self.get(object, key, constructor)?;
        match self.as_object(&proto) {
            Some(proto) => Ok(proto),
            None => {
                self.function_realm(object)?;
                Ok(self.state.intrinsics.object_prototype)
            }
        }
    }

    pub(crate) fn is_constructor(&self, value: &Value) -> bool {
        match self.callable(value) {
            Some(Callable::Closure(closure)) => is_constructor_kind(closure.info.code.kind),
            Some(Callable::Native { constructor, .. }) => constructor,
            Some(Callable::Bound(bound)) => self.is_constructor(&bound.target),
            None => false,
        }
    }

    /// `f.call(...)`/`f.apply(...)` as a direct call of `f`, for the dispatch loop.
    pub(crate) fn redirect_call(
        &mut self,
        f: &Value,
        this: &Value,
        args: &[Value],
    ) -> JsResult<Option<(Value, Value, Vec<Value>)>> {
        let Some(object) = self.as_object(f) else {
            return Ok(None);
        };
        let intrinsics = self.state.intrinsics;
        let this_arg = args.first().cloned().unwrap_or(Value::Undefined);
        if object == intrinsics.function_call && self.callable(this).is_some() {
            return Ok(Some((
                this.clone(),
                this_arg,
                args.iter().skip(1).cloned().collect(),
            )));
        }
        if object == intrinsics.function_apply && self.callable(this).is_some() {
            let list = self.arguments_list(args.get(1))?;
            return Ok(Some((this.clone(), this_arg, list)));
        }
        Ok(None)
    }

    /// CreateListFromArrayLike for `apply` (`null`/`undefined` mean no arguments).
    fn arguments_list(&mut self, array_like: Option<&Value>) -> JsResult<Vec<Value>> {
        match array_like {
            None | Some(Value::Undefined | Value::Null) => Ok(Vec::new()),
            Some(value) => self.list_from_array_like(value),
        }
    }

    /// BoundFunctionCreate plus the `name`/`length` of `Function.prototype.bind`.
    fn bind_function(&mut self, target: &Value, args: &[Value]) -> JsResult<Value> {
        let target_object = self.as_object(target).expect("bind targets are callable");
        let proto = self.prototype_of(target_object)?;
        let length_key = self.state.intrinsics.length_key;
        let length = match self.own_property(target_object, length_key)? {
            Some(_) => match self.get(target_object, length_key, target)? {
                Value::Number(n) if n.is_nan() => 0.0,
                Value::Number(n) => (n.trunc() - args.len().saturating_sub(1) as f64).max(0.0),
                _ => 0.0,
            },
            None => 0.0,
        };
        let name_key = self.realm.key_str("name");
        let name = match self.get(target_object, name_key, target)? {
            Value::String(name) => name,
            _ => JsStr::from_str(""),
        };
        let bound = Callable::Bound(Rc::new(BoundFunction {
            target: target.clone(),
            this: args.first().cloned().unwrap_or(Value::Undefined),
            args: args.iter().skip(1).cloned().collect(),
        }));
        let name = JsStr::from_str("bound ").concat(&name);
        let function =
            self.create_function_object(bound, &name, length.min(f64::from(u32::MAX)) as u32)?;
        if length > f64::from(u32::MAX) {
            let desc = PropertyDescriptor::data(Value::Number(length), false, false, true);
            self.define_property(function, length_key, &desc)?;
        }
        let result = self.realm.set_prototype_of(function, proto);
        result.map_err(|e| self.object_error(e))?;
        Ok(object_value(function))
    }

    /// `call`, `apply` and `bind` on `Function.prototype` (pre-allocated intrinsics).
    pub(crate) fn install_function_prototype(&mut self) -> JsResult<()> {
        let intrinsics = self.state.intrinsics;
        let methods: [(ObjectRef, &str, u32, super::function::NativeFn); 2] = [
            (intrinsics.function_call, "call", 1, Rc::new(function_call)),
            (
                intrinsics.function_apply,
                "apply",
                2,
                Rc::new(function_apply),
            ),
        ];
        for (object, name, length, call) in methods {
            let callable = Callable::Native {
                call,
                constructor: false,
            };
            self.make_function(object, callable, &JsStr::from_str(name), length)?;
            self.define_method(intrinsics.function_prototype, name, object_value(object))?;
        }
        let bind = self.create_native_function("bind", 1, Rc::new(function_bind), false)?;
        self.define_method(intrinsics.function_prototype, "bind", object_value(bind))
    }
}

fn is_constructor_kind(kind: FunctionKind) -> bool {
    kind == FunctionKind::Normal || is_class_constructor(kind)
}

fn require_callable(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<()> {
    if vm.callable(this).is_some() {
        return Ok(());
    }
    let message = format!("Function.prototype.{method} called on a non-function");
    Err(vm.error(ErrorKind::Type, &message))
}

fn function_call(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    require_callable(vm, this, "call")?;
    let this_arg = args.first().cloned().unwrap_or(Value::Undefined);
    vm.call_value(this, &this_arg, args.get(1..).unwrap_or_default())
}

fn function_apply(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    require_callable(vm, this, "apply")?;
    let this_arg = args.first().cloned().unwrap_or(Value::Undefined);
    let list = vm.arguments_list(args.get(1))?;
    vm.call_value(this, &this_arg, &list)
}

fn function_bind(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    require_callable(vm, this, "bind")?;
    vm.bind_function(this, args)
}
