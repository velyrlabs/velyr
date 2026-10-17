//! Bytecode compiler: disassembly snapshots per construct group, explicit `unsupported`
//! errors, the line table, and end-to-end programs on the bytecode VM.
//!
//! Snapshot expectations live in `src/compiler/bytecode/snapshots/<test name>.txt`.

use velyr_engine_gc::external::{Charge, Kind};
use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::JsStr;
use velyr_engine_vm::bytecode::{
    FunctionCode, FunctionKind, Instruction, Reg, SourcePosition, Target,
};
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

fn disassemble(source: &str) -> String {
    match compile_script(source) {
        Ok(code) => code.to_string(),
        Err(error) => panic!("compile failed: {}", error.message),
    }
}

fn compile_error(source: &str) -> String {
    match compile_script(source) {
        Ok(code) => panic!("expected a compile error, got:\n{code}"),
        Err(error) => error.message,
    }
}

#[track_caller]
fn assert_snapshot(source: &str, expected: &str) {
    let actual = disassemble(source);
    assert_eq!(
        actual.trim_end(),
        expected.trim(),
        "\n--- actual ---\n{actual}"
    );
}

macro_rules! snapshot_test {
    ($name:ident, $source:expr) => {
        #[test]
        fn $name() {
            let expected = include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/compiler/bytecode/snapshots/",
                stringify!($name),
                ".txt"
            ));
            assert_snapshot($source, expected);
        }
    };
}

#[test]
fn script_globals_and_completion_value() {
    assert_snapshot(
        "let a = 1; a + 2;",
        r#"
== "" Script sloppy params=0 length=0 registers=2
   0: DeclareGlobals(c0)
   1: LdaSmi(1)
   2: InitializeGlobalLexical(c1)
   3: LdaGlobal(c1)
   4: Star(r1)
   5: LdaSmi(2)
   6: Add(r1)
   7: Star(r0)
   8: Ldar(r0)
   9: Return
c0: globals functions=[] vars=[] lets=["a"] consts=[]
c1: "a"
"#,
    );
}

snapshot_test!(
    declarations_tdz_and_closure_capture,
    "function f() { g(); let x = 1; const y = x; var z; function g() { return x; } x = 2; y = 3; return y; }"
);

snapshot_test!(
    loops_labels_break_and_continue,
    "function f(n) { let s = 0; outer: for (let i = 0; i < n; i++) { while (true) { if (s > 9) break outer; if (s === 5) continue outer; s += i; break; } } do { s--; } while (s > 0); return s; }"
);

snapshot_test!(
    switch_keeps_tdz_checks_for_case_bindings,
    "function f(k) { switch (k) { case 1: let a = 1; return a; case 2: return a; default: return 0; } }"
);

snapshot_test!(
    operators,
    "a ?? b; a?.b.c; a?.[k](1); a ** 2; x ||= 1; o.p &&= 2; o[k] ??= 3; typeof u; typeof a.b; delete o.p; delete o[k]; k in o; o instanceof C; (1, 2); c ? 1 : 2; o.n++; --x; -x; +x; !x; ~x; void 0; x -= 1;"
);

snapshot_test!(
    destructuring_patterns,
    "function f(o, a) { let { p, q: [r, s = 1], ...rest } = o; [a, o.x] = [o, a]; ({ [p]: r = 2 } = o); return rest; }"
);

snapshot_test!(
    object_and_array_literals,
    "x = { a, b: 1, [k]: 2, get g() { return 1; }, set g(v) {}, m() {}, ...o, __proto__: null, 1.5: 3 }; y = [1, , ...z]; /re/gi; 10n;"
);

snapshot_test!(
    for_in_and_for_of,
    "function f(o, xs) { for (const k in o) { if (k) continue; } for (let x of xs) { if (x) break; g(() => x); } for (o.p of xs); }"
);

snapshot_test!(
    templates_and_spread,
    "function f(a) { return `a${a}b${a}` + tag`x${a}` + g(...a, 1) + new C(...a) + new D(a); }"
);

snapshot_test!(
    try_catch_finally_inside_for_of,
    "function f() { for (const x of xs) { try { if (x) return x; } catch ({ message }) { throw message; } finally { g(); } } }"
);

snapshot_test!(
    let_loop_bindings_get_a_context_per_iteration,
    "function f() { let fs = []; for (let i = 0; i < 2; i++) fs.push(() => i); return fs; }"
);

#[test]
fn script_block_bindings_live_in_registers() {
    assert_snapshot(
        "{ let a = 1; a; }",
        r#"
== "" Script sloppy params=0 length=0 registers=2
   0: LdaTheHole
   1: Star(r1)
   2: LdaSmi(1)
   3: Star(r1)
   4: Ldar(r1)
   5: Star(r0)
   6: Ldar(r0)
   7: Return
"#,
    );
}

#[test]
fn typeof_of_a_missing_global_does_not_throw() {
    let listing = disassemble("typeof missing; typeof arguments;");
    assert_eq!(
        listing.matches("LdaGlobalInsideTypeof").count(),
        2,
        "{listing}"
    );
}

