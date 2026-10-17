//! Direct eval with static scope resolution (19.2.1.1 PerformEval) and eval root mode
//! (19.2.1.3 EvalDeclarationInstantiation) on the bytecode VM.

use std::rc::Rc;

use velyr_engine_native::bytecode_compiler::{
    compile_indirect_eval, compile_named_script, compile_script,
};
use velyr_engine_vm::interpreter::{Abrupt, CompileGoal, Interpreter};

/// `String(completion)` of `source` on a VM with the isolate's compile hook.
fn run(source: &str) -> String {
    let mut vm = Interpreter::new();
    vm.set_compile_hook(Rc::new(|text: &str, goal| {
        let code = match goal {
            CompileGoal::Function => compile_named_script("<anonymous>", text),
            CompileGoal::IndirectEval => compile_indirect_eval(text),
        };
        code.map_err(|error| error.message)
    }));
    let code = match compile_script(source) {
        Ok(code) => code,
        Err(error) => return format!("compile error: {}", error.message),
    };
    match vm.run_script(&code) {
        Ok(value) => vm.to_js_string(&value).expect("string").to_string_lossy(),
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(abrupt) => format!("{abrupt:?}"),
    }
}

#[test]
fn reads_and_writes_caller_locals() {
    let source = "function f() { var a = 1; let b = 2; eval('a += b'); return a; } f()";
    assert_eq!(run(source), "3");
}

#[test]
fn closure_from_eval_captures() {
    assert_eq!(
        run("function f(x) { return eval('() => x * 2'); } f(21)()"),
        "42"
    );
}

#[test]
fn strict_eval_vars_are_local() {
    let source = "function f() { 'use strict'; eval('var z = 1'); return typeof z; } f()";
    assert_eq!(run(source), "undefined");
}

#[test]
fn this_new_target_super() {
    let source = "class A { m() { return 'A'; } }
        class B extends A { m() { return eval('super.m() + typeof new.target + (this instanceof B)'); } }
        new B().m()";
    assert_eq!(run(source), "Aundefinedtrue");
}

#[test]
fn eval_lexicals_do_not_leak() {
    let source = "eval('let q = 1'); (0, eval)('let r = 1'); typeof q + typeof r";
    assert_eq!(run(source), "undefinedundefined");
}

#[test]
fn field_initializer_arguments_is_syntax_error() {
    let source = "class C { x = eval('arguments'); } try { new C } catch (e) { e.name }";
    assert_eq!(run(source), "SyntaxError");
}

#[test]
fn completion_value_and_arguments_object() {
    assert_eq!(
        run("function f() { return eval('arguments.length'); } f(1, 2)"),
        "2"
    );
    assert_eq!(run("eval('1; if (true) { 2; }')"), "2");
}
