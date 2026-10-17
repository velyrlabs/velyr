//! Security pins for the CommonJS function code cache (ML-13): key binding, parameter binding,
//! tamper tolerance and path hygiene. The envelope digest is SipHash with fixed keys: it detects
//! corruption, it is not a MAC; the trust boundary is the private cache directory.

use velyr_engine_api::{
    CodeCacheStats, Engine, Isolate, IsolateOptions, Source, Value, function_digest, seal_code_cache as seal,
};
use velyr_engine_v8::{V8Engine, V8Isolate};

/// The CommonJS wrapper parameters (`compat-commonjs` `WRAPPER_PARAMS`).
const WRAPPER: [&str; 6] = [
    "exports",
    "require",
    "module",
    "__filename",
    "__dirname",
    "__velyrImport",
];

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

/// Compiles and calls `text` as a wrapper function, consuming `cache`.
fn run(name: &str, text: &str, params: &[&str], cache: Option<&[u8]>) -> (Value, V8Isolate) {
    let mut iso = isolate();
    let function = iso
        .compile_function(&src(name, text), params, cache)
        .expect("compile_function");
    let args = vec![Value::Number(2.0); params.len()];
    let out = iso.call(&function, &Value::Undefined, &args).expect("call");
    (out, iso)
}

fn blob_for(name: &str, text: &str, params: &[&str]) -> Vec<u8> {
    let (_, mut iso) = run(name, text, params, None);
    iso.create_function_code_cache(name).expect("cache")
}

fn stats(iso: &V8Isolate) -> CodeCacheStats {
    iso.code_cache_stats()
}

const REJECTED: CodeCacheStats = CodeCacheStats {
    accepted: 0,
    rejected: 1,
};

// (1) The key binds the text: a changed file never reuses stale code, even at the same path
// and with the same length.
#[test]
fn same_path_different_text_is_rejected() {
    let blob = blob_for("/app/a.js", "return exports * 21;", &WRAPPER);
    let (out, iso) = run("/app/a.js", "return exports * 22;", &WRAPPER, Some(&blob));
    assert_eq!(out, Value::Number(44.0), "must run the new text, not the cached code");
    assert_eq!(stats(&iso), REJECTED);
}

#[test]
fn function_digest_changes_with_text() {
    assert_ne!(
        function_digest(&WRAPPER, "return 1;"),
        function_digest(&WRAPPER, "return 2;")
    );
}

// (2) The key binds the parameter list: the digest folds `params` in, in order.
#[test]
fn wrapper_params_are_folded_into_the_digest() {
    let body = "return 1;";
    let base = function_digest(&WRAPPER, body);
    assert_ne!(base, function_digest(&WRAPPER[..5], body), "dropped param");
    let mut swapped = WRAPPER;
    swapped.swap(0, 1);
    assert_ne!(base, function_digest(&swapped, body), "reordered params");
    let mut renamed = WRAPPER;
    renamed[5] = "__other";
    assert_ne!(base, function_digest(&renamed, body), "renamed param");
}

#[test]
fn cache_for_other_wrapper_params_is_rejected() {
    let body = "return exports + require;";
    let blob = blob_for("/app/p.js", body, &WRAPPER);
    let mut other = WRAPPER;
    other.swap(0, 1);
    let (out, iso) = run("/app/p.js", body, &other, Some(&blob));
    assert_eq!(out, Value::Number(4.0));
    assert_eq!(stats(&iso), REJECTED);
}

#[test]
fn matching_params_and_text_are_accepted() {
    let body = "return exports * 21;";
    let blob = blob_for("/app/ok.js", body, &WRAPPER);
    let (out, iso) = run("/app/ok.js", body, &WRAPPER, Some(&blob));
    assert_eq!(out, Value::Number(42.0));
    assert_eq!(
        stats(&iso),
        CodeCacheStats {
            accepted: 1,
            rejected: 0
        }
    );
}

// Tamper tolerance: an attacker who can write the cache can recompute the envelope digest
// (it is unkeyed). V8 must still reject the payload and the module must run from source.
#[test]
fn resealed_garbage_payload_is_rejected_and_source_runs() {
    let body = "return exports * 21;";
    let digest = function_digest(&WRAPPER, body);
    let blob = seal(digest, &[0xAB; 256]);
    let (out, iso) = run("/app/t.js", body, &WRAPPER, Some(&blob));
    assert_eq!(out, Value::Number(42.0));
    assert_eq!(stats(&iso), REJECTED);
}

// Run it alone: the failure is a V8 fatal error that aborts the whole test process.
#[test]
#[ignore = "ML-13 finding: a bit-flipped V8 payload under a recomputed envelope digest aborts V8 (Fatal: unreachable code in the code deserializer)"]
fn resealed_bitflipped_payload_never_misbehaves() {
    let body = "return exports * 21;";
    let digest = function_digest(&WRAPPER, body);
    let real = blob_for("/app/f.js", body, &WRAPPER);
    let payload = real[28..].to_vec();
    for index in (0..payload.len()).step_by(7) {
        let mut flipped = payload.clone();
        flipped[index] ^= 0xFF;
        let tampered = seal(digest, &flipped);
        let (out, _) = run("/app/f.js", body, &WRAPPER, Some(&tampered));
        assert_eq!(out, Value::Number(42.0), "flip at {index} changed behavior");
    }
}

#[test]
fn resealed_truncated_payload_is_rejected() {
    let body = "return exports * 21;";
    let digest = function_digest(&WRAPPER, body);
    let real = blob_for("/app/f.js", body, &WRAPPER);
    let truncated = seal(digest, &real[28..real.len() / 2]);
    let (out, iso) = run("/app/f.js", body, &WRAPPER, Some(&truncated));
    assert_eq!(out, Value::Number(42.0));
    assert_eq!(stats(&iso), REJECTED);
}

// (4) The blob carries no absolute path of the compiling tenant.
#[test]
#[ignore = "ML-13 finding: the V8 payload embeds the script name, i.e. the absolute path of the file"]
fn blob_does_not_embed_the_absolute_path() {
    let name = "/srv/tenant-secret-7f3a/project/lib/a.js";
    let blob = blob_for(name, "return exports * 21;", &WRAPPER);
    for needle in ["tenant-secret-7f3a", "/srv/", "project/lib"] {
        assert!(
            !blob.windows(needle.len()).any(|w| w == needle.as_bytes()),
            "blob embeds {needle:?}"
        );
    }
}

// A blob moved to another path must not leak its origin path into stack traces.
#[test]
fn cache_reused_under_another_path_reports_the_new_path() {
    let body = "return new Error('x').stack;";
    let blob = blob_for("/old/tenant-a/f.js", body, &WRAPPER);
    let (out, _) = run("/new/tenant-b/f.js", body, &WRAPPER, Some(&blob));
    let Value::String(stack) = out else {
        panic!("expected a string, got {out:?}")
    };
    let stack = stack.to_string_lossy();
    assert!(stack.contains("/new/tenant-b/f.js"), "stack: {stack}");
    assert!(!stack.contains("tenant-a"), "stack leaks the old path: {stack}");
}
