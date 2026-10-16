//! Promises and the microtask queue on engine-native (VELYR-108), through the engine-api.

use velyr_engine_api::{
    Engine, EngineFeatures, Isolate, IsolateOptions, JsString, MicrotaskProgress, PromiseState,
    Source, Value,
};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate() -> NativeIsolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut NativeIsolate, text: &str) -> Value {
    let source = Source {
        name: "promises.js".into(),
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

/// Runs `setup`, drains the queue and returns `String(log)`.
fn log_after(setup: &str) -> Value {
    let mut iso = isolate();
    eval(&mut iso, &format!("var log = []; {setup}"));
    drain(&mut iso);
    eval(&mut iso, "String(log)")
}

#[test]
fn promises_feature_is_enabled() {
    assert!(NativeEngine.features().contains(EngineFeatures::PROMISES));
}

#[test]
fn reactions_run_in_fifo_order_after_the_script() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "var log = [];
         Promise.resolve(1).then(v => log.push('a' + v));
         Promise.resolve().then(() => log.push('b')).then(() => log.push('c'));
         log.push('sync');",
    );
    assert_eq!(eval(&mut iso, "String(log)"), text("sync"));
    assert_eq!(
        iso.perform_microtask_checkpoint(),
        MicrotaskProgress::Progressed
    );
    assert_eq!(iso.perform_microtask_checkpoint(), MicrotaskProgress::Idle);
    assert_eq!(eval(&mut iso, "String(log)"), text("sync,a1,b,c"));
}

#[test]
fn thenables_are_unwrapped_in_a_later_job() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "var log = [];
         var p = new Promise(r => r({ then(f) { log.push('then'); f(7); } }));
         log.push('sync');
         p.then(v => log.push(v));",
    );
    assert_eq!(eval(&mut iso, "String(log)"), text("sync"));
    drain(&mut iso);
    assert_eq!(eval(&mut iso, "String(log)"), text("sync,then,7"));
}

#[test]
fn self_resolution_and_throwing_handlers_reject() {
    let source = "var self; self = new Promise(r => Promise.resolve().then(() => r(self)));
        self.catch(e => log.push(e instanceof TypeError));
        Promise.resolve().then(() => { throw 'boom'; }).catch(e => log.push(e));
        new Promise(() => { throw 'ctor'; }).then(null, e => log.push(e));";
    assert_eq!(log_after(source), text("ctor,true,boom"));
}

#[test]
fn catch_and_finally_pass_values_through() {
    let source = "Promise.reject(1).catch(e => e + 1).finally(() => log.push('f'))
            .then(v => log.push(v));
        Promise.reject(2).finally(() => 99).catch(e => log.push('r' + e));";
    assert_eq!(log_after(source), text("f,r2,2"));
}

#[test]
fn all_and_all_settled_collect_in_order() {
    let source = "Promise.all([Promise.resolve(1), 2, { then(f) { f(3); } }])
            .then(v => log.push('all:' + v.join('|')));
        Promise.all([1, Promise.reject('x')]).catch(e => log.push('allerr:' + e));
        Promise.allSettled([1, Promise.reject('y')]).then(r => log.push(
            r.map(e => e.status + '=' + (e.value ?? e.reason)).join('|')));";
    assert_eq!(
        log_after(source),
        text("allerr:x,fulfilled=1|rejected=y,all:1|2|3")
    );
}

#[test]
fn any_race_with_resolvers_and_try() {
    let source =
        "Promise.any([Promise.reject(1), Promise.resolve(2)]).then(v => log.push('any:' + v));
        Promise.any([Promise.reject('a'), Promise.reject('b')]).catch(e => log.push(
            e.constructor.name + ':' + e.errors.join('|') + ':' + (e instanceof Error)));
        Promise.race([new Promise(() => {}), Promise.resolve('r')]).then(v => log.push(v));
        const w = Promise.withResolvers(); w.promise.then(v => log.push('w' + v)); w.resolve(5);
        Promise.try((a, b) => a + b, 3, 4).then(v => log.push('try' + v));
        Promise.try(() => { throw 'e'; }).catch(e => log.push('tryerr' + e));";
    assert_eq!(
        log_after(source),
        text("w5,try7,tryerre,any:2,AggregateError:a|b:true,r")
    );
}

#[test]
fn subclasses_and_species_are_respected() {
    let source = "class P extends Promise {}
        const p = P.resolve(1);
        log.push(p instanceof P, p.then(() => {}) instanceof P, Promise.resolve(p) === p,
            P.resolve(p) === p, Object.prototype.toString.call(p));";
    assert_eq!(
        log_after(source),
        text("true,true,false,true,[object Promise]")
    );
}

