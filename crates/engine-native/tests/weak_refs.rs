//! `WeakRef` and `FinalizationRegistry` on engine-native (VELYR-194): targets are held
//! weakly, and cleanup callbacks run as jobs after a collection, never inside the GC.

use velyr_engine_api::{
    Engine, GcReason, Isolate, IsolateOptions, JsString, MicrotaskProgress, Source, Value,
};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, text: &str) -> Value {
    let source = Source {
        name: "weak_refs.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(value) => value,
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

fn drain(iso: &mut NativeIsolate) {
    while iso.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
}

/// Ends the job (releasing kept WeakRef targets), collects, then drains the cleanups.
fn collect(iso: &mut NativeIsolate) {
    drain(iso);
    iso.request_gc(GcReason::Explicit);
    drain(iso);
}

#[test]
fn weak_ref_derefs_while_target_is_reachable() {
    let mut iso = isolate();
    eval(&mut iso, "var target = {}; var ref = new WeakRef(target);");
    collect(&mut iso);
    assert_eq!(
        eval(&mut iso, "String(ref.deref() === target)"),
        text("true")
    );
}

#[test]
fn weak_ref_is_cleared_once_target_is_collected() {
    let mut iso = isolate();
    eval(&mut iso, "var ref = new WeakRef({});");
    collect(&mut iso);
    assert_eq!(eval(&mut iso, "String(ref.deref())"), text("undefined"));
}

#[test]
fn cleanup_callback_receives_held_values_after_collection() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "var log = []; var registry = new FinalizationRegistry(held => log.push(held));
         registry.register({}, 'a'); var kept = {}; registry.register(kept, 'b');",
    );
    assert_eq!(eval(&mut iso, "String(log)"), text(""));
    collect(&mut iso);
    assert_eq!(eval(&mut iso, "String(log)"), text("a"));
}

#[test]
fn unregistered_targets_never_reach_the_callback() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "var log = []; var token = {}; var registry = new FinalizationRegistry(h => log.push(h));
         registry.register({}, 'a', token); var removed = registry.unregister(token);",
    );
    collect(&mut iso);
    assert_eq!(
        eval(
            &mut iso,
            "String([removed, registry.unregister(token), log.length])"
        ),
        text("true,false,0")
    );
}

#[test]
fn register_validates_its_arguments() {
    let mut iso = isolate();
    let script = "var registry = new FinalizationRegistry(() => {}); var o = {};
        [() => registry.register(1), () => registry.register(o, o),
         () => registry.register(o, 1, 2), () => registry.unregister(1),
         () => registry.register(Symbol.for('x')), () => new FinalizationRegistry(1),
         () => FinalizationRegistry(() => {}), () => new WeakRef(1), () => WeakRef(o)]
        .map(f => { try { f(); return 'ok'; } catch (e) { return e.constructor.name; } })
        .join()";
    let errors = ["TypeError"; 9].join(",");
    assert_eq!(eval(&mut iso, script), text(&errors));
    assert_eq!(
        eval(
            &mut iso,
            "String(registry.register(Symbol('s'), 1, Symbol('t')))"
        ),
        text("undefined")
    );
}
