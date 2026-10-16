//! ES modules on engine-native (VELYR-110), through the engine-api: linking, live bindings,
//! namespaces, cycles, top-level await, `import.meta` and `import()`.

use std::cell::RefCell;
use std::collections::HashMap;

use velyr_engine_api::{
    CallContext, Engine, EngineFeatures, HostError, HostFunctionDef, Isolate, IsolateOptions,
    JsString, MicrotaskProgress, ModuleResolveHook, PromiseState, ResolverId, Source, Value,
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

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    match iso.eval_script(&source("check.js", code)) {
        Ok(value) => value,
        Err(error) => panic!("`{code}` threw {}", error.message),
    }
}

fn drain(iso: &mut NativeIsolate) {
    while iso.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
}

/// Serves modules by specifier (the specifier is the module name) and counts resolutions.
struct Graph {
    sources: HashMap<String, String>,
    resolved: usize,
}

impl Graph {
    fn new(modules: &[(&str, &str)]) -> Self {
        let sources = modules
            .iter()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .collect();
        Self {
            sources,
            resolved: 0,
        }
    }
}

impl ModuleResolveHook for Graph {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        self.resolved += 1;
        match self.sources.get(specifier) {
            Some(text) => Ok(source(specifier, text)),
            None => Err(HostError::TypeError(format!(
                "module `{specifier}` not found"
            ))),
        }
    }
}

/// Compiles, links and evaluates `modules[0]`; returns the evaluation promise.
fn run_in(iso: &mut NativeIsolate, modules: &[(&str, &str)]) -> Result<Value, String> {
    let mut graph = Graph::new(modules);
    let (name, text) = modules[0];
    let module = iso
        .compile_module(&source(name, text))
        .map_err(|e| e.message)?;
    iso.instantiate_module(module, &mut graph)
        .map_err(|e| e.message)?;
    iso.evaluate_module(module).map_err(|e| e.message)
}

/// Runs the graph, drains the microtasks and returns `globalThis.out`.
fn out(modules: &[(&str, &str)]) -> Value {
    let mut iso = isolate();
    let promise = run_in(&mut iso, modules).expect("the graph links");
    drain(&mut iso);
    match iso.promise_state(&promise) {
        Some(PromiseState::Fulfilled(Value::Undefined)) => {}
        Some(PromiseState::Rejected(reason)) => panic!("evaluation rejected ({reason:?})"),
        other => panic!("unexpected evaluation state {other:?}"),
    }
    eval(&mut iso, "globalThis.out")
}

#[test]
fn modules_feature_is_enabled() {
    assert!(NativeEngine.features().contains(EngineFeatures::MODULES));
}

#[test]
fn named_default_and_renamed_imports() {
    let value = out(&[
        (
            "main.js",
            "import d, { a, b as c } from './lib.js'; globalThis.out = `${d}-${a}-${c}`;",
        ),
        (
            "./lib.js",
            "export const a = 1; const x = 2; export { x as b }; export default 'D';",
        ),
    ]);
    assert_eq!(value, text("D-1-2"));
}

#[test]
fn default_exports_of_functions_classes_and_expressions() {
    let value = out(&[
        (
            "main.js",
            "import f from 'f'; import C from 'c'; import e from 'e'; import g from 'g';
             globalThis.out = [f(), new C().k, e, f.name, g.name, C.name].join();",
        ),
        ("f", "export default function () { return 'f'; }"),
        (
            "c",
            "export default class { constructor() { this.k = 'c'; } }",
        ),
        ("e", "export default 1 + 1;"),
        ("g", "export default function named() {}"),
    ]);
    assert_eq!(value, text("f,c,2,default,named,default"));
}

#[test]
fn bindings_are_live_and_imports_are_immutable() {
    let value = out(&[
        (
            "main.js",
            "import { count, inc } from 'counter';
             const before = count; inc(); inc();
             let error;
             try { count = 9; } catch (e) { error = e.constructor.name; }
             globalThis.out = `${before}-${count}-${error}`;",
        ),
        (
            "counter",
            "export let count = 0; export function inc() { count++; }",
        ),
    ]);
    assert_eq!(value, text("0-2-TypeError"));
}