#[test]
fn api_promises_settle_once_and_report_state() {
    let mut iso = isolate();
    let (promise, resolver) = iso.create_promise();
    assert_eq!(iso.promise_state(&promise), Some(PromiseState::Pending));
    iso.resolve(resolver, Value::Number(5.0));
    iso.reject(resolver, Value::Number(6.0));
    assert_eq!(
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Number(5.0)))
    );
    let (rejected, resolver) = iso.create_promise();
    iso.reject(resolver, text("no"));
    assert_eq!(
        iso.promise_state(&rejected),
        Some(PromiseState::Rejected(text("no")))
    );
    assert_eq!(iso.promise_state(&Value::Number(1.0)), None);
    let object = eval(&mut iso, "({})");
    assert_eq!(iso.promise_state(&object), None);
}

#[test]
fn api_promises_are_usable_from_js() {
    let mut iso = isolate();
    eval(&mut iso, "var log = [];");
    let (promise, resolver) = iso.create_promise();
    let watch = eval(&mut iso, "(p => { p.then(v => log.push(v)); })");
    iso.call(&watch, &Value::Undefined, &[promise])
        .expect("watch");
    iso.resolve(resolver, Value::Number(8.0));
    drain(&mut iso);
    assert_eq!(eval(&mut iso, "String(log)"), text("8"));
}

#[test]
fn rejection_tracker_reports_unhandled_until_handled() {
    let mut iso = isolate();
    eval(
        &mut iso,
        "globalThis.p = Promise.reject(1); Promise.reject(2).catch(() => {});",
    );
    drain(&mut iso);
    assert_eq!(iso.take_unhandled_rejections(), vec![Value::Number(1.0)]);
    // The set is the current state, not a one-shot queue.
    assert_eq!(iso.take_unhandled_rejections(), vec![Value::Number(1.0)]);
    eval(&mut iso, "p.catch(() => {});");
    drain(&mut iso);
    assert!(iso.take_unhandled_rejections().is_empty());
    let (_, resolver) = iso.create_promise();
    iso.reject(resolver, text("api"));
    assert_eq!(iso.take_unhandled_rejections(), vec![text("api")]);
}

#[test]
fn promise_objects_survive_garbage_collection() {
    let source = "let chain = Promise.resolve(0);
        for (let i = 0; i < 2000; i++) { chain = chain.then(v => { [{}, {}, [i]]; return v + 1; }); }
        chain.then(v => log.push(v));";
    assert_eq!(log_after(source), text("2000"));
}

#[test]
fn promise_state_survives_a_collection_at_every_allocation() {
    use velyr_engine_gc::{Heap, HeapConfig};
    use velyr_engine_native::bytecode_compiler::compile_script;
    use velyr_engine_vm::Value as Val;
    use velyr_engine_vm::interpreter::{Interpreter, PromiseStatus};

    let heap = Heap::new(HeapConfig {
        gc_threshold_bytes: 0,
        hard_limit_bytes: None,
    });
    let mut vm = Interpreter::with_heap(heap);
    let (promise, id) = vm.create_promise().expect("promise");
    let handle = vm.create_handle(&promise).expect("promises are objects");
    let source = "var log = [];
        Promise.all([1, Promise.resolve(2), { then(f) { f([3]); } }]).then(v => log.push(v.join('')));
        Promise.allSettled([Promise.reject('x')]).then(r => log.push(r[0].reason));
        Promise.any([Promise.reject('y')]).catch(e => log.push(e.errors[0]));
        Promise.resolve().finally(() => {}).then(() => log.push('fin'));
        0";
    vm.run_script(&compile_script(source).unwrap())
        .expect("script");
    let object = vm
        .run_script(&compile_script("({ v: 9 })").unwrap())
        .unwrap();
    vm.settle_promise(id, &object, false).expect("settle");
    while vm.perform_microtask_checkpoint().expect("checkpoint") {}
    let log = vm
        .run_script(&compile_script("String(log)").unwrap())
        .unwrap();
    let log = vm.to_js_string(&log).unwrap().to_string_lossy();
    assert_eq!(log, "x,y,123,fin");
    let promise = vm.resolve_handle(handle).expect("handle is live");
    let Some(PromiseStatus::Fulfilled(value)) = vm.promise_status(&promise) else {
        panic!("embedder promise must be fulfilled");
    };
    let getter = compile_script("(o => o.v)").unwrap();
    let getter = vm.run_script(&getter).unwrap();
    let v = vm.call(&getter, &Val::Undefined, &[value]).unwrap();
    assert!(matches!(v, Val::Number(n) if n == 9.0));
}
