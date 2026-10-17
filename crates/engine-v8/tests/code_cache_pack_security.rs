//! Security pins for cached code containers (ML-19).
//!
//! The mmap'd single-pack cache (ML-16: `CodePack`, `map_readonly`) does not exist yet, so the
//! pack checks of ML-19 items (1) "a shrunk file is rejected at open" and (2) "index offsets are
//! bounds-checked with `u64::MAX`" are blocked on ML-16. What exists today is the `VCC1` envelope
//! every blob (and every future pack entry) travels in; these tests pin that its length field is
//! never trusted for slicing and that hostile envelopes fall back to compiling from source.

use velyr_engine_api::{
    CodeCacheStats, Engine, HostError, Isolate, IsolateOptions, ModuleResolveHook, Source, Value,
    function_digest, open_code_cache as open, seal_code_cache as seal, source_digest,
};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_errors::VelyrError;

/// `VCC1` header layout: magic (4), source digest (8), payload digest (8), payload length (8).
const LENGTH_FIELD: std::ops::Range<usize> = 20..28;
const HEADER: usize = 28;

const REJECTED: CodeCacheStats = CodeCacheStats {
    accepted: 0,
    rejected: 1,
};

const HOSTILE_LENGTHS: [u64; 6] = [
    u64::MAX,
    u64::MAX - HEADER as u64 + 1,
    1 << 63,
    u32::MAX as u64 + 1,
    0,
    1,
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

fn with_length_field(blob: &[u8], length: u64) -> Vec<u8> {
    let mut out = blob.to_vec();
    out[LENGTH_FIELD].copy_from_slice(&length.to_le_bytes());
    out
}

struct NoImports;

impl ModuleResolveHook for NoImports {
    fn resolve(&mut self, specifier: &str, _: &str) -> Result<Source, HostError> {
        Err(HostError::Velyr(VelyrError::not_found(
            specifier.to_owned(),
        )))
    }
}

/// Compiles `text` as a module with `cache`, runs it, and returns `globalThis.out` and stats.
fn run_module(text: &str, cache: &[u8]) -> (Value, CodeCacheStats) {
    let mut iso = isolate();
    let module = iso
        .compile_module_cached(&src("/app/m.js", text), Some(cache))
        .expect("compile");
    iso.instantiate_module(module, &mut NoImports)
        .expect("instantiate");
    iso.evaluate_module(module).expect("evaluate");
    iso.perform_microtask_checkpoint();
    let out = iso
        .eval_script(&src("probe.js", "globalThis.out"))
        .expect("probe");
    (out, iso.code_cache_stats())
}

fn module_blob(text: &str) -> Vec<u8> {
    let mut iso = isolate();
    let module = iso
        .compile_module(&src("/app/m.js", text))
        .expect("compile");
    iso.instantiate_module(module, &mut NoImports)
        .expect("instantiate");
    iso.evaluate_module(module).expect("evaluate");
    iso.create_code_cache("/app/m.js").expect("cache")
}

#[test]
fn hostile_length_fields_are_rejected_without_slicing() {
    let digest = source_digest("x");
    let blob = seal(digest, &[7; 64]);
    assert!(open(digest, &blob).is_some());
    let near = [63, 65, 64 + HEADER as u64];
    for length in HOSTILE_LENGTHS.into_iter().chain(near) {
        assert_eq!(
            open(digest, &with_length_field(&blob, length)),
            None,
            "{length}"
        );
    }
}

#[test]
fn every_truncation_of_a_valid_envelope_is_rejected() {
    let digest = source_digest("x");
    let blob = seal(digest, &[7; 64]);
    for end in 0..blob.len() {
        assert_eq!(open(digest, &blob[..end]), None, "prefix of {end} bytes");
    }
}

#[test]
fn trailing_bytes_after_the_declared_payload_are_rejected() {
    let digest = source_digest("x");
    let mut blob = seal(digest, &[7; 64]);
    blob.push(0);
    assert_eq!(open(digest, &blob), None);
}

#[test]
fn header_only_envelope_claiming_a_huge_payload_is_rejected() {
    let digest = source_digest("x");
    let header = with_length_field(&seal(digest, &[]), u64::MAX);
    assert_eq!(header.len(), HEADER);
    assert_eq!(open(digest, &header), None);
}

#[test]
fn module_cache_with_hostile_length_runs_from_source() {
    let text = "globalThis.out = 6 * 7;";
    let blob = module_blob(text);
    for length in HOSTILE_LENGTHS {
        let (out, stats) = run_module(text, &with_length_field(&blob, length));
        assert_eq!(out, Value::Number(42.0), "{length}");
        assert_eq!(stats, REJECTED, "{length}");
    }
}

#[test]
fn module_cache_for_other_text_runs_from_source() {
    let blob = module_blob("globalThis.out = 1;");
    let (out, stats) = run_module("globalThis.out = 2;", &blob);
    assert_eq!(out, Value::Number(2.0));
    assert_eq!(stats, REJECTED);
}

#[test]
fn function_cache_with_hostile_length_runs_from_source() {
    let body = "return a * 21;";
    let params = ["a"];
    let blob = seal(function_digest(&params, body), &[0xAB; 128]);
    for length in HOSTILE_LENGTHS {
        let mut iso = isolate();
        let tampered = with_length_field(&blob, length);
        let function = iso
            .compile_function(&src("/app/f.js", body), &params, Some(&tampered))
            .expect("compile_function");
        let out = iso
            .call(&function, &Value::Undefined, &[Value::Number(2.0)])
            .expect("call");
        assert_eq!(out, Value::Number(42.0), "{length}");
        assert_eq!(iso.code_cache_stats(), REJECTED, "{length}");
    }
}