#[test]
fn line_table_maps_instructions_to_source_spans() {
    let source = "let a = 1;\nf(a);\nthrow a;";
    let code = compile_script(source).expect("compiles");
    let text_of = |matches: fn(&Instruction) -> bool| {
        let offset = code
            .instructions
            .iter()
            .position(matches)
            .expect("instruction");
        let (start, end) = code.span_at(offset as u32).expect("position");
        &source[start as usize..end as usize]
    };
    assert_eq!(
        text_of(|i| matches!(i, Instruction::CallUndefinedReceiver { .. })),
        "f(a)"
    );
    assert_eq!(text_of(|i| matches!(i, Instruction::Throw)), "throw a;");
    assert_eq!(
        (code.source_start, code.source_end),
        (0, source.len() as u32)
    );
}

#[test]
fn strict_functions_are_marked_and_reject_writes_to_their_name() {
    let strict = disassemble("'use strict'; (function f() { f = 1; });");
    assert!(strict.contains("Normal strict"), "{strict}");
    assert!(strict.contains("ThrowConstAssignment"), "{strict}");
    let sloppy = disassemble("(function f() { f = 1; });");
    assert!(!sloppy.contains("ThrowConstAssignment"), "{sloppy}");
}

#[test]
fn unsupported_constructs_are_explicit_syntax_errors() {
    // Inside a lazy body they throw on its first call instead (tests/lazy.rs).
    for (source, construct) in [
        ("with ({}) {}", "with statement"),
        ("function f() { eval('1'); }", "direct eval"),
        ("{ function f() {} }", "block-level function in sloppy mode"),
    ] {
        let message = compile_error(source);
        assert_eq!(
            message,
            format!("SyntaxError: unsupported: {construct}"),
            "source: {source}"
        );
    }
}

#[test]
fn early_errors_are_syntax_errors() {
    assert!(compile_error("let a; let a;").starts_with("SyntaxError: "));
    assert!(compile_error("1 +").starts_with("SyntaxError: "));
}

/// Runs a script on the bytecode VM and returns `String(completion)` (or the uncaught
/// exception as `throw <message>`).
fn run(source: &str) -> String {
    let code = compile_script(source).expect("compiles");
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
fn executes_tdz_read_as_reference_error() {
    let source =
        "(function () { try { x; } catch (e) { return e instanceof ReferenceError; } let x; })()";
    assert_eq!(run(source), "true");
}

#[test]
fn executes_per_iteration_closures() {
    let source = "let s = ''; let fs = []; for (let i = 0; i < 3; i++) fs.push(() => i); for (const f of fs) s += f(); s";
    assert_eq!(run(source), "012");
}

#[test]
fn executes_destructuring() {
    let source = "let { a, b: [c, d = 4], ...r } = { a: 1, b: [3], e: 5 }; a + c + d + r.e";
    assert_eq!(run(source), "13");
}

#[test]
fn executes_return_through_finally() {
    let source =
        "let log = ''; function f() { try { return 'r'; } finally { log += 'f'; } } f() + log";
    assert_eq!(run(source), "rf");
}

#[test]
fn executes_labeled_continue() {
    let source = "let n = 0; outer: for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) { if (j === 1) continue outer; n++; } n";
    assert_eq!(run(source), "3");
}

#[test]
fn executes_optional_chaining_and_nullish_coalescing() {
    assert_eq!(run("let o = null; (o?.a ?? 'd') + o?.b.c"), "dundefined");
}

#[test]
fn executes_break_out_of_for_of_closing_the_iterator() {
    let source = "let closed = false; const it = { [Symbol.iterator]() { return { next() { return { value: 1, done: false }; }, return() { closed = true; return {}; } }; } }; for (const x of it) break; closed";
    assert_eq!(run(source), "true");
}

#[test]
fn executes_accessors_and_templates() {
    let source = "let o = { _v: 1, get v() { return this._v; }, set v(x) { this._v = x * 2; } }; o.v = 5; `v=${o.v}`";
    assert_eq!(run(source), "v=10");
}

fn code_with_positions(positions: Vec<SourcePosition>) -> FunctionCode {
    FunctionCode {
        name: JsStr::from_str("f"),
        kind: FunctionKind::Normal,
        is_strict: true,
        parameter_count: 0,
        length: 0,
        register_count: 0,
        instructions: vec![Instruction::LdaUndefined, Instruction::Return],
        constants: Vec::new(),
        handlers: Vec::new(),
        positions,
        source_start: 0,
        source_end: 10,
        script: None,
        charge: Charge::new(Kind::Code, 0),
        lazy: None,
    }
}

#[test]
fn span_at_finds_the_last_position_at_or_before_the_offset() {
    let code = code_with_positions(vec![
        SourcePosition {
            offset: 0,
            start: 0,
            end: 4,
        },
        SourcePosition {
            offset: 1,
            start: 5,
            end: 9,
        },
    ]);
    assert_eq!(code.span_at(0), Some((0, 4)));
    assert_eq!(code.span_at(1), Some((5, 9)));
    assert_eq!(code.span_at(7), Some((5, 9)));
}

#[test]
fn span_at_is_none_before_the_first_position() {
    let code = code_with_positions(vec![SourcePosition {
        offset: 2,
        start: 0,
        end: 1,
    }]);
    assert_eq!(code.span_at(1), None);
}

#[test]
fn jump_targets_are_patchable() {
    let mut jump = Instruction::IteratorStep {
        record: Reg(0),
        done: Target(0),
    };
    *jump.target_mut().expect("jump has a target") = Target(9);
    assert_eq!(
        jump,
        Instruction::IteratorStep {
            record: Reg(0),
            done: Target(9)
        }
    );
    assert!(Instruction::Return.target_mut().is_none());
}
