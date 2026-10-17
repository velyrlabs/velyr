//! VELYR-104: bytecode interpreter (dispatch loop, frames, inline caches, termination).

use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use velyr_engine_gc::{Heap, HeapConfig};
use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter, NativeFn};
use velyr_engine_vm::{JsStr, Value};

const TEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Termination latency bound. The loops are infinite, so without the back-edge poll they
/// would only end at `TEST_TIMEOUT`; the bound stays far below that but leaves room for a
/// loaded machine (parallel test binaries) descheduling the VM thread for hundreds of ms.
const TERMINATION_LATENCY: Duration = Duration::from_secs(2);

/// `String(completion)`, or `throw <message>` / `terminated` for an abrupt completion.
fn run_in(vm: &mut Interpreter, source: &str) -> String {
    let code = compile_script(source).unwrap_or_else(|e| panic!("compile: {}", e.message));
    match vm.run_script(&code) {
        Ok(value) => match vm.to_js_string(&value) {
            Ok(text) => text.to_string_lossy(),
            Err(_) => "<unprintable>".into(),
        },
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(Abrupt::Terminated) => "terminated".into(),
        Err(Abrupt::Heap(error)) => format!("heap {error}"),
    }
}

fn run(source: &str) -> String {
    run_in(&mut Interpreter::new(), source)
}

/// Runs `f` on a worker thread so a hang or host stack overflow fails the test instead
/// of stalling the suite.
fn with_timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(TEST_TIMEOUT)
        .expect("test finished before the timeout")
}

#[test]
fn evaluates_arithmetic_and_the_completion_value() {
    assert_eq!(run("1 + 2 * 3"), "7");
    assert_eq!(run("let a = 2; a * 21;"), "42");
    assert_eq!(run("'a' + 1"), "a1");
    assert_eq!(run("2 ** 10 - 7 % 4"), "1021");
    assert_eq!(run("(5 >>> 1) | (1 << 4)"), "18");
}

#[test]
fn runs_functions_closures_and_recursion() {
    let fib = "function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(20)";
    assert_eq!(run(fib), "6765");
    let counter =
        "function make() { let c = 0; return () => ++c; } const f = make(); f(); f(); f()";
    assert_eq!(run(counter), "3");
}

#[test]
fn objects_arrays_and_property_access() {
    assert_eq!(run("let o = { a: 1 }; o.b = 2; o.a + o.b"), "3");
    assert_eq!(run("let xs = [1, 2]; xs.push(3); xs.length + xs[2]"), "6");
    let shapes = "let s = 0; for (const o of [{ x: 1 }, { y: 0, x: 2 }, { x: 3 }]) s += o.x; s";
    assert_eq!(run(shapes), "6");
}

#[test]
fn exceptions_unwind_to_handlers_across_frames() {
    let source = "function f() { throw new TypeError('boom'); } let r; try { f(); } catch (e) { r = e.message; } finally { r += '!'; } r";
    assert_eq!(run(source), "boom!");
    assert_eq!(
        run("null.x"),
        "throw TypeError: Cannot read properties of null (reading 'x')"
    );
    assert_eq!(run("nope"), "throw ReferenceError: nope is not defined");
}

#[test]
fn constructs_with_new_and_instanceof() {
    let source = "function P(x) { this.x = x; } P.prototype.get = function () { return this.x; }; const p = new P(4); p.get() + (p instanceof P)";
    assert_eq!(run(source), "5");
}

#[test]
fn deep_js_recursion_is_a_range_error_without_crashing() {
    let result =
        with_timeout(|| run("function f(n) { return n === 0 ? 0 : 1 + f(n - 1); } f(100000)"));
    assert_eq!(result, "throw RangeError: Maximum call stack size exceeded");
    let caught = with_timeout(|| {
        run(
            "function f(n) { return f(n + 1); } try { f(0); } catch (e) { e instanceof RangeError }",
        )
    });
    assert_eq!(caught, "true");
}

