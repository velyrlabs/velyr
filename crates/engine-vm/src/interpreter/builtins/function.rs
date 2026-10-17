//! The `Function` constructor and `Function.prototype` (ECMA-262 §20.2), plus
//! %ThrowTypeError%, `eval` (%eval%; direct calls go through `CallDirectEval`) and
//! OrdinaryHasInstance.

use std::rc::Rc;

use super::symbol::SYMBOL_HAS_INSTANCE;
use super::{Interpreter, Intrinsic, JsResult, arg};
use crate::bytecode::{ConstIndex, Constant, FunctionCode};
use crate::interpreter::function::{Callable, Closure};
use crate::interpreter::state::CodeInfo;
use crate::interpreter::{Abrupt, CompileGoal, ErrorKind};
use crate::{JsStr, ObjectKey, PropertyDescriptor, Value};

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.state.intrinsics.function_prototype;
    let empty = Callable::Native {
        call: Rc::new(|_, _, _| Ok(Value::Undefined)),
        constructor: false,
    };
    vm.make_function(proto, empty, &JsStr::from_str(""), 0)?;
    let constructor = vm.constructor("Function", 1, function_constructor, proto)?;
    vm.set_intrinsic(Intrinsic::FunctionConstructor, constructor);
    vm.method(proto, "toString", 0, function_to_string)?;
    let has_instance = vm.symbol_method(proto, SYMBOL_HAS_INSTANCE, 1, function_has_instance)?;
    let desc = PropertyDescriptor::data(has_instance, false, false, false);
    vm.define_property(proto, ObjectKey::Symbol(SYMBOL_HAS_INSTANCE), &desc)?;
    install_throw_type_error(vm)?;
    let global = vm.state.intrinsics.global;
    let eval = vm.new_builtin("eval", 1, global_eval)?;
    let eval_object = vm.as_object(&eval).expect("functions are objects");
    vm.set_intrinsic(Intrinsic::Eval, eval_object);
    vm.define_method(global, "eval", eval)
}

/// %ThrowTypeError% and the restricted `caller`/`arguments` of `Function.prototype`.
fn install_throw_type_error(vm: &mut Interpreter) -> JsResult<()> {
    let thrower = vm.new_builtin("", 0, throw_type_error)?;
    let thrower_object = vm.as_object(&thrower).expect("functions are objects");
    for name in ["length", "name"] {
        let key = vm.realm.key_str(name);
        let desc = PropertyDescriptor {
            configurable: Some(false),
            ..PropertyDescriptor::default()
        };
        vm.define_property(thrower_object, key, &desc)?;
    }
    vm.prevent_extensions(thrower_object)?;
    vm.set_intrinsic(Intrinsic::ThrowTypeError, thrower_object);
    let proto = vm.state.intrinsics.function_prototype;
    for name in ["caller", "arguments"] {
        let key = vm.realm.key_str(name);
        let desc = PropertyDescriptor::accessor(thrower.clone(), thrower.clone(), false, true);
        vm.define_property(proto, key, &desc)?;
    }
    Ok(())
}

fn throw_type_error(vm: &mut Interpreter, _: &Value, _: &[Value]) -> JsResult<Value> {
    vm.throw_type(
        "'caller', 'callee', and 'arguments' properties may not be accessed on strict mode functions or the arguments objects for calls to them",
    )
}

impl Interpreter {
    /// Compiles `source` as `goal` with the embedder's hook and runs it as a script.
    fn run_dynamic(&mut self, source: &str, goal: CompileGoal) -> JsResult<Value> {
        let Some(compile) = self.compile.clone() else {
            let message = "dynamic code evaluation is not available";
            return Err(self.error(ErrorKind::Syntax, message));
        };
        match compile(source, goal) {
            Ok(code) => self.run_script(&code),
            Err(message) => Err(self.compile_error(&message)),
        }
    }

    /// PerformEval for a direct call of %eval% with `source` from the `site` of `info`.
    pub(crate) fn direct_eval(
        &mut self,
        info: &CodeInfo,
        site: ConstIndex,
        source: Option<&Value>,
    ) -> JsResult<Value> {
        let Some(Value::String(text)) = source else {
            return Ok(source.cloned().unwrap_or(Value::Undefined));
        };
        let Constant::EvalSite(site) = &info.code.constants[site.0 as usize] else {
            return Err(self.internal("CallDirectEval expects an eval site"));
        };
        match site.compile(&text.to_string_lossy()) {
            Ok(code) => self.run_eval(code),
            Err(message) => Err(self.compile_error(&message)),
        }
    }

    /// Runs eval code in the running frame's context with its `this`, `new.target` and
    /// home object; its completion value is the result.
    fn run_eval(&mut self, code: FunctionCode) -> JsResult<Value> {
        let frame = self.top();
        let closure = Closure {
            // Not cached in `code_infos`: every eval compiles a fresh code object.
            info: Rc::new(CodeInfo::new(Rc::new(code))),
            context: frame.context.clone(),
            env: frame.env.clone(),
            home: Value::Undefined,
            elements: None,
            _charge: Closure::charge(),
        };
        let env = closure.env.clone();
        self.run_closure(&closure, &Value::Undefined, env, &[], None)
    }

    /// The error a compiler reports as `"<Kind>: <message>"`; SyntaxError without a known kind.
    pub(crate) fn compile_error(&mut self, message: &str) -> Abrupt {
        let named = ErrorKind::ALL.into_iter().find_map(|kind| {
            let rest = message.strip_prefix(kind.name())?.strip_prefix(": ")?;
            Some((kind, rest))
        });
        let (kind, message) = named.unwrap_or((ErrorKind::Syntax, message));
        self.error(kind, message)
    }

