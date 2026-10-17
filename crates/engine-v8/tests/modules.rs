use std::collections::HashMap;
use std::sync::Arc;

use velyr_engine_api::{
    CallContext, Engine, GcReason, HostError, HostFunctionDef, Isolate, IsolateOptions, JsString,
    ModuleResolveHook, PromiseState, SharedSource, Source, Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_errors::VelyrError;

fn isolate() -> V8Isolate {
    V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn src(name: &str, text: &str) -> Source {
    Source {
        name: name.into(),
        text: text.into(),
    }
}

/// In-memory module graph; counts `resolve` calls.
#[derive(Default)]
struct MapHook {
    modules: HashMap<String, String>,
    calls: usize,
}

impl MapHook {
    fn with(modules: &[(&str, &str)]) -> Self {
        Self {
            modules: modules
                .iter()
                .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
                .collect(),
            calls: 0,
        }
    }
}

impl ModuleResolveHook for MapHook {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        self.calls += 1;
        match self.modules.get(specifier) {
            Some(text) => Ok(src(specifier, text)),
            None => Err(HostError::Velyr(VelyrError::not_found(format!(
                "module `{specifier}` not found"
            )))),
        }
    }
}

/// Compiles, instantiates and evaluates `entry`, then drains microtasks.
fn run(iso: &mut V8Isolate, entry: &Source, hook: &mut MapHook) -> Value {
    let module = iso.compile_module(entry).expect("compile");
    iso.instantiate_module(module, hook).expect("instantiate");
    let promise = iso.evaluate_module(module).expect("evaluate");
    iso.perform_microtask_checkpoint();
    promise
}

fn global(iso: &mut V8Isolate, expr: &str) -> Value {
    iso.eval_script(&src("probe.js", expr)).expect("probe")
}

#[test]
fn import_exposes_dependency_export() {
    let mut iso = isolate();
    let mut hook = MapHook::with(&[("b.js", "export const b = 41;")]);
    let entry = src("a.js", "import { b } from 'b.js'; globalThis.out = b + 1;");
    let promise = run(&mut iso, &entry, &mut hook);
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Undefined))
    );
    assert_eq!(global(&mut iso, "out"), Value::Number(42.0));
}

#[test]
fn diamond_and_cycle_compile_each_name_once() {
    let mut iso = isolate();
    let mut hook = MapHook::with(&[
        (
            "left.js",
            "import { base } from 'base.js'; export const l = base + 1;",
        ),
        (
            "right.js",
            "import { base } from 'base.js'; export const r = base + 2;",
        ),
        (
            "base.js",
            "import { l } from 'left.js'; export const base = 10; export function peek() { return l; }",
        ),
    ]);
    let entry = src(
        "main.js",
        "import { l } from 'left.js'; import { r } from 'right.js'; globalThis.out = l + r;",
    );
    run(&mut iso, &entry, &mut hook);
    assert_eq!(global(&mut iso, "out"), Value::Number(23.0));
}

#[test]
fn top_level_await_settles_after_checkpoint() {
    let mut iso = isolate();
    let module = iso
        .compile_module(&src("tla.js", "globalThis.out = await Promise.resolve(1);"))
        .expect("compile");
    iso.instantiate_module(module, &mut MapHook::default())
        .expect("instantiate");
    let promise = iso.evaluate_module(module).expect("evaluate");
    assert_eq!(iso.promise_state(&promise), Some(PromiseState::Pending));
    iso.perform_microtask_checkpoint();
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Undefined))
    );
    assert_eq!(global(&mut iso, "out"), Value::Number(1.0));
}

#[test]
fn syntax_error_reports_position() {
    let mut iso = isolate();
    let exc = iso
        .compile_module(&src("bad.js", "const ok = 1;\nconst = ;"))
        .unwrap_err();
    let text = format!("{} {}", exc.message, exc.stack.unwrap_or_default());
    assert!(text.contains("SyntaxError"), "{text}");
    assert!(text.contains("bad.js:2:"), "{text}");
}

#[test]
fn import_meta_url_is_source_name() {
    let mut iso = isolate();
    let mut hook = MapHook::default();
    let entry = src("file:///app/main.js", "globalThis.out = import.meta.url;");
    run(&mut iso, &entry, &mut hook);
    assert_eq!(
        global(&mut iso, "out"),
        Value::String(JsString::from("file:///app/main.js"))
    );
}

