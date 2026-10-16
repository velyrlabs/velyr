//! Language fixes measured against test262 `test/language` (VELYR-154): mapped arguments,
//! global declarations on a non-extensible global and classes nested in class scopes.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::NativeEngine;
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
fn sloppy_arguments_alias_simple_parameters() {
    let source =
        "function f(a, b) { arguments[0] = 9; b = 7; return [a, arguments[1], arguments.length]; }
        function late(a, b) { b = 2; return arguments[1] === undefined && arguments.length === 1; }
        function captured(a) { const read = () => arguments[0]; a = 5; return read(); }
        function escaped(a) { return [arguments, () => a]; }
        const [args, get] = escaped(1); args[0] = 6;
        function dup(a, a) { arguments[1] = 5; arguments[0] = 4; return a; }
        function desc(a) { a = 3; return Object.getOwnPropertyDescriptor(arguments, '0').value; }
        function callee(a) { return arguments.callee === callee; }
        [f(1, 2).join('|'), late(1), captured(1), get(), dup(1, 2), desc(1), callee(0),
         Object.prototype.toString.call(escaped(0)[0])].join()";
    assert_eq!(run(source), "9|7|2,true,5,6,5,3,true,[object Arguments]");
}

#[test]
fn deleting_or_redefining_an_argument_breaks_the_mapping() {
    let source = "function del(a) { delete arguments[0]; arguments[0] = 3; return a; }
        function frozen(a) { Object.defineProperty(arguments, '0', { value: 4, writable: false });
            a = 8; return [a, arguments[0]].join('|'); }
        function accessor(a) { Object.defineProperty(arguments, '0', { get() { return 'g'; } });
            a = 2; return arguments[0] + a; }
        function strict(a) { 'use strict'; arguments[0] = 2; return a; }
        function freeze(a) { Object.freeze(arguments); a = 9; return arguments[0]; }
        [del(1), frozen(1), accessor(1), strict(1), freeze(1)].join()";
    assert_eq!(run(source), "1,8|4,g2,1,1");
}

#[test]
fn classes_nested_in_class_scopes_compile() {
    let source = "class A extends class B { m() { return 1; } } {}
        class C { x = class D {}; static y = class E extends class F {} {}; }
        [String(A), new A().m(), new C().x.name, C.y.name].join('|')";
    assert_eq!(
        run(source),
        "class A extends class B { m() { return 1; } } {}|1|D|E"
    );
}

#[test]
fn global_declarations_need_an_extensible_global() {
    let mut isolate = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "language.js".into(),
        text: "var log = [];
            Object.preventExtensions(this);
            for (const code of ['var fresh;', 'function freshFn() {}', 'var log;']) {
                try { (0, eval)(code); log.push('ok'); } catch (e) { log.push(e.constructor.name); }
            }
            String(log)"
            .into(),
    };
    let result = isolate.eval_script(&source).expect("script runs");
    assert_eq!(result, Value::String("TypeError,TypeError,ok".into()));
}
