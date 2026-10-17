//! Iterator sequencing and joint iteration: `Iterator.concat`, `zip` and `zipKeyed`.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::NativeEngine;

fn run(text: &str) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "iterator_joint.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        Ok(other) => panic!("`{text}` returned {other:?}"),
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

#[test]
fn concat_chains_iterables() {
    assert_eq!(
        run("JSON.stringify(Iterator.concat([1,2],[3]).toArray())"),
        "[1,2,3]"
    );
}

#[test]
fn concat_rejects_non_iterables_eagerly() {
    let script = "const out = []; for (const bad of [1, 'ab', {}, null]) { \
        try { Iterator.concat([1], bad); out.push('ok'); } catch (e) { out.push(e.constructor.name); } } out.join()";
    assert_eq!(run(script), "TypeError,TypeError,TypeError,TypeError");
}

#[test]
fn concat_is_lazy_and_closes_the_current_iterator() {
    let script = "const log = []; \
        const src = { [Symbol.iterator]() { log.push('open'); return { next() { return { value: 1, done: false }; }, return() { log.push('close'); return {}; } }; } }; \
        const it = Iterator.concat(src); log.push('made'); it.next(); it.return(); log.join()";
    assert_eq!(run(script), "made,open,close");
}

#[test]
fn zip_shortest_is_the_default() {
    assert_eq!(
        run("JSON.stringify(Iterator.zip([[1,2],[3,4]]).toArray())"),
        "[[1,3],[2,4]]"
    );
    assert_eq!(
        run("JSON.stringify(Iterator.zip([[1,2],[3]]).toArray())"),
        "[[1,3]]"
    );
}

#[test]
fn zip_longest_pads() {
    assert_eq!(
        run("JSON.stringify(Iterator.zip([[1,2],[3]],{mode:'longest',padding:[0,9]}).toArray())"),
        "[[1,3],[2,9]]"
    );
}

#[test]
fn zip_strict_throws_on_length_mismatch() {
    let script = "try { Iterator.zip([[1,2],[3]],{mode:'strict'}).toArray(); 'none' } catch (e) { e.constructor.name }";
    assert_eq!(run(script), "TypeError");
    assert_eq!(
        run("JSON.stringify(Iterator.zip([[1],[2]],{mode:'strict'}).toArray())"),
        "[[1,2]]"
    );
}

#[test]
fn zip_rejects_a_bad_mode() {
    assert_eq!(
        run("try { Iterator.zip([[1]],{mode:'x'}); 'none' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn zip_closes_opened_iterators_when_a_later_one_fails() {
    let script = "const log = []; \
        const good = { [Symbol.iterator]() { return { next() { return { done: true }; }, return() { log.push('closed'); return {}; } }; } }; \
        try { Iterator.zip([good, 1]); } catch (e) { log.push(e.constructor.name); } log.join()";
    assert_eq!(run(script), "closed,TypeError");
}

#[test]
fn zip_keyed_builds_null_prototype_objects() {
    assert_eq!(
        run("JSON.stringify(Iterator.zipKeyed({a:[1],b:[2]}).toArray())"),
        r#"[{"a":1,"b":2}]"#
    );
    assert_eq!(
        run("String(Object.getPrototypeOf(Iterator.zipKeyed({a:[1]}).next().value))"),
        "null"
    );
}

#[test]
fn zip_keyed_longest_pads_by_key() {
    assert_eq!(
        run(
            "JSON.stringify(Iterator.zipKeyed({a:[1,2],b:[3]},{mode:'longest',padding:{b:7}}).toArray())"
        ),
        r#"[{"a":1,"b":3},{"a":2,"b":7}]"#
    );
}
