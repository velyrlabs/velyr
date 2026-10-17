//! Destructuring closes its iterator when a generator `return()` is delivered inside a
//! target (13.15.5.5, 8.6.2).

use std::rc::Rc;

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::Interpreter;

fn run(source: &str) -> String {
    let mut vm = Interpreter::new();
    vm.set_compile_hook(Rc::new(|s: &str, _| {
        compile_script(s).map_err(|e| e.message)
    }));
    let code = compile_script(source).expect("compiles");
    let value = vm.run_script(&code).expect("runs");
    vm.to_js_string(&value).expect("string").to_string_lossy()
}

const ITERABLE: &str = "var closed = 0; var it = { [Symbol.iterator]() { return { next() { return {done: false}; }, return() { closed++; return {}; } }; } };";

#[test]
fn generator_return_inside_assignment_target_closes_iterator() {
    let source = format!(
        "{ITERABLE} function* g() {{ [ {{}}[yield] ] = it; }} var i = g(); i.next(); i.return(); closed"
    );
    assert_eq!(run(&source), "1");
}

#[test]
fn generator_return_inside_binding_default_closes_iterator() {
    let source = format!(
        "{ITERABLE} function* g() {{ var [ a = yield ] = it; }} var i = g(); i.next(); i.return(); closed"
    );
    assert_eq!(run(&source), "1");
}