#[test]
fn dynamic_import_rejects_with_unsupported_code() {
    let mut iso = isolate();
    let entry = src(
        "dyn.js",
        "globalThis.out = await import('x').then(() => 'resolved', (e) => e.code);",
    );
    let promise = run(&mut iso, &entry, &mut MapHook::default());
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Undefined))
    );
    assert_eq!(
        global(&mut iso, "out"),
        Value::String(JsString::from("VELYR_UNSUPPORTED"))
    );
}

#[test]
fn unresolved_specifier_fails_instantiate() {
    let mut iso = isolate();
    let module = iso
        .compile_module(&src("a.js", "import 'missing.js';"))
        .expect("compile");
    let exc = iso
        .instantiate_module(module, &mut MapHook::default())
        .unwrap_err();
    assert!(exc.message.contains("missing.js"), "{}", exc.message);
}

#[test]
fn missing_export_fails_instantiate() {
    let mut iso = isolate();
    let mut hook = MapHook::with(&[("b.js", "export const b = 1;")]);
    let module = iso
        .compile_module(&src("a.js", "import { nope } from 'b.js';"))
        .expect("compile");
    let exc = iso.instantiate_module(module, &mut hook).unwrap_err();
    assert!(exc.message.contains("nope"), "{}", exc.message);
}

#[test]
fn evaluate_before_instantiate_is_an_error() {
    let mut iso = isolate();
    let module = iso
        .compile_module(&src("a.js", "export {}"))
        .expect("compile");
    assert!(iso.evaluate_module(module).is_err());
}

#[test]
fn throwing_module_rejects_evaluate_promise() {
    let mut iso = isolate();
    let module = iso
        .compile_module(&src("boom.js", "throw new Error('boom');"))
        .expect("compile");
    iso.instantiate_module(module, &mut MapHook::default())
        .expect("instantiate");
    let promise = iso.evaluate_module(module).expect("evaluate");
    assert!(matches!(
        iso.promise_state(&promise),
        Some(PromiseState::Rejected(_))
    ));
}

#[test]
fn stale_module_handle_is_rejected() {
    let mut iso = isolate();
    let module = iso
        .compile_module(&src("a.js", "export {}"))
        .expect("compile");
    iso.release(module);
    assert!(
        iso.instantiate_module(module, &mut MapHook::default())
            .is_err()
    );
}

struct Hits(u32);

fn bump(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let hits = ctx.isolate_slot::<Hits>().expect("hits slot");
    hits.0 += 1;
    Ok(Value::Number(f64::from(hits.0)))
}

/// `loadSync(name)`: compile, link, evaluate and return the namespace from inside a host function.
fn load_sync(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(name)] = args else {
        return Err(HostError::TypeError("expected a name".into()));
    };
    let name = name.to_string_lossy();
    let mut hook = MapHook::with(&[
        (
            "dep.js",
            "export const answer = 42; globalThis.seen = bump();",
        ),
        ("tla.js", "await 0; export const late = 1;"),
        ("boom.js", "throw new Error('boom');"),
    ]);
    let text = hook.modules.get(&name).cloned().unwrap_or_default();
    let module = ctx.compile_module(&src(&name, &text))?;
    ctx.instantiate_module(module, &mut hook)?;
    let promise = ctx.evaluate_module(module)?;
    match ctx.promise_state(&promise) {
        Some(PromiseState::Fulfilled(_)) => ctx.module_namespace(module),
        Some(PromiseState::Rejected(reason)) => Err(HostError::Thrown(reason)),
        _ => Err(HostError::TypeError("pending".into())),
    }
}

fn sync_isolate() -> V8Isolate {
    let mut iso = isolate();
    iso.set_slot(Hits(0));
    for (name, callback) in [
        ("loadSync", load_sync as velyr_engine_api::HostFn),
        ("bump", bump),
    ] {
        iso.register_host_function(HostFunctionDef {
            name,
            arity: 1,
            callback,
        })
        .expect("register");
    }
    iso
}

#[test]
fn host_function_loads_module_synchronously() {
    let mut iso = sync_isolate();
    let out = global(
        &mut iso,
        "const ns = loadSync('dep.js'); [ns.answer, seen].join()",
    );
    assert_eq!(out, Value::String(JsString::from("42,1")));
}

#[test]
fn sync_load_of_tla_module_is_pending() {
    let mut iso = sync_isolate();
    let out = global(
        &mut iso,
        "try { loadSync('tla.js') } catch (e) { e.message }",
    );
    assert_eq!(out, Value::String(JsString::from("pending")));
}

