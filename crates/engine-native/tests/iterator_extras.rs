//! Iterator helpers part 1: `chunks`, `windows`, `includes`, `join` and the argument
//! validation order of the existing helpers (VELYR-109 follow-up).

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::NativeEngine;

fn run(text: &str) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "iterator_extras.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        Ok(other) => panic!("`{text}` returned {other:?}"),
        Err(error) => panic!("`{text}` threw {}", error.message),
    }
}

#[test]
fn chunks_groups_and_keeps_the_partial_tail() {
    assert_eq!(
        run("JSON.stringify([1,2,3,4,5].values().chunks(2).toArray())"),
        "[[1,2],[3,4],[5]]"
    );
}

#[test]
fn chunks_rejects_out_of_range_sizes() {
    let script = "const out = []; for (const n of [0, -0, 2 ** 32]) { \
        try { [].values().chunks(n); } catch (e) { out.push(e.constructor.name); } } \
        for (const bad of ['1', Infinity]) try { [].values().chunks(bad); } catch (e) { out.push(e.constructor.name); } out.join()";
    assert_eq!(
        run(script),
        "RangeError,RangeError,RangeError,TypeError,TypeError"
    );
}

#[test]
fn windows_slides_and_honors_undersized() {
    assert_eq!(
        run("JSON.stringify([1,2,3,4,5].values().windows(2).toArray())"),
        "[[1,2],[2,3],[3,4],[4,5]]"
    );
    assert_eq!(
        run("JSON.stringify([1,2].values().windows(3, 'allow-partial').toArray())"),
        "[[1,2]]"
    );
    assert_eq!(
        run("JSON.stringify([1,2].values().windows(3).toArray())"),
        "[]"
    );
    assert_eq!(
        run("try { [].values().windows(1, 'bad'); 'no' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn includes_uses_same_value_zero_and_skips() {
    assert_eq!(
        run(
            "[[1,2,3].values().includes(2), [1,2,3].values().includes(1, 1), \
             [NaN].values().includes(NaN), [-0].values().includes(0)].join()"
        ),
        "true,false,true,true"
    );
    assert_eq!(
        run("try { [].values().includes(1, -1); 'no' } catch (e) { e.constructor.name }"),
        "RangeError"
    );
}

#[test]
fn includes_closes_on_match() {
    assert_eq!(
        run("let closed = false; let i = 0; \
             const it = { __proto__: Iterator.prototype, next() { return { done: false, value: ++i }; }, \
             return() { closed = true; return {}; } }; \
             it.includes(3) + ':' + closed"),
        "true:true"
    );
}

#[test]
fn join_stringifies_and_defaults_the_separator() {
    assert_eq!(run("[1,2,3].values().join('-')"), "1-2-3");
    assert_eq!(run("['a', null, undefined, 'b'].values().join()"), "a,,,b");
}

#[test]
fn join_closes_when_a_value_cannot_be_stringified() {
    assert_eq!(
        run(
            "let closed = false; const it = { next() { return { done: false, value: Symbol() }; }, \
             return() { closed = true; return {}; } }; \
             try { Iterator.prototype.join.call(it); } catch (e) { e.constructor.name + ':' + closed }"
        ),
        "TypeError:true"
    );
}

#[test]
fn new_methods_have_length_one() {
    assert_eq!(
        run(
            "['chunks','windows','includes','join'].map((n) => Iterator.prototype[n].length).join()"
        ),
        "1,1,1,1"
    );
}

#[test]
fn invalid_take_limit_closes_the_underlying_iterator() {
    assert_eq!(
        run(
            "let closed = false; const it = { next() { throw 1; }, return() { closed = true; return {}; } }; \
             try { Iterator.prototype.take.call(it, -1); } catch (e) { e.constructor.name + ':' + closed }"
        ),
        "RangeError:true"
    );
}

#[test]
fn argument_validation_does_not_coerce_the_bad_argument() {
    assert_eq!(
        run(
            "const log = []; const bad = { valueOf() { log.push('valueOf'); return () => {}; } }; \
             try { Iterator.prototype.map.call({ next() {} }, bad); } catch (e) { log.push(e.constructor.name); } \
             try { [].values().take(Number.MAX_SAFE_INTEGER + 1); } catch (e) { log.push(e.constructor.name); } \
             log.join()"
        ),
        "TypeError,RangeError"
    );
}