#[test]
fn deep_accessor_recursion_is_a_range_error_without_crashing() {
    let result = with_timeout(|| run("const o = { get v() { return this.v; } }; o.v"));
    assert_eq!(result, "throw RangeError: Maximum call stack size exceeded");
}

#[test]
fn terminate_stops_an_infinite_loop_quickly_and_is_uncatchable() {
    for source in [
        "while (true) {}",
        "for (;;) { try { while (true) {} } catch (e) {} }",
    ] {
        let (flag_tx, flag_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut vm = Interpreter::new();
            flag_tx.send(vm.termination_flag()).unwrap();
            let _ = done_tx.send(run_in(&mut vm, source));
        });
        let flag = flag_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        let result = done_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result, "terminated", "{source}");
        assert!(elapsed < TERMINATION_LATENCY, "{source}: {elapsed:?}");
    }
}

#[test]
fn native_functions_receive_this_and_arguments() {
    let mut vm = Interpreter::new();
    let sum: NativeFn = Rc::new(|_, _, args| {
        let total = args.iter().map(|v| match v {
            Value::Number(n) => *n,
            _ => 0.0,
        });
        Ok(Value::Number(total.sum()))
    });
    vm.define_global_function("sum", 2, sum).unwrap();
    assert_eq!(run_in(&mut vm, "sum(1, 2, 3) + sum.length"), "8");
    let f = vm.run_script(&compile_script("sum").unwrap()).unwrap();
    let out = vm.call(&f, &Value::Undefined, &[Value::Number(4.0)]);
    assert!(matches!(out, Ok(Value::Number(n)) if n == 4.0));
}

#[test]
fn globals_persist_across_scripts() {
    let mut vm = Interpreter::new();
    run_in(
        &mut vm,
        "let a = 40; var b = 1; function g() { return a + b; }",
    );
    assert_eq!(run_in(&mut vm, "g() + 1"), "42");
    assert_eq!(
        run_in(&mut vm, "let a = 1"),
        "throw SyntaxError: Identifier 'a' has already been declared"
    );
}

#[test]
fn string_values_cross_as_js_strings() {
    let mut vm = Interpreter::new();
    let value = vm
        .run_script(&compile_script("'x' + `${1 + 1}`").unwrap())
        .unwrap();
    assert!(matches!(value, Value::String(s) if s == JsStr::from_str("x2")));
}

#[test]
fn values_survive_a_collection_at_every_allocation() {
    let heap = Heap::new(HeapConfig {
        gc_threshold_bytes: 0,
        hard_limit_bytes: None,
    });
    let mut vm = Interpreter::with_heap(heap);
    let source = "function make(n) { let c = n; return { get: () => c, xs: [n, n + 1] }; }
        let parts = [];
        for (const o of [make(1), make(2)]) {
            const { xs: [a, b], ...rest } = o;
            parts.push(`${a}${b}${rest.get()}`);
        }
        let err;
        try { null.x; } catch (e) { err = e.message.length > 0; }
        parts.join() + err";
    assert_eq!(run_in(&mut vm, source), "121,232true");
}

#[test]
fn native_isolate_runs_on_the_interpreter_and_honors_termination() {
    use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source};
    use velyr_engine_native::NativeEngine;

    fn source(text: &str) -> Source {
        Source {
            name: "test.js".into(),
            text: text.into(),
        }
    }
    let deep = with_timeout(|| {
        let mut isolate = NativeEngine
            .create_isolate(IsolateOptions::default())
            .unwrap();
        let script = source("function f(n) { return n === 0 ? 0 : 1 + f(n - 1); } f(100000)");
        isolate.eval_script(&script).unwrap_err().message
    });
    assert_eq!(deep, "RangeError: Maximum call stack size exceeded");

    let (handle_tx, handle_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut isolate = NativeEngine
            .create_isolate(IsolateOptions::default())
            .unwrap();
        handle_tx.send(isolate.termination_handle()).unwrap();
        let result = isolate.eval_script(&source("while (true) {}"));
        let _ = done_tx.send(result.unwrap_err().message);
    });
    let handle = handle_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    handle.terminate();
    let message = done_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    assert!(
        started.elapsed() < TERMINATION_LATENCY,
        "{:?}",
        started.elapsed()
    );
    assert_eq!(message, "execution terminated");
}

