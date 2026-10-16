//! Top-level await ordering and `import()` options on engine-native (VELYR-168): independent
//! async modules run concurrently, a module waits only for its own async dependencies, and
//! import attributes are rejected with a clear error.

use std::collections::HashMap;

use velyr_engine_api::{
    Engine, HostError, Isolate, IsolateOptions, MicrotaskProgress, ModuleResolveHook, PromiseState,
    Source, Value,
};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn source(name: &str, text: &str) -> Source {
    Source {
        name: name.into(),
        text: text.into(),
    }
}

fn eval(iso: &mut NativeIsolate, code: &str) -> Value {
    match iso.eval_script(&source("check.js", code)) {
        Ok(value) => value,
        Err(error) => panic!("`{code}` threw {}", error.message),
    }
}

struct Graph(HashMap<String, String>);

impl ModuleResolveHook for Graph {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        let text = self
            .0
            .get(specifier)
            .ok_or_else(|| HostError::TypeError(format!("module `{specifier}` not found")))?;
        Ok(source(specifier, text))
    }
}

/// Evaluates `modules[0]` with `globalThis.log = []`, drains the jobs and returns the log.
fn log_of(modules: &[(&str, &str)]) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    eval(&mut iso, "globalThis.log = []");
    let mut graph = Graph(
        modules
            .iter()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .collect(),
    );
    let (name, text) = modules[0];
    let module = iso.compile_module(&source(name, text)).expect("compiles");
    iso.instantiate_module(module, &mut graph).expect("links");
    let promise = iso.evaluate_module(module).expect("evaluates");
    while iso.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Undefined))
    );
    match eval(&mut iso, "globalThis.log.join()") {
        Value::String(log) => log.to_string_lossy(),
        other => panic!("log is {other:?}"),
    }
}

const LOG_MAIN: &str = "globalThis.log.push('main');";

#[test]
fn independent_top_level_awaits_run_concurrently() {
    let main = format!("import 'x'; import 'y'; {LOG_MAIN}");
    let log = log_of(&[
        ("main.js", &main),
        ("x", "log.push('x1'); await 0; log.push('x2');"),
        ("y", "log.push('y1'); await 0; log.push('y2');"),
    ]);
    assert_eq!(log, "x1,y1,x2,y2,main");
}

#[test]
fn a_module_waits_only_for_its_own_async_dependencies() {
    let main = format!("import 'a'; import 'c'; import 'b'; {LOG_MAIN}");
    let log = log_of(&[
        ("main.js", &main),
        ("a", "log.push('a1'); await 0; log.push('a2');"),
        ("c", "import 'a'; log.push('c');"),
        ("b", "log.push('b');"),
    ]);
    assert_eq!(log, "a1,b,a2,c,main");
}

#[test]
fn a_cycle_waits_for_its_pending_async_member() {
    let main = format!("import 'a'; import 'c'; {LOG_MAIN}");
    let log = log_of(&[
        ("main.js", &main),
        ("a", "import 'b'; log.push('a');"),
        ("b", "import 'a'; log.push('b1'); await 0; log.push('b2');"),
        ("c", "import 'a'; log.push('c');"),
    ]);
    assert_eq!(log, "b1,b2,a,c,main");
}

#[test]
fn an_importer_of_a_cycle_member_waits_for_the_whole_cycle() {
    let main = format!("import 'root'; import 'importer'; {LOG_MAIN}");
    let log = log_of(&[
        ("main.js", &main),
        (
            "root",
            "import 'leaf'; log.push('r1'); await 1; log.push('r2');",
        ),
        (
            "leaf",
            "import 'root'; log.push('l1'); await 1; log.push('l2');",
        ),
        ("importer", "import 'leaf'; log.push('importer');"),
    ]);
    assert_eq!(log, "l1,l2,r1,r2,importer,main");
}

#[test]
fn import_options_are_validated_and_attributes_rejected() {
    let main = "const outcome = (options) => import('x', options)
            .then(() => 'loaded', (e) => `${e.name}: ${e.message}`);
        const results = await Promise.all([
            outcome(1),
            outcome({ with: 1 }),
            outcome({ with: { type: 1 } }),
            outcome({ with: { type: 'json' } }),
        ]);
        log.push(...results);";
    let log = log_of(&[("main.js", main)]);
    let results: Vec<&str> = log.split(',').collect();
    assert!(results[0].starts_with("TypeError: "), "{log}");
    assert!(results[1].starts_with("TypeError: "), "{log}");
    assert!(results[2].starts_with("TypeError: "), "{log}");
    assert!(
        results[3].starts_with("SyntaxError: ") && results[3].contains("type"),
        "{log}"
    );
}
