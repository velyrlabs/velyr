//! Lazy namespace shells: the init runs on the first operation ON the shell, never on
//! `typeof`, `delete` or a descriptor read of the global that holds it.
//!
//! Every direct `realm.get_own_property/own_property_keys/define_own_property/delete`
//! call is either behind a `touch` (`get`, `set`, `get_value`, `set_value` and the
//! `object_ops` wrappers `own_property`, `define_own`, `has_property`, `delete_key`,
//! `own_keys`) or cannot see a shell: `function.rs` and `class.rs` act on fresh function
//! and class objects, `property.rs::define_property` is reached through `define_own`, and
//! `inline_cache.rs` runs only after `get_value`/`set_value` touched the object.

use std::cell::Cell;
use std::rc::Rc;

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

fn run_in(vm: &mut Interpreter, source: &str) -> String {
    let code = compile_script(source).unwrap_or_else(|e| panic!("compile: {}", e.message));
    match vm.run_script(&code) {
        Ok(value) => vm
            .to_js_string(&value)
            .expect("printable")
            .to_string_lossy(),
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(_) => "abrupt".into(),
    }
}

/// A VM with a lazy `Lazy` whose init defines an enumerable `answer` and counts runs.
fn lazy_vm() -> (Interpreter, Rc<Cell<u32>>) {
    let mut vm = Interpreter::new();
    let runs = Rc::new(Cell::new(0));
    let counter = runs.clone();
    let init = Rc::new(move |vm: &mut Interpreter| {
        counter.set(counter.get() + 1);
        // Reading the shell from inside the init must not recurse.
        let code = compile_script("Lazy.answer = 42").expect("compile");
        vm.run_script(&code).map(|_| ())
    });
    vm.define_lazy_namespace("Lazy", init).expect("define");
    (vm, runs)
}

#[test]
fn shell_is_inert_until_touched() {
    let (mut vm, runs) = lazy_vm();
    assert_eq!(vm.pending_lazy(), ["Lazy"]);
    assert_eq!(run_in(&mut vm, "typeof Lazy"), "object");
    assert_eq!(runs.get(), 0);
}

#[test]
fn first_read_materializes_once() {
    let (mut vm, runs) = lazy_vm();
    assert_eq!(run_in(&mut vm, "Lazy.answer"), "42");
    assert_eq!(runs.get(), 1);
    assert_eq!(run_in(&mut vm, "Lazy.answer"), "42");
    assert_eq!(runs.get(), 1);
    assert!(vm.pending_lazy().is_empty());
}

#[test]
fn in_operator_materializes() {
    let (mut vm, runs) = lazy_vm();
    assert_eq!(run_in(&mut vm, "'answer' in Lazy"), "true");
    assert_eq!(runs.get(), 1);
}

#[test]
fn own_keys_materialize() {
    let (mut vm, runs) = lazy_vm();
    assert_eq!(run_in(&mut vm, "Object.keys(Lazy).join()"), "answer");
    assert_eq!(runs.get(), 1);
}

#[test]
fn delete_of_the_global_does_not_materialize() {
    let (mut vm, runs) = lazy_vm();
    assert_eq!(run_in(&mut vm, "delete globalThis.Lazy"), "true");
    assert_eq!(runs.get(), 0);
}

#[test]
fn global_descriptor_read_does_not_materialize() {
    let (mut vm, runs) = lazy_vm();
    let source = "const d = Object.getOwnPropertyDescriptor(globalThis, 'Lazy');\
        [d.writable, d.enumerable, d.configurable].join()";
    assert_eq!(run_in(&mut vm, source), "true,false,true");
    assert_eq!(runs.get(), 0);
}

#[test]
fn write_materializes_then_sets() {
    let (mut vm, runs) = lazy_vm();
    assert_eq!(run_in(&mut vm, "Lazy.x = 1; Lazy.x + Lazy.answer"), "43");
    assert_eq!(runs.get(), 1);
}

#[test]
fn delete_of_a_shell_property_and_define_materialize() {
    let (mut vm, runs) = lazy_vm();
    assert_eq!(run_in(&mut vm, "delete Lazy.answer"), "true");
    assert_eq!(runs.get(), 1);
    let (mut vm, runs) = lazy_vm();
    let source = "Object.defineProperty(Lazy, 'k', { value: 1 }); Object.keys(Lazy).join()";
    assert_eq!(run_in(&mut vm, source), "answer");
    assert_eq!(runs.get(), 1);
}

#[test]
fn force_lazy_by_name() {
    let (mut vm, runs) = lazy_vm();
    vm.force_lazy("Lazy").expect("force");
    vm.force_lazy("Lazy").expect("force twice");
    vm.force_lazy("Missing").expect("absent is a no-op");
    assert_eq!(runs.get(), 1);
}