#[test]
fn sync_load_of_throwing_module_rethrows_reason() {
    let mut iso = sync_isolate();
    let out = global(
        &mut iso,
        "try { loadSync('boom.js') } catch (e) { e.message }",
    );
    assert_eq!(out, Value::String(JsString::from("boom")));
}

#[test]
fn sync_evaluated_rejection_is_not_reported_unhandled() {
    let mut iso = sync_isolate();
    global(&mut iso, "try { loadSync('boom.js') } catch {}");
    iso.perform_microtask_checkpoint();
    assert!(iso.take_unhandled_rejections().is_empty());
}

#[cfg(not(debug_assertions))]
#[test]
fn wide_graph_links_in_bounded_time() {
    const WIDTH: usize = 30000;
    let mut iso = isolate();
    let deps: Vec<(String, String)> = (0..WIDTH)
        .map(|i| (format!("d{i}.js"), format!("export const v = {i};")))
        .collect();
    let entry_text: String = (0..WIDTH)
        .map(|i| format!("import {{ v as v{i} }} from 'd{i}.js';\n"))
        .collect();
    let mut hook = MapHook::with(
        &deps
            .iter()
            .map(|(name, text)| (name.as_str(), text.as_str()))
            .collect::<Vec<_>>(),
    );
    let started = std::time::Instant::now();
    run(&mut iso, &src("wide.js", &entry_text), &mut hook);
    let elapsed = started.elapsed();
    assert!(elapsed.as_millis() < 1500, "linking took {elapsed:?}");
}

fn shared_src(name: &str, text: &Arc<str>) -> SharedSource {
    SharedSource {
        name: name.into(),
        text: Arc::clone(text),
    }
}

/// Serves every module from shared buffers, like the runtime's link hook.
struct SharedHook(HashMap<String, Arc<str>>);

impl ModuleResolveHook for SharedHook {
    fn resolve(&mut self, _specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        unreachable!("the engine links through resolve_shared")
    }

    fn resolve_shared(&mut self, specifier: &str, _: &str) -> Result<SharedSource, HostError> {
        let text = self.0.get(specifier).expect("known module");
        Ok(shared_src(specifier, text))
    }
}

#[test]
fn external_ascii_source_matches_copied_source() {
    let mut iso = isolate();
    let cases = [
        ("ascii", "globalThis.ascii_x = 'abc'.length + 1;", true),
        ("latin1", "globalThis.latin1_x = 'a\u{e7}\u{e3}o'.length;", false),
        ("utf8", "globalThis.utf8_x = '\u{1f600}\u{4e2d}'.length;", false),
    ];
    for (key, text, expect_external) in cases {
        let shared: Arc<str> = Arc::from(text);
        let module = iso
            .compile_module_shared(&shared_src(&format!("{key}.js"), &shared), None)
            .expect("compile shared");
        // The external path keeps one clone alive inside V8; the copy path keeps none.
        assert_eq!(Arc::strong_count(&shared) > 1, expect_external, "{key}");
        iso.instantiate_module(module, &mut MapHook::default())
            .expect("instantiate");
        iso.evaluate_module(module).expect("evaluate");
        let from_shared = global(&mut iso, &format!("{key}_x"));
        run(&mut iso, &src(&format!("{key}-copy.js"), text), &mut MapHook::default());
        assert_eq!(global(&mut iso, &format!("{key}_x")), from_shared, "{key}");
    }
    assert_eq!(global(&mut iso, "ascii_x"), Value::Number(4.0));
}

#[test]
fn external_source_outlives_module_graph() {
    let entry: Arc<str> = Arc::from("import { b } from 'b.js'; globalThis.out = b + 1;");
    let dep: Arc<str> = Arc::from("export const b = 41;");
    let weak = (Arc::downgrade(&entry), Arc::downgrade(&dep));
    let mut iso = isolate();
    let module = iso
        .compile_module_shared(&shared_src("a.js", &entry), None)
        .expect("compile");
    let mut hook = SharedHook(HashMap::from([("b.js".to_owned(), dep)]));
    iso.instantiate_module(module, &mut hook).expect("instantiate");
    drop((entry, hook));
    iso.request_gc(GcReason::Explicit);
    let promise = iso.evaluate_module(module).expect("evaluate");
    iso.perform_microtask_checkpoint();
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Undefined))
    );
    assert_eq!(global(&mut iso, "out"), Value::Number(42.0));
    assert!(weak.0.upgrade().is_some() && weak.1.upgrade().is_some());
    drop(iso);
    // Isolate teardown runs the finalizers, which release the keepalives.
    assert!(weak.0.upgrade().is_none() && weak.1.upgrade().is_none());
}
