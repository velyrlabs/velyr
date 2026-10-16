//! Lazy function bodies (VELYR-201): a nested function compiles on its first call, and the
//! late compile sees the same bindings, `this`, `arguments` and private names as an eager one.

use velyr_engine_api::{
    Engine, HostError, Isolate, IsolateOptions, JsString, MicrotaskProgress, ModuleResolveHook,
    PromiseState, Source, Value,
};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn source(name: &str, text: &str) -> Source {
    Source {
        name: name.into(),
        text: text.into(),
    }
}

fn eval(iso: &mut NativeIsolate, text: &str) -> Value {
    match iso.eval_script(&source("lazy.js", text)) {
        Ok(value) => value,
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

fn drain(iso: &mut NativeIsolate) {
    while iso.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
}

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

fn code_bytes(iso: &NativeIsolate) -> usize {
    iso.heap_stats().breakdown.expect("native breakdown").code
}

/// Runs `script`, drains the microtasks and returns `log.join(" ")`.
fn log_of(script: &str) -> Value {
    let mut iso = isolate();
    eval(&mut iso, &format!("var log = [];\n{script}"));
    drain(&mut iso);
    eval(&mut iso, "log.join(' ')")
}

#[test]
fn an_uncalled_function_compiles_no_bytecode_until_its_first_call() {
    let statements: String = (0..200)
        .map(|i| format!("x = x * {i} + (o.p{i} | 0);\n"))
        .collect();
    let mut iso = isolate();
    let before = code_bytes(&iso);
    eval(
        &mut iso,
        &format!("function big(o) {{ let x = 0;\n{statements}return x; }}"),
    );
    let declared = code_bytes(&iso);
    // Compiled, each statement is several 20-byte instructions plus line-table entries.
    assert!(declared - before < 4_000, "{before} -> {declared}");
    assert_eq!(eval(&mut iso, "big({ p199: 1 })"), Value::Number(1.0));
    let called = code_bytes(&iso);
    assert!(called - declared > 200 * 3 * 20, "{declared} -> {called}");
    // Only the calling script's own code this time: the body compiles once.
    eval(&mut iso, "big({})");
    let again = code_bytes(&iso);
    assert!(again - called < 1_000, "{called} -> {again}");
}

#[test]
fn headers_are_known_before_the_first_call() {
    let value = log_of(
        "function f(a, b = 1, c) {}
         const g = { *m(x) {}, async n() {} };
         log.push(f.length, f.name, String(f), g.m.name, g.m.length);
         log.push(Object.getPrototypeOf(g.m) === Object.getPrototypeOf(function* () {}));
         log.push(typeof g.n().then);",
    );
    assert_eq!(
        value,
        text("1 f function f(a, b = 1, c) {} m 1 true function")
    );
}

#[test]
fn closures_compiled_late_see_their_outer_bindings() {
    let value = log_of(
        "const top = 'T';
         function outer(a) {
           let count = 0;
           function inc(step) { count += step; return count; }
           return { inc, get: () => count + a, nested: () => () => top + a };
         }
         const o = outer(10);
         o.inc(2); o.inc(3);
         log.push(o.get(), o.nested()());
         function wrap() { return function named(n) { return n ? named(n - 1) : 'done'; }; }
         log.push(wrap()(3));
         for (let i = 0; i < 2; i++) { log.push((() => i)()); }",
    );
    assert_eq!(value, text("15 T10 done 0 1"));
}

#[test]
fn generators_and_async_functions_compile_late() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "var log = [];
         function* gen(n) { for (let i = 0; i < n; i++) yield i * 2; }
         async function af(x) { await null; return x + 1; }
         async function* ag() { yield 1; await null; yield 2; }
         log.push([...gen(3)].join(','));
         af(41).then(v => log.push('async' + v));
         (async () => { for await (const v of ag()) log.push('ag' + v); })();",
    );
    drain(&mut iso);
    let value = eval(&mut iso, "log.sort().join(' ')");
    assert_eq!(value, text("0,2,4 ag1 ag2 async42"));
}

