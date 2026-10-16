//! Continuation context (VELYR-203): a value captured when a promise reaction or `await` is
//! registered and restored while it runs, the same contract engine-v8 gets from V8's
//! continuation-preserved embedder data. `AsyncLocalStorage` is built on it.

use velyr_engine_api::{
    CallContext, Engine, EngineFeatures, GcReason, HostError, HostFunctionDef, Isolate,
    IsolateOptions, Source, Value,
};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn set(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    ctx.set_continuation_context(args.first().cloned().unwrap_or(Value::Undefined));
    Ok(Value::Undefined)
}

fn get(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(ctx.get_continuation_context())
}

fn isolate() -> NativeIsolate {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    for (name, arity, callback) in [("setCtx", 1, set as _), ("getCtx", 0, get as _)] {
        iso.register_host_function(HostFunctionDef {
            name,
            arity,
            callback,
        })
        .expect("host function");
    }
    iso
}

fn eval(iso: &mut NativeIsolate, text: &str) -> Value {
    let source = Source {
        name: "continuation.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source).expect("eval")
}

fn text(iso: &mut NativeIsolate, expression: &str) -> String {
    match eval(iso, expression) {
        Value::String(text) => text.to_string_lossy(),
        other => panic!("{expression}: {other:?}"),
    }
}

#[test]
fn the_engine_declares_the_feature() {
    assert!(
        NativeEngine
            .features()
            .contains(EngineFeatures::CONTINUATION_CONTEXT)
    );
}

#[test]
fn the_isolate_reads_back_what_it_set() {
    let mut iso = isolate();
    iso.set_continuation_context(Value::String("outer".into()));
    let outer = Value::String("outer".into());
    assert_eq!(iso.get_continuation_context(), outer);
    assert_eq!(text(&mut iso, "getCtx()"), "outer");
}

#[test]
fn reactions_and_await_see_the_context_of_their_registration() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "globalThis.seen = [];
         setCtx('A');
         Promise.resolve().then(() => seen.push('then:' + getCtx()));
         (async () => { await null; seen.push('await:' + getCtx()); await null; seen.push('again:' + getCtx()); })();
         (async function* () { await null; seen.push('generator:' + getCtx()); })().next();
         setCtx('B');
         Promise.resolve().then(() => seen.push('later:' + getCtx()));
         setCtx(undefined);",
    );
    iso.perform_microtask_checkpoint();
    assert_eq!(
        text(&mut iso, "seen.join(',')"),
        "then:A,await:A,generator:A,later:B,again:A"
    );
}

#[test]
fn a_pending_reaction_keeps_the_context_it_captured_until_it_settles() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "globalThis.seen = [];
         let resolve;
         const pending = new Promise((r) => { resolve = r; });
         globalThis.resolveLater = resolve;
         setCtx('pending');
         pending.then(() => seen.push(getCtx()));
         setCtx('other');",
    );
    iso.perform_microtask_checkpoint();
    eval(&mut iso, "resolveLater()");
    iso.perform_microtask_checkpoint();
    assert_eq!(text(&mut iso, "seen.join(',')"), "pending");
}

#[test]
fn the_context_is_restored_after_a_job_even_when_it_throws() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "setCtx('job');
         Promise.resolve().then(() => { throw new Error('boom'); });
         Promise.reject(1).then(null, () => { throw 2; }).catch(() => {});",
    );
    iso.set_continuation_context(Value::String("host".into()));
    iso.perform_microtask_checkpoint();
    assert_eq!(iso.get_continuation_context(), Value::String("host".into()));
}

#[test]
fn a_captured_context_survives_collection_until_its_reaction_runs() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "{ const context = { tag: 'kept' };
           globalThis.ref = new WeakRef(context);
           setCtx(context); }
         let resolve;
         new Promise((r) => { resolve = r; }).then(() => { globalThis.seen = getCtx().tag; });
         globalThis.resolveLater = resolve;
         setCtx(undefined);",
    );
    // The checkpoint ends the job that kept the WeakRef target (ClearKeptObjects).
    iso.perform_microtask_checkpoint();
    iso.request_gc(GcReason::Explicit);
    let state = "ref.deref() ? 'alive' : 'collected'";
    assert_eq!(text(&mut iso, state), "alive");
    eval(&mut iso, "resolveLater()");
    iso.perform_microtask_checkpoint();
    assert_eq!(text(&mut iso, "seen"), "kept");
    iso.request_gc(GcReason::Explicit);
    assert_eq!(text(&mut iso, state), "collected");
}
