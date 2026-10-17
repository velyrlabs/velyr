//! Reference evaluation order for compound, logical and update targets (ECMA-262 13.15.2).

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

/// `String(completion)` on the bytecode path only (no tree-interpreter fallback).
fn run(source: &str) -> String {
    let code = match compile_script(source) {
        Ok(code) => code,
        Err(error) => return format!("compile error: {}", error.message),
    };
    let mut vm = Interpreter::new();
    match vm.run_script(&code) {
        Ok(value) => match vm.to_js_string(&value) {
            Ok(text) => text.to_string_lossy(),
            Err(_) => "<unprintable completion>".into(),
        },
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(abrupt) => format!("{abrupt:?}"),
    }
}

#[test]
fn null_base_throws_before_key_conversion() {
    let src = "var log = []; var k = { toString() { log.push('key'); return 'p'; } };
               try { null[k] += (log.push('rhs'), 1); } catch (e) { log.push(e.constructor.name); } log.join()";
    assert_eq!(run(src), "TypeError");
}

#[test]
fn private_accessor_compound_yields_value_not_setter_result() {
    let src = "class C { get #a() { return 1; } set #a(v) { return 99; } m() { return this.#a += 1; } } new C().m()";
    assert_eq!(run(src), "2");
}