#[test]
fn arrows_compiled_late_keep_this_arguments_and_strictness() {
    let value = log_of(
        "function withArrow() {
           const f = () => [this.tag, arguments.length, arguments[1]].join(',');
           return f();
         }
         log.push(withArrow.call({ tag: 't' }, 1, 2, 3));
         function mapped(a) { arguments[0] = 5; return a; }
         log.push(mapped(1));
         function strict() { 'use strict'; return () => this; }
         log.push(strict()() === undefined);
         function sloppy() { return this === globalThis; }
         log.push(sloppy());",
    );
    assert_eq!(value, text("t,3,2 5 true true"));
}

#[test]
fn class_members_and_fields_compile_late() {
    let value = log_of(
        "class C {
           field = this.make();
           arrow = () => this.#p;
           static s = 7;
           #p = 9;
           make() { return 'f'; }
           get p() { return this.#p; }
           static of() { return new this(); }
         }
         class D extends C {
           constructor() { super(); this.d = super.make() + 'd'; }
           make() { return 'g'; }
         }
         const c = C.of();
         log.push(c.field, C.s, c.p, c.arrow());
         const d = new D();
         log.push(d.field, d.d);",
    );
    assert_eq!(value, text("f 7 9 9 g fd"));
}

#[test]
fn a_late_body_resolves_the_names_around_it() {
    let value = log_of(
        "const k = 1;
         function outer(undefined) {
           var v = 2;
           const inner = function self(s) { self = 0; return typeof self; };
           function decl() { return decl === outer.saved; }
           outer.saved = decl;
           return () => [undefined, typeof v, typeof missing, delete v, inner(),
                         decl(), new.target === void 0].join(',');
         }
         log.push(outer('u')());
         log.push((function () { return (() => new.target === undefined)(); })());
         function writes() { try { k = 2; } catch (e) { log.push(e.name); }
                             try { late; } catch (e) { log.push(e.name); } }
         writes(); let late = 1; log.push(late);
         var x = 'outside', probe;
         function pf(_ = probe = function () { return x; }) { var x = 'in'; return () => x; }
         const body = pf(); log.push(probe(), body());",
    );
    assert_eq!(
        value,
        text(
            "u,number,undefined,false,function,true,true true TypeError ReferenceError 1 outside in"
        )
    );
}

#[test]
fn a_late_body_keeps_script_offsets_and_private_names() {
    let value = log_of(
        "function f() {
           class K { static m() {} #p = 3; get() { return () => this.#p; } }
           const where = new Error('x').stack.split('\\n')[1];
           return [String(K.m), String(() => 1), new K().get()(), where.includes('lazy.js:4:')];
         }
         log.push(f().join('|'));",
    );
    assert_eq!(value, text("m() {}|() => 1|3|true"));
}

#[test]
fn direct_eval_is_still_rejected_at_load() {
    let source = "function f() { return eval('1'); }";
    let error = velyr_engine_native::bytecode_compiler::compile_script(source)
        .err()
        .expect("direct eval is unsupported");
    assert_eq!(error.message, "SyntaxError: unsupported: direct eval");
}

#[test]
fn an_unsupported_body_throws_on_its_first_call() {
    let value = log_of(
        "class A { m() { delete super.x; } }
         try { new A().m(); } catch (e) { log.push(e.name, e.message); }",
    );
    assert_eq!(value, text("SyntaxError unsupported: super"));
}

struct Graph(Vec<(&'static str, &'static str)>);

impl ModuleResolveHook for Graph {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        let found = self.0.iter().find(|(name, _)| *name == specifier);
        let (name, text) = found.ok_or_else(|| HostError::TypeError(specifier.into()))?;
        Ok(source(name, text))
    }
}

#[test]
fn module_functions_compile_late_with_imports_and_meta() {
    let mut iso = isolate();
    let lib = "export let n = 1;
               export function bump() { n++; return n; }
               export default function () { return import.meta.url !== undefined; }";
    let main = "import meta, { n, bump } from './lib.js';
                function read() { return [bump(), n, meta()].join(','); }
                globalThis.out = read;";
    let module = iso.compile_module(&source("main.js", main)).expect("main");
    let mut graph = Graph(vec![("./lib.js", lib)]);
    iso.instantiate_module(module, &mut graph).expect("links");
    let promise = iso.evaluate_module(module).expect("evaluates");
    drain(&mut iso);
    assert!(matches!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(_))
    ));
    assert_eq!(eval(&mut iso, "out()"), text("2,2,true"));
}
