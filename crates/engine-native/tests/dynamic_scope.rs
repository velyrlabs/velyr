//! `with` (14.11) and sloppy direct eval `var` injection (19.2.1.3) through dynamic name
//! lookup, and Annex B.3.2.3 block functions in eval code.

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
fn with_reads_and_writes_object() {
    assert_eq!(
        run("var o = {x: 1}; with (o) { x = 2; var y = x; } o.x + y"),
        "4"
    );
}

#[test]
fn with_respects_unscopables() {
    assert_eq!(
        run("var x = 'outer'; var o = {x: 'in', [Symbol.unscopables]: {x: true}}; with (o) x"),
        "outer"
    );
}

#[test]
fn with_call_receiver() {
    assert_eq!(
        run("var o = {f() { return this === o; }}; with (o) f()"),
        "true"
    );
}

#[test]
fn eval_injects_var_into_function() {
    assert_eq!(
        run("var x = 'g'; function f() { eval('var x = \"l\"'); return x; } f() + x"),
        "lg"
    );
}

#[test]
fn eval_var_conflicts_with_let() {
    assert_eq!(
        run("function f() { let x; { eval('var x'); } } try { f() } catch (e) { e.name }"),
        "SyntaxError"
    );
}

#[test]
fn annex_b_in_eval() {
    assert_eq!(
        run("function f() { eval('{ function h() { return 1; } }'); return h(); } f() + typeof h"),
        "1undefined"
    );
}

#[test]
fn with_reaches_lazy_bodies_and_handlers() {
    let source = "var o = {x: 1}; var x = 0;
        with (o) { var get = function () { return x; }; try { throw 1 } catch (e) { x = 5 } }
        get() + ':' + o.x + ':' + x";
    assert_eq!(run(source), "5:5:0");
}

#[test]
fn eval_var_is_seen_by_nested_functions_typeof_and_delete() {
    let source = "function f() { eval('var v = 1'); var g = () => v;
        return [g(), typeof v, delete v, typeof v].join(); } f()";
    assert_eq!(run(source), "1,number,true,undefined");
}

#[test]
fn eval_sees_named_function_expression() {
    assert_eq!(
        run("(function self() { return eval('typeof self'); })()"),
        "function"
    );
}

#[test]
fn eval_home_of_generators_and_class_keys() {
    let generator = "function* g() { eval('super.x'); } try { g().next() } catch (e) { e.name }";
    assert_eq!(run(generator), "SyntaxError");
    let heritage = "function f() { return class extends eval('arguments')[0] {}; }
        typeof f(Object)";
    assert_eq!(run(heritage), "function");
}

#[test]
fn eval_var_in_parameters_conflicts_with_parameters() {
    let source = "function f(a, p = eval('var a')) {} try { f(); 'none' } catch (e) { e.name }";
    assert_eq!(run(source), "SyntaxError");
    let arguments =
        "function g(p = eval('var arguments')) {} try { g(); 'none' } catch (e) { e.name }";
    assert_eq!(run(arguments), "SyntaxError");
}

#[test]
fn with_assignment_resolves_before_the_value() {
    let source = "var x = 0; var scope = {x: 1};
        with (scope) { x = (delete scope.x, 2); x += (delete scope.x, 3); }
        scope.x + ':' + x";
    assert_eq!(run(source), "5:0");
}

#[test]
fn with_var_initializer_resolves_before_the_value_and_skips_catch_parameters() {
    let source = "var x = 0; var scope = {x: 1};
        with (scope) { var x = (delete scope.x, 2); }
        var r; with ({p: 1}) { try { throw 'e'; } catch (e) { r = e; } }
        scope.x + ':' + x + ':' + r";
    assert_eq!(run(source), "2:0:e");
}

#[test]
fn eval_in_arrow_injects_into_the_arrow() {
    let source = "var f = () => { eval('var a = 1'); return a; }; f() + typeof a";
    assert_eq!(run(source), "1undefined");
}

#[test]
fn nested_arrow_eval_in_field_initializer_rejects_arguments() {
    let source = "class C { x = () => { var t = () => eval('arguments'); return t(); } }
        try { new C().x(); 'none' } catch (e) { e.name }";
    assert_eq!(run(source), "SyntaxError");
}
