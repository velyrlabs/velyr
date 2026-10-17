//! Shared prelude plumbing: running hidden-source JS preludes, and the eager stubs of the
//! locale-sensitive built-ins that force the lazy `Intl` namespace on first call.

use std::rc::Rc;

use velyr_engine_vm::Value;
use velyr_engine_vm::bytecode::{Constant, FunctionCode};
use velyr_engine_vm::interpreter::{Abrupt, Interpreter, NativeFn};

use crate::bytecode_compiler::compile_named_script;

/// Hides the source of every function in `code`, so `toString` shows `[native code]` like
/// a built-in (an empty source range is the VM's cue).
fn hide_source(code: &mut Rc<FunctionCode>) {
    let code = Rc::make_mut(code);
    code.source_end = code.source_start;
    for constant in &mut code.constants {
        if let Constant::Function(inner) = constant {
            hide_source(inner);
        }
    }
}

/// Compiles `source` as script `name`, hides its function sources and runs it.
pub(crate) fn run_prelude(vm: &mut Interpreter, name: &str, source: &str) -> Result<(), String> {
    let mut code = compile_named_script(name, source).map_err(|error| error.message)?;
    hide_source(&mut code);
    match vm.run_script(&code) {
        Ok(_) => Ok(()),
        Err(Abrupt::Throw(value)) => Err(vm.exception_message(&value)),
        Err(abrupt) => Err(format!("{abrupt:?}")),
    }
}

/// `__lazy_force(name)`: materializes the lazy namespace `name`.
fn lazy_force(vm: &mut Interpreter, _: &Value, args: &[Value]) -> Result<Value, Abrupt> {
    if let Some(Value::String(name)) = args.first() {
        vm.force_lazy(&name.to_string_lossy())?;
    }
    Ok(Value::Undefined)
}

/// Runs `source` with the native `__lazy_force` defined as a global; the snippet captures
/// it and deletes the global, which is removed here if it did not.
pub(crate) fn run_prelude_with_force(
    vm: &mut Interpreter,
    name: &str,
    source: &str,
) -> Result<(), String> {
    let native: NativeFn = Rc::new(lazy_force);
    vm.define_global_function("__lazy_force", 1, native)
        .map_err(|_| "cannot define __lazy_force".to_owned())?;
    let result = run_prelude(vm, name, source);
    cleanup_force(vm);
    result
}

fn cleanup_force(vm: &mut Interpreter) {
    let _ = vm.run_script(
        &compile_named_script("velyr:lazy_cleanup", "delete globalThis.__lazy_force;")
            .expect("static source compiles"),
    );
}

const LAZY_STUBS: &str = r#"'use strict';
(() => {
  const force = globalThis.__lazy_force;
  delete globalThis.__lazy_force;
  const stub = (holder, name, length) => {
    const f = { [name](...a) {
      force('Intl');
      const real = holder[name];
      if (real === f) throw new TypeError('Intl failed to install ' + name);
      return Reflect.apply(real, this, a);
    } }[name];
    Object.defineProperty(f, 'length', { value: length, configurable: true });
    Object.defineProperty(holder, name, { value: f, writable: true, configurable: true });
  };
  stub(Number.prototype, 'toLocaleString', 0);
  stub(BigInt.prototype, 'toLocaleString', 0);
  stub(String.prototype, 'localeCompare', 1);
  stub(Date.prototype, 'toLocaleString', 0);
  stub(Date.prototype, 'toLocaleDateString', 0);
  stub(Date.prototype, 'toLocaleTimeString', 0);
  stub(Array.prototype, 'toLocaleString', 0);
})();
"#;

/// Eager stubs for the locale-sensitive built-ins; the first call materializes `Intl`,
/// whose prelude replaces them with the real methods.
pub(crate) fn install_lazy_stubs(vm: &mut Interpreter) -> Result<(), String> {
    run_prelude_with_force(vm, "velyr:lazy_stubs", LAZY_STUBS)
}
