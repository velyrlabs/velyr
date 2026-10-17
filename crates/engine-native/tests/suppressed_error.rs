//! `Symbol.dispose`, `Symbol.asyncDispose` and `SuppressedError` (bare VM, no preludes).

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
fn dispose_symbols_exist() {
    assert_eq!(
        run("typeof Symbol.dispose + typeof Symbol.asyncDispose"),
        "symbolsymbol"
    );
    assert_eq!(run("Symbol.dispose.description"), "Symbol.dispose");
    assert_eq!(
        run("Symbol.asyncDispose.description"),
        "Symbol.asyncDispose"
    );
    assert_eq!(run("Symbol.dispose !== Symbol.asyncDispose"), "true");
}

#[test]
fn dispose_symbols_are_frozen_constants() {
    let source = "var d = Object.getOwnPropertyDescriptor(Symbol, 'dispose');
        [d.writable, d.enumerable, d.configurable].join()";
    assert_eq!(run(source), "false,false,false");
}

#[test]
fn user_symbols_stay_distinct_from_well_known() {
    assert_eq!(
        run("Symbol('x') !== Symbol.asyncDispose && Symbol('x').description === 'x'"),
        "true"
    );
}

#[test]
fn constructor_sets_error_and_suppressed() {
    let source = "const e = new SuppressedError(1, 2, 'm');
        [e.error, e.suppressed, e.message, e instanceof Error].join()";
    assert_eq!(run(source), "1,2,m,true");
}

#[test]
fn constructor_shape() {
    assert_eq!(run("SuppressedError.length"), "3");
    assert_eq!(
        run("Object.getPrototypeOf(SuppressedError) === Error"),
        "true"
    );
    assert_eq!(
        run("Object.getPrototypeOf(SuppressedError.prototype) === Error.prototype"),
        "true"
    );
    assert_eq!(run("SuppressedError.prototype.name"), "SuppressedError");
    assert_eq!(run("SuppressedError.prototype.message === ''"), "true");
}

#[test]
fn callable_without_new() {
    assert_eq!(
        run("SuppressedError(1, 2) instanceof SuppressedError"),
        "true"
    );
}

#[test]
fn own_properties_are_non_enumerable_and_cause_is_ignored() {
    assert_eq!(
        run("Object.keys(new SuppressedError(1, 2, 'm')).length"),
        "0"
    );
    assert_eq!(
        run(
            "'message' in new SuppressedError(1, 2) && !Object.hasOwn(new SuppressedError(1, 2), 'message')"
        ),
        "true"
    );
    assert_eq!(
        run("Object.hasOwn(new SuppressedError(1, 2, 'm', { cause: 3 }), 'cause')"),
        "false"
    );
}
