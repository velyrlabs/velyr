//! `super[expr]` evaluation order, `delete` of optional chains and of `arguments`.

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
fn super_base_read_before_key_conversion() {
    let src =
        "var p1 = {p: 1}, p2 = {p: -1}; var o = {__proto__: p1, m() { return super[k] += 1; }};
               var k = {toString() { Object.setPrototypeOf(o, p2); return 'p'; }}; o.m()";
    assert_eq!(run(src), "2");
}

#[test]
fn super_key_not_evaluated_before_this() {
    let src = "class B { constructor() { throw 1; } }
               class D extends B { constructor() { return super[super()]; } }
               try { new D } catch (e) { e instanceof ReferenceError }";
    assert_eq!(run(src), "true");
}

#[test]
fn delete_optional_chain() {
    let src = "var o = {a: {b: 1}}; [delete o?.a.b, delete o.x?.y, 'b' in o.a]";
    assert_eq!(run(src), "true,true,false");
}

#[test]
fn delete_arguments_is_false() {
    assert_eq!(
        run("function f() { return delete arguments; } f()"),
        "false"
    );
}