#[test]
fn handles_root_objects_until_released_with_a_collection_at_every_allocation() {
    let heap = Heap::new(HeapConfig {
        gc_threshold_bytes: 0,
        hard_limit_bytes: None,
    });
    let mut vm = Interpreter::with_heap(heap);
    let object = vm
        .run_script(&compile_script("({ a: 42 })").unwrap())
        .unwrap();
    let handle = vm.create_handle(&object).expect("objects get handles");
    run_in(&mut vm, "let g; for (let i = 0; i < 100; i++) g = [i]; 0");
    let rooted = vm.resolve_handle(handle).expect("handle is live");
    let getter = vm
        .run_script(&compile_script("(function () { return this.a; })").unwrap())
        .unwrap();
    let a = vm.call(&getter, &rooted, &[]);
    assert!(matches!(a, Ok(Value::Number(n)) if n == 42.0));
    vm.release_handle(handle);
    assert!(vm.resolve_handle(handle).is_none());
}

#[test]
fn bigint_constants_parse_decimal_digits() {
    let constant = velyr_engine_vm::bytecode::Constant::bigint_from_decimal("255");
    assert_eq!(format!("{:?}", constant.expect("valid digits")), "255n");
}

#[test]
fn template_objects_are_frozen() {
    assert_eq!(
        run(
            "function t(s) { return s; } var s = t`a${1}b`; Object.isFrozen(s) && Object.isFrozen(s.raw)"
        ),
        "true"
    );
    assert_eq!(
        run(
            "'use strict'; function t(s) { return s; } try { t`a`[0] = 1; 'no' } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

/// A script that declares Annex B global `f`, then runs `InitializeAnnexBGlobal` with 7.
fn annex_b_script() -> Rc<velyr_engine_vm::bytecode::FunctionCode> {
    use velyr_engine_vm::bytecode::{
        ConstIndex, Constant, FunctionCode, GlobalDeclarations, Instruction as I,
    };
    let mut code: FunctionCode = Rc::unwrap_or_clone(compile_script("0").unwrap());
    let declarations = GlobalDeclarations {
        annex_b: vec![JsStr::from_str("f")],
        ..GlobalDeclarations::default()
    };
    code.constants = vec![
        Constant::GlobalDeclarations(Rc::new(declarations)),
        Constant::String(JsStr::from_str("f")),
    ];
    code.instructions = vec![
        I::DeclareGlobals(ConstIndex(0)),
        I::LdaSmi(7),
        I::InitializeAnnexBGlobal(ConstIndex(1)),
        I::LdaUndefined,
        I::Return,
    ];
    Rc::new(code)
}

#[test]
fn annex_b_global_is_declared_and_initialized() {
    let mut vm = Interpreter::new();
    vm.run_script(&annex_b_script()).unwrap();
    assert_eq!(run_in(&mut vm, "f"), "7");
    let probe = "Object.getOwnPropertyDescriptor(globalThis, 'f').configurable";
    assert_eq!(run_in(&mut vm, probe), "false");
}

#[test]
fn annex_b_global_yields_to_lexical_declaration() {
    let mut vm = Interpreter::new();
    assert_eq!(run_in(&mut vm, "let f = 1;"), "undefined");
    vm.run_script(&annex_b_script()).unwrap();
    assert_eq!(run_in(&mut vm, "f"), "1");
    assert_eq!(
        run_in(&mut vm, "Object.getOwnPropertyDescriptor(globalThis, 'f')"),
        "undefined"
    );
}