#[test]
fn re_exports_and_star_exports() {
    let value = out(&[
        (
            "main.js",
            "import { a, renamed, b, ns } from 'hub';
             globalThis.out = [a, renamed, b, ns.a, typeof ns.default].join();",
        ),
        (
            "hub",
            "export { a } from 'a'; export { a as renamed } from 'a';
             export * from 'b'; export * as ns from 'a';",
        ),
        ("a", "export const a = 'A'; export default 'skip';"),
        ("b", "export const b = 'B'; export default 'hidden';"),
    ]);
    assert_eq!(value, text("A,A,B,A,string"));
}

#[test]
fn namespace_objects_are_exotic() {
    let value = out(&[
        (
            "main.js",
            "import * as ns from 'lib';
             const r = [];
             r.push(Object.keys(ns).join('|'));
             r.push(Object.prototype.toString.call(ns));
             r.push(Object.getPrototypeOf(ns) === null);
             r.push(Object.isExtensible(ns), Object.isSealed(ns), Object.isFrozen(ns));
             const d = Object.getOwnPropertyDescriptor(ns, 'z');
             r.push(d.value, d.writable, d.enumerable, d.configurable);
             r.push('z' in ns, 'nope' in ns, ns.nope);
             try { ns.z = 1; r.push('set'); } catch (e) { r.push(e.constructor.name); }
             try { delete ns.z; r.push('deleted'); } catch (e) { r.push(e.constructor.name); }
             r.push(Reflect.deleteProperty(ns, 'nope'));
             globalThis.out = r.join();",
        ),
        (
            "lib",
            "export const z = 26; export let a = 1; export function m() {} export default 0;",
        ),
    ]);
    assert_eq!(
        value,
        text(
            "a|default|m|z,[object Module],true,false,true,false,26,true,true,false,\
             true,false,,TypeError,TypeError,true"
        )
    );
}

#[test]
fn cycles_see_hoisted_functions_and_tdz() {
    let value = out(&[
        (
            "a",
            "import { fromB, readA } from 'b';
             export function hello() { return 'hi'; }
             export let late = 'L';
             globalThis.out = [fromB, readA()].join();",
        ),
        (
            "b",
            "import { hello, late } from 'a';
             export const fromB = hello();
             let tdz;
             try { late; } catch (e) { tdz = e.constructor.name; }
             export function readA() { return tdz + ':' + late; }",
        ),
    ]);
    assert_eq!(value, text("hi,ReferenceError:L"));
}

#[test]
fn modules_are_strict_with_undefined_this_and_evaluate_once() {
    let value = out(&[
        (
            "main.js",
            "import 'side'; import 'side2';
             let sloppy; try { undeclared = 1; } catch (e) { sloppy = e.constructor.name; }
             globalThis.out = [typeof this, sloppy, globalThis.runs].join();",
        ),
        ("side", "globalThis.runs = (globalThis.runs || 0) + 1;"),
        ("side2", "import 'side';"),
    ]);
    assert_eq!(value, text("undefined,ReferenceError,1"));
}

#[test]
fn synchronous_graphs_settle_without_a_checkpoint() {
    let mut iso = isolate();
    let promise = run_in(&mut iso, &[("m", "globalThis.out = 1;")]).expect("links");
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Undefined))
    );
}

#[test]
fn top_level_await_delays_dependents() {
    let mut iso = isolate();
    let promise = run_in(
        &mut iso,
        &[
            (
                "main.js",
                "import { v } from 'slow'; globalThis.out = v + globalThis.log.join('');",
            ),
            (
                "slow",
                "globalThis.log = ['a']; export let v = 0;
                 v = await Promise.resolve(41).then(x => x + 1);
                 for await (const x of [Promise.resolve('b')]) globalThis.log.push(x);",
            ),
        ],
    )
    .expect("links");
    assert_eq!(iso.promise_state(&promise), Some(PromiseState::Pending));
    drain(&mut iso);
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Undefined))
    );
    assert_eq!(eval(&mut iso, "globalThis.out"), text("42ab"));
}

#[test]
fn evaluation_errors_reject_dependents_with_the_same_reason() {
    let mut iso = isolate();
    let promise = run_in(
        &mut iso,
        &[
            ("main.js", "import 'bad'; globalThis.out = 'ran';"),
            ("bad", "throw new RangeError('boom');"),
        ],
    )
    .expect("links");
    drain(&mut iso);
    let Some(PromiseState::Rejected(reason)) = iso.promise_state(&promise) else {
        panic!("expected a rejection");
    };
    assert!(matches!(reason, Value::Object(_)), "{reason:?}");
    assert_eq!(eval(&mut iso, "typeof globalThis.out"), text("undefined"));
    assert!(iso.take_unhandled_rejections().is_empty());
}