    /// OrdinaryHasInstance(C, O).
    pub(crate) fn ordinary_has_instance(&mut self, c: &Value, o: &Value) -> JsResult<bool> {
        match self.callable(c) {
            None => return Ok(false),
            Some(Callable::Bound(bound)) => return self.instance_of_operator(o, &bound.target),
            Some(_) => {}
        }
        let Some(mut cursor) = self.as_object(o) else {
            return Ok(false);
        };
        let c_object = self.as_object(c).expect("callables are objects");
        let proto = self.get_named(c_object, "prototype")?;
        let Some(proto) = self.as_object(&proto) else {
            return self.throw_type("Function has non-object prototype in instanceof check");
        };
        while let Some(next) = self.get_prototype(cursor)? {
            if next == proto {
                return Ok(true);
            }
            cursor = next;
        }
        Ok(false)
    }

    /// InstanceofOperator(V, target).
    pub(crate) fn instance_of_operator(&mut self, value: &Value, target: &Value) -> JsResult<bool> {
        if !matches!(target, Value::Object(_)) {
            return self.throw_type("Right-hand side of 'instanceof' is not an object");
        }
        if let Some(handler) = self.get_method(target, ObjectKey::Symbol(SYMBOL_HAS_INSTANCE))? {
            let result = self.call_value(&handler, target, std::slice::from_ref(value))?;
            return Ok(crate::to_boolean(&result));
        }
        if !self.is_callable(target) {
            return self.throw_type("Right-hand side of 'instanceof' is not callable");
        }
        self.ordinary_has_instance(target, value)
    }
}

/// CreateDynamicFunction for normal functions.
fn function_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let fallback = vm.state.intrinsics.function_prototype;
    create_dynamic_function(vm, args, "function", fallback)
}

/// CreateDynamicFunction: `keyword` is `function`, `function*`, `async function` or
/// `async function*`; `fallback` is the [[Prototype]] when `new.target` has none.
pub(crate) fn create_dynamic_function(
    vm: &mut Interpreter,
    args: &[Value],
    keyword: &str,
    fallback: crate::ObjectRef,
) -> JsResult<Value> {
    let new_target = vm.new_target();
    let mut parameters = Vec::new();
    for value in args.iter().take(args.len().saturating_sub(1)) {
        parameters.push(vm.coerce_rust_string(value)?);
    }
    let body = match args.last() {
        Some(body) => vm.coerce_rust_string(body)?,
        None => String::new(),
    };
    let parameters = parameters.join(",");
    // shortcut: an unclosed `/*` in the parameters could swallow the `) {` glue; real
    // separate parsing of parameters and body would replace this check.
    let unclosed_comment = parameters
        .rfind("/*")
        .is_some_and(|open| !parameters[open + 2..].contains("*/"));
    if unclosed_comment {
        return Err(vm.error(ErrorKind::Syntax, "Unexpected token in parameters"));
    }
    let source = format!("({keyword} anonymous({parameters}\n) {{\n{body}\n}})");
    let function = vm.run_dynamic(&source, CompileGoal::Function)?;
    if !matches!(new_target, Value::Undefined) {
        let object = vm.as_object(&function).expect("functions are objects");
        let proto = vm.proto_from_constructor(&new_target, fallback)?;
        vm.set_prototype(object, Some(proto))?;
    }
    Ok(function)
}

fn global_eval(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    match arg(args, 0) {
        Value::String(source) => {
            vm.run_dynamic(&source.to_string_lossy(), CompileGoal::IndirectEval)
        }
        other => Ok(other),
    }
}

fn function_to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    match vm.callable(this) {
        Some(Callable::Closure(closure)) => {
            let code = &closure.info.code;
            if let Some(script) = &code.script {
                let text = script.slice(code.source_start, code.source_end);
                if !text.is_empty() {
                    return Ok(Value::str(text));
                }
            }
            let name = code.name.to_string_lossy();
            Ok(Value::str(&format!(
                "function {name}() {{ [native code] }}"
            )))
        }
        Some(_) => {
            let object = vm.as_object(this).expect("callables are objects");
            let key = vm.realm.key_str("name");
            let name = match vm.own_property(object, key)? {
                Some(crate::Property {
                    value:
                        crate::PropertyValue::Data {
                            value: Value::String(name),
                            ..
                        },
                    ..
                }) => name.to_string_lossy(),
                _ => String::new(),
            };
            let name = if is_native_function_name(&name) {
                name
            } else {
                String::new()
            };
            Ok(Value::str(&format!(
                "function {name}() {{ [native code] }}"
            )))
        }
        None => vm.throw_type("Function.prototype.toString requires that 'this' be a Function"),
    }
}

/// Whether a native function's name fits NativeFunction syntax (an identifier, an
/// accessor `get x`/`set x` or a computed `[Symbol.x]`).
fn is_native_function_name(name: &str) -> bool {
    let name = name
        .strip_prefix("get ")
        .or_else(|| name.strip_prefix("set "))
        .unwrap_or(name);
    if name.starts_with('[') && name.ends_with(']') {
        return true;
    }
    let mut chars = name.chars();
    let valid_start = |c: char| c == '$' || c == '_' || c.is_alphabetic();
    match chars.next() {
        Some(first) => valid_start(first) && chars.all(|c| valid_start(c) || c.is_alphanumeric()),
        None => false,
    }
}

fn function_has_instance(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(vm.ordinary_has_instance(this, &arg(args, 0))?))
}
