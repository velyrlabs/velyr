//! Annex B.3.1 labelled functions, B.3.2 block-level functions, B.3.3 `if` clause functions
//! and B.3.4 `var` in a `catch` block, on the bytecode path.

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
fn block_function_visible_after_block() {
    assert_eq!(
        run(
            "function g() { var a = typeof f; { function f() { return 1; } } return a + f(); } g()"
        ),
        "undefined1"
    );
}

#[test]
fn block_binding_is_hoisted_in_block() {
    assert_eq!(
        run("function g() { { var r = f(); function f() { return 2; } } return r; } g()"),
        "2"
    );
}

#[test]
fn var_binding_updated_on_evaluation_only() {
    assert_eq!(
        run("function g() { { f = 1; function f() {} } return typeof f; } g()"),
        "number"
    );
}

#[test]
fn global_block_function() {
    assert_eq!(run("{ function h() { return 3; } } h()"), "3");
}

#[test]
fn if_clause_function() {
    assert_eq!(run("if (true) function k() { return 4; } k()"), "4");
}

#[test]
fn labelled_function() {
    assert_eq!(run("l: function m() { return 5; } m()"), "5");
}

#[test]
fn var_in_catch_assigns_parameter() {
    assert_eq!(
        run("var out; try { throw 1; } catch (e) { var e = 2; out = e; } [out, e]"),
        "2,"
    );
}

#[test]
fn strict_block_function_stays_block_scoped() {
    assert_eq!(
        run("'use strict'; { function f() {} } typeof f"),
        "undefined"
    );
}

#[test]
fn captured_block_function_and_global_before_block() {
    let source = "var before = typeof h; { function h() { return 6; } }
        function g() { var a = typeof f; { function f() { return 7; } var get = () => f; } return [a, get()(), f()]; }
        [before, h(), g()].join()";
    assert_eq!(run(source), "undefined,6,undefined,7,7");
}

#[test]
fn if_clause_function_skips_parameter() {
    let source = "var after; (function(f) { if (true) function f() {} after = f; }(123)); after";
    assert_eq!(run(source), "123");
}

#[test]
fn if_clause_function_skips_default_parameter() {
    let source = "var after; (function(f = 123) { if (true) function f() {} after = f; }()); after";
    assert_eq!(run(source), "123");
}

#[test]
fn if_clause_function_skips_early_error() {
    let source =
        "var after; (function() { let f = 123; if (true) function f() {} after = f; }()); after";
    assert_eq!(run(source), "123");
}

#[test]
fn block_function_skips_parameter() {
    let source = "var after; (function(f) { { function f() {} } after = f; }(123)); after";
    assert_eq!(run(source), "123");
}
