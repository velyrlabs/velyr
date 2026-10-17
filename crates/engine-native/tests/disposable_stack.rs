//! `DisposableStack` (explicit resource management, N31).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::NativeEngine;

fn run(text: &str) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "disposable_stack.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        Ok(Value::Bool(b)) => b.to_string(),
        Ok(Value::Number(n)) => n.to_string(),
        Ok(other) => panic!("`{text}` returned {other:?}"),
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

/// Evaluates `body` and reports the constructor name of what it throws.
fn thrown(body: &str) -> String {
    run(&format!(
        "var s = new DisposableStack(); try {{ {body}; 'none' }} catch (e) {{ e.constructor.name }}"
    ))
}

#[test]
fn disposes_in_reverse_order() {
    let source = "var s = new DisposableStack(), log = [];
        s.defer(() => log.push(1)); s.adopt('r', v => log.push(v));
        s.use({ [Symbol.dispose]() { log.push(3); } });
        s.dispose(); log.join()";
    assert_eq!(run(source), "3,r,1");
}

#[test]
fn disposed_flag_and_alias() {
    assert_eq!(
        run(
            "var s = new DisposableStack(); var a = s.disposed; s.dispose(); [a, s.disposed, s.dispose === s[Symbol.dispose]].join()"
        ),
        "false,true,true"
    );
}

#[test]
fn second_dispose_is_a_noop() {
    let source =
        "var n = 0, s = new DisposableStack(); s.defer(() => n++); s.dispose(); s.dispose(); n";
    assert_eq!(run(source), "1");
}

#[test]
fn errors_aggregate_into_suppressed_error() {
    let source = "var s = new DisposableStack(), a = new Error('a'), b = new Error('b');
        s.defer(() => { throw a; }); s.defer(() => { throw b; });
        try { s.dispose(); 'none' } catch (e) {
          [e instanceof SuppressedError, e.error === a, e.suppressed === b].join()
        }";
    assert_eq!(run(source), "true,true,true");
}

#[test]
fn single_error_is_rethrown_as_is() {
    let source = "var s = new DisposableStack(), a = new Error('a');
        s.defer(() => { throw a; }); s.defer(() => {});
        try { s.dispose(); } catch (e) { e === a }";
    assert_eq!(run(source), "true");
}

#[test]
fn use_validates_its_argument() {
    assert_eq!(thrown("s.use(1)"), "TypeError");
    assert_eq!(thrown("s.use({})"), "TypeError");
    assert_eq!(thrown("s.use({ [Symbol.dispose]: 1 })"), "TypeError");
    assert_eq!(
        run("var s = new DisposableStack(); String(s.use(null)) + s.use(undefined)"),
        "nullundefined"
    );
    assert_eq!(
        run("var s = new DisposableStack(), o = { [Symbol.dispose]() {} }; s.use(o) === o"),
        "true"
    );
}

#[test]
fn adopt_and_defer_validate_callbacks() {
    assert_eq!(thrown("s.adopt(1, 2)"), "TypeError");
    assert_eq!(thrown("s.defer(1)"), "TypeError");
    assert_eq!(
        run("var s = new DisposableStack(); s.adopt(7, () => {}) === 7"),
        "true"
    );
}

#[test]
fn disposed_stack_rejects_new_resources() {
    assert_eq!(thrown("s.dispose(); s.defer(() => {})"), "ReferenceError");
    assert_eq!(thrown("s.dispose(); s.use(null)"), "ReferenceError");
    assert_eq!(thrown("s.dispose(); s.move()"), "ReferenceError");
}

#[test]
fn move_transfers_resources() {
    let source = "var s = new DisposableStack(), n = 0; s.defer(() => n++);
        var t = s.move();
        var r = [t instanceof DisposableStack, t !== s, s.disposed, t.disposed];
        s.dispose(); r.push(n); t.dispose(); r.push(n); r.join()";
    assert_eq!(run(source), "true,true,true,false,0,1");
}

#[test]
fn constructor_and_brand_checks() {
    assert_eq!(
        run("try { DisposableStack(); 'none' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(run("DisposableStack.length"), "0");
    assert_eq!(
        run(
            "try { DisposableStack.prototype.use.call({}, null); 'none' } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
    assert_eq!(
        run(
            "try { Object.getOwnPropertyDescriptor(DisposableStack.prototype, 'disposed').get.call({}); 'none' } catch (e) { e.constructor.name }"
        ),
        "TypeError"
    );
}

#[test]
fn property_shape() {
    let source = "var P = DisposableStack.prototype, g = Object.getOwnPropertyDescriptor(P, 'disposed').get;
        [P[Symbol.toStringTag], P.use.length, P.adopt.length, P.defer.length, P.move.length, P.dispose.length,
         P.dispose.name, P[Symbol.dispose].name, g.name, DisposableStack.name,
         Object.getOwnPropertyDescriptor(DisposableStack, 'prototype').writable,
         Object.keys(P).length].join()";
    assert_eq!(
        run(source),
        "DisposableStack,1,2,1,0,0,dispose,dispose,get disposed,DisposableStack,false,0"
    );
}
