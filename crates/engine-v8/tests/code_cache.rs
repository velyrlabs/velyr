//! Code cache round trip and rejection against real V8 (VELYR-60).

use std::collections::HashMap;
use std::sync::Arc;

use velyr_engine_api::{
    CodeCacheStats, Engine, HostError, Isolate, IsolateOptions, ModuleResolveHook, Source, Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_errors::VelyrError;

const ENTRY: &str = "import { a } from 'a.js'; import { b } from 'b.js'; globalThis.out = a + b;";
const A: &str = "export const a = 40;\nexport function unused() { return 'lazy'; }";
const B: &str = "export const b = 2;";

fn src(name: &str, text: &str) -> Source {
    Source {
        name: name.into(),
        text: text.into(),
    }
}

fn isolate() -> V8Isolate {
    V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

#[derive(Default)]
struct Hook {
    sources: HashMap<String, String>,
    caches: HashMap<String, Arc<[u8]>>,
}

impl ModuleResolveHook for Hook {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        match self.sources.get(specifier) {
            Some(text) => Ok(src(specifier, text)),
            None => Err(HostError::Velyr(VelyrError::not_found(specifier))),
        }
    }

    fn code_cache(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.caches.get(name).cloned()
    }
}

fn hook(a: &str) -> Hook {
    Hook {
        sources: [
            ("a.js".to_owned(), a.to_owned()),
            ("b.js".to_owned(), B.to_owned()),
        ]
        .into(),
        caches: HashMap::new(),
    }
}

/// Runs the graph; returns `out` and the stats of the isolate.
fn run(entry_cache: Option<&[u8]>, hook: &mut Hook) -> (Value, CodeCacheStats, V8Isolate) {
    let mut iso = isolate();
    let module = iso
        .compile_module_cached(&src("main.js", ENTRY), entry_cache)
        .expect("compile");
    iso.instantiate_module(module, hook).expect("instantiate");
    iso.evaluate_module(module).expect("evaluate");
    iso.perform_microtask_checkpoint();
    let out = iso.eval_script(&src("probe.js", "out")).expect("probe");
    let stats = iso.code_cache_stats();
    (out, stats, iso)
}

/// Caches of every module of the graph, produced by a first isolate after it ran.
fn produce() -> HashMap<String, Vec<u8>> {
    let (out, stats, mut iso) = run(None, &mut hook(A));
    assert_eq!(out, Value::Number(42.0));
    assert_eq!(stats, CodeCacheStats::default());
    ["main.js", "a.js", "b.js"]
        .into_iter()
        .map(|name| {
            let blob = iso.create_code_cache(name).expect("cache");
            (name.to_owned(), blob)
        })
        .collect()
}

fn consume(caches: &HashMap<String, Vec<u8>>, a: &str) -> (Value, CodeCacheStats) {
    let mut h = hook(a);
    h.caches = caches
        .iter()
        .filter(|(name, _)| *name != "main.js")
        .map(|(name, blob)| (name.clone(), Arc::from(blob.as_slice())))
        .collect();
    let (out, stats, _) = run(Some(&caches["main.js"]), &mut h);
    (out, stats)
}

#[test]
fn every_module_of_the_graph_consumes_its_cache() {
    let caches = produce();
    let (out, stats) = consume(&caches, A);
    assert_eq!(out, Value::Number(42.0));
    assert_eq!(
        stats,
        CodeCacheStats {
            accepted: 3,
            rejected: 0
        }
    );
}

#[test]
fn same_length_different_source_is_rejected_and_compiles_normally() {
    let caches = produce();
    let edited = A.replace("= 40", "= 41");
    assert_eq!(edited.len(), A.len());
    let (out, stats) = consume(&caches, &edited);
    assert_eq!(out, Value::Number(43.0));
    assert_eq!(
        stats,
        CodeCacheStats {
            accepted: 2,
            rejected: 1
        }
    );
}

#[test]
fn garbage_truncated_and_bit_flipped_blobs_are_rejected() {
    let good = produce();
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("garbage", b"not a cache".to_vec()),
        ("empty", Vec::new()),
        ("truncated", good["a.js"][..good["a.js"].len() / 2].to_vec()),
    ];
    for index in (0..good["a.js"].len()).step_by(7) {
        let mut flipped = good["a.js"].clone();
        flipped[index] ^= 0x40;
        cases.push(("flipped", flipped));
    }
    for (label, blob) in cases {
        let mut caches = good.clone();
        caches.insert("a.js".into(), blob);
        let (out, stats) = consume(&caches, A);
        assert_eq!(out, Value::Number(42.0), "{label}");
        assert_eq!(
            stats,
            CodeCacheStats {
                accepted: 2,
                rejected: 1
            },
            "{label}"
        );
    }
}

#[test]
fn unknown_module_has_no_cache() {
    let mut iso = isolate();
    assert!(iso.create_code_cache("missing.js").is_none());
}

const BODY: &str = "return a * 21;";

/// Compiles `BODY` (or `body`) as a function of `params`, consuming `cache`; returns `f(2)`.
fn call_function(iso: &mut V8Isolate, body: &str, params: &[&str], cache: Option<&[u8]>) -> Value {
    let function = iso
        .compile_function(&src("f.js", body), params, cache)
        .expect("compile_function");
    iso.call(&function, &Value::Undefined, &[Value::Number(2.0)])
        .expect("call")
}

fn function_blob() -> Vec<u8> {
    let mut iso = isolate();
    assert_eq!(
        call_function(&mut iso, BODY, &["a"], None),
        Value::Number(42.0)
    );
    iso.create_function_code_cache("f.js").expect("cache")
}

#[test]
fn compile_function_consumes_its_cache() {
    let blob = function_blob();
    let mut iso = isolate();
    let out = call_function(&mut iso, BODY, &["a"], Some(&blob));
    assert_eq!(out, Value::Number(42.0));
    assert_eq!(
        iso.code_cache_stats(),
        CodeCacheStats {
            accepted: 1,
            rejected: 0
        }
    );
}

#[test]
fn compile_function_rejects_cache_for_changed_source() {
    let blob = function_blob();
    let edited = BODY.replace("21", "22");
    let mut iso = isolate();
    let out = call_function(&mut iso, &edited, &["a"], Some(&blob));
    assert_eq!(out, Value::Number(44.0));
    assert_eq!(
        iso.code_cache_stats(),
        CodeCacheStats {
            accepted: 0,
            rejected: 1
        }
    );
}

#[test]
fn compile_function_rejects_cache_for_changed_params() {
    let blob = function_blob();
    let mut iso = isolate();
    let body = "return b * 21;";
    let out = call_function(&mut iso, body, &["b"], Some(&blob));
    assert_eq!(out, Value::Number(42.0));
    assert_eq!(iso.code_cache_stats().rejected, 1);
}
