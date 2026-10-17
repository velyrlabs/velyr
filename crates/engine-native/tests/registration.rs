//! Registration gates for built-ins whose bodies land in later tasks.

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::Interpreter;

fn run(source: &str) -> String {
    let code = compile_script(source).expect("compiles");
    let mut vm = Interpreter::new();
    let value = vm.run_script(&code).expect("runs");
    vm.to_js_string(&value)
        .expect("printable")
        .to_string_lossy()
}

#[test]
fn uri_registered() {
    assert_eq!(run("typeof encodeURI"), "function");
}

#[test]
fn suppressed_error_registered() {
    assert_eq!(run("typeof SuppressedError"), "function");
}