#[test]
fn missing_exports_and_syntax_errors_fail_before_evaluation() {
    let mut iso = isolate();
    let missing = run_in(
        &mut iso,
        &[
            ("main.js", "import { nope } from 'lib';"),
            ("lib", "export const a = 1;"),
        ],
    );
    let message = missing.expect_err("missing export");
    assert!(message.starts_with("SyntaxError"), "{message}");
    assert!(message.contains("nope"), "{message}");
    let syntax = iso.compile_module(&source("bad.js", "export const = 1;"));
    assert!(
        syntax
            .expect_err("syntax error")
            .message
            .starts_with("SyntaxError")
    );
    let unresolved = run_in(&mut iso, &[("main2.js", "import 'absent';")]);
    assert!(unresolved.expect_err("unresolved").contains("absent"));
}

#[test]
fn star_exports_with_conflicting_names_are_ambiguous() {
    let mut iso = isolate();
    let result = run_in(
        &mut iso,
        &[
            ("main.js", "import { x } from 'hub';"),
            ("hub", "export * from 'p'; export * from 'q';"),
            ("p", "export const x = 1;"),
            ("q", "export const x = 2;"),
        ],
    );
    assert!(result.expect_err("ambiguous").starts_with("SyntaxError"));
}

#[test]
fn import_meta_url_is_the_module_name() {
    let value = out(&[(
        "file:///app/main.mjs",
        "globalThis.out = import.meta.url + (import.meta === (() => import.meta)());",
    )]);
    assert_eq!(value, text("file:///app/main.mjstrue"));
}

#[test]
fn modules_are_compiled_once_per_name() {
    let mut iso = isolate();
    let mut graph = Graph::new(&[("shared", "export const s = 1;")]);
    for name in ["one.js", "two.js"] {
        let module = iso
            .compile_module(&source(
                name,
                "import { s } from 'shared'; globalThis.out = s;",
            ))
            .expect("compiles");
        iso.instantiate_module(module, &mut graph).expect("links");
        iso.evaluate_module(module).expect("evaluates");
    }
    assert_eq!(graph.resolved, 2);
    assert_eq!(eval(&mut iso, "globalThis.out"), Value::Number(1.0));
}

thread_local! {
    static IMPORTS: RefCell<Vec<(String, String, u64)>> = const { RefCell::new(Vec::new()) };
}

fn op_module_import(_ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (Some(Value::String(specifier)), Some(Value::String(referrer)), Some(Value::Number(id))) =
        (args.first(), args.get(1), args.get(2))
    else {
        return Err(HostError::TypeError("bad import request".into()));
    };
    let request = (
        specifier.to_string_lossy(),
        referrer.to_string_lossy(),
        *id as u64,
    );
    IMPORTS.with(|imports| imports.borrow_mut().push(request));
    Ok(Value::Undefined)
}

#[test]
fn dynamic_import_goes_through_the_host_op() {
    let mut iso = isolate();
    iso.register_host_function(HostFunctionDef {
        name: "op_module_import",
        arity: 3,
        callback: op_module_import,
    })
    .expect("registers");
    run_in(
        &mut iso,
        &[(
            "main.js",
            "import('dep').then(ns => { globalThis.out = ns.d + ns.default; });",
        )],
    )
    .expect("links");
    let requests = IMPORTS.with(|imports| imports.take());
    assert_eq!(requests.len(), 1);
    let (specifier, referrer, id) = requests[0].clone();
    assert_eq!((specifier.as_str(), referrer.as_str()), ("dep", "main.js"));
    let mut graph = Graph::new(&[("dep", "export const d = 'D'; export default await 'X';")]);
    let dep = iso
        .compile_module(&source("dep", &graph.sources["dep"].clone()))
        .expect("compiles");
    iso.instantiate_module(dep, &mut graph).expect("links");
    iso.resolve(ResolverId(id), text("dep"));
    drain(&mut iso);
    assert_eq!(eval(&mut iso, "globalThis.out"), text("DX"));
}

#[test]
fn dynamic_import_without_a_host_rejects() {
    let value = out(&[(
        "main.js",
        "globalThis.out = 'pending';
         import('x').catch(e => { globalThis.out = e instanceof Error; });",
    )]);
    // `out` reads before the rejection handler runs only if import() threw synchronously.
    assert_eq!(value, Value::Bool(true));
}
