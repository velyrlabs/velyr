//! Runs one test262 test (all its strict/sloppy variants) on a fresh isolate per variant.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use velyr_engine_api::{
    CallContext, Engine, HostError, HostFunctionDef, Isolate, IsolateOptions, JsException,
    MicrotaskProgress, ModuleResolveHook, PromiseState, ResolverId, Source, Value,
};
use velyr_runtime_errors::VelyrError;

use crate::frontmatter::{self, Meta};
use crate::host262::{self, ParsePhase};
use crate::report::{Outcome, retry_timeout};

const HEAP_LIMIT_BYTES: usize = 1 << 30;
const ASYNC_COMPLETE: &str = "Test262:AsyncTestComplete";
const ASYNC_FAILURE: &str = "Test262:AsyncTestFailure:";
const DEFAULT_INCLUDES: [&str; 2] = ["assert.js", "sta.js"];
const DONE_INCLUDE: &str = "doneprintHandle.js";
// `transfer` is captured before a test can replace it; a detached buffer reports `byteLength` 0.
const PRELUDE: &str = "var $262 = { global: this, detachArrayBuffer: (function (transfer) { return function (b) { if (!b.detached) transfer.call(b); return null; }; })(ArrayBuffer.prototype.transfer) }; $262.evalScript = __evalScript; $262.gc = __gc;";
const DESCRIBE: &str = "(function (value) { return String(value); })";

/// `$262` capabilities engine-api cannot provide: no realm, agent or [[IsHTMLDDA]]
/// access from host functions. Tests that use them are skipped, not run against a lie.
const UNSUPPORTED_HOST_APIS: [&str; 3] = ["$262.createRealm", "$262.agent", "$262.IsHTMLDDA"];

/// `harness/*.js` by file name.
pub struct Harness(HashMap<String, String>);

impl Harness {
    /// Reads every `.js` under `dir`, keyed by its path relative to `dir` (`sm/foo.js` for subdirectories).
    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let mut files = HashMap::new();
        load_dir(dir, dir, &mut files)?;
        Ok(Self(files))
    }

    #[cfg(test)]
    pub fn from_files(files: &[(&str, &str)]) -> Self {
        Self(
            files
                .iter()
                .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
                .collect(),
        )
    }

    fn source_for(&self, meta: &Meta) -> Result<String, String> {
        let mut names: Vec<&str> = Vec::new();
        if !meta.raw {
            names.extend(DEFAULT_INCLUDES);
            if meta.is_async {
                names.push(DONE_INCLUDE);
            }
            names.extend(meta.includes.iter().map(String::as_str));
        }
        let mut text = String::new();
        for name in names {
            let file = self
                .0
                .get(name)
                .ok_or(format!("missing harness file {name}"))?;
            text.push_str(file);
            text.push('\n');
        }
        Ok(text)
    }
}

fn load_dir(root: &Path, dir: &Path, files: &mut HashMap<String, String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            load_dir(root, &path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "js") {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            files.insert(name, std::fs::read_to_string(&path)?);
        }
    }
    Ok(())
}

pub struct TestCase {
    pub abs_path: PathBuf,
    pub text: String,
}

thread_local! {
    // One isolate runs at a time per worker thread, and `HostFn` is a plain `fn`, so `print`
    // output goes through a thread-local that each variant clears.
    static PRINTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    // `import()` requests queued by `op_module_import`: (specifier, referrer, resolver id).
    static IMPORTS: RefCell<Vec<(String, String, u64)>> = const { RefCell::new(Vec::new()) };
}

fn import_op(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    if let (
        Some(Value::String(specifier)),
        Some(Value::String(referrer)),
        Some(Value::Number(id)),
    ) = (args.first(), args.get(1), args.get(2))
    {
        IMPORTS.with_borrow_mut(|queue| {
            queue.push((
                specifier.to_string_lossy(),
                referrer.to_string_lossy(),
                *id as u64,
            ));
        });
    }
    Ok(Value::Undefined)
}

fn print(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    if let Some(Value::String(line)) = args.first() {
        PRINTED.with_borrow_mut(|lines| lines.push(line.to_string_lossy()));
    }
    Ok(Value::Undefined)
}

pub fn run_test<E: Engine>(
    engine: &E,
    harness: &Harness,
    case: &TestCase,
    timeout: Duration,
) -> Outcome {
    let meta = frontmatter::parse(&case.text);
    if let Some(api) = UNSUPPORTED_HOST_APIS
        .iter()
        .find(|api| case.text.contains(**api))
    {
        return Outcome::Skip(format!("{api} is not available through engine-api"));
    }
    let strict_modes: &[bool] = match (meta.raw || meta.module, meta.only_strict, meta.no_strict) {
        (true, _, _) | (_, false, true) => &[false],
        (_, true, _) => &[true],
        _ => &[false, true],
    };
    for &strict in strict_modes {
        let outcome = retry_timeout(|| run_variant(engine, harness, case, &meta, strict, timeout));
        if outcome != Outcome::Pass {
            return outcome;
        }
    }
    Outcome::Pass
}

fn run_variant<E: Engine>(
    engine: &E,
    harness: &Harness,
    case: &TestCase,
    meta: &Meta,
    strict: bool,
    timeout: Duration,
) -> Outcome {
    let opts = IsolateOptions {
        heap_limit_bytes: Some(HEAP_LIMIT_BYTES),
    };
    let mut isolate = match engine.create_isolate(opts) {
        Ok(isolate) => isolate,
        Err(err) => return Outcome::Crash(format!("create_isolate: {err}")),
    };
    PRINTED.with_borrow_mut(Vec::clear);
    IMPORTS.with_borrow_mut(Vec::clear);
    let hosts = [
        ("print", print as velyr_engine_api::HostFn),
        ("op_module_import", import_op),
        ("__evalScript", host262::eval_script_op),
        ("__gc", host262::gc_op),
    ];
    for (name, callback) in hosts {
        let def = HostFunctionDef {
            name,
            arity: 1,
            callback,
        };
        if let Err(err) = isolate.register_host_function(def) {
            return Outcome::Crash(format!("register {name}: {err}"));
        }
    }

    let timed_out = Arc::new(AtomicBool::new(false));
    let (done, wait) = mpsc::channel::<()>();
    let watchdog = {
        let handle = isolate.termination_handle();
        let timed_out = Arc::clone(&timed_out);
        thread::spawn(move || {
            if wait.recv_timeout(timeout) == Err(mpsc::RecvTimeoutError::Timeout) {
                timed_out.store(true, Ordering::SeqCst);
                handle.terminate();
            }
        })
    };
    if !meta.module {
        let Ok(harness_text) = harness.source_for(meta) else {
            return Outcome::Fail("missing harness file".to_owned());
        };
        let script = script_source(&harness_text, case, strict);
        match host262::check_parse_phase(&mut isolate, meta, &script) {
            ParsePhase::NotApplicable => {}
            ParsePhase::Unavailable => {
                return Outcome::Skip(
                    "check_script is not available through engine-api".to_owned(),
                );
            }
            ParsePhase::Done(verdict) => {
                return verdict.map_or_else(Outcome::Fail, |()| Outcome::Pass);
            }
        }
    }
    let result = execute(&mut isolate, harness, case, meta, strict);
    drop(done);
    let _ = watchdog.join();

    if timed_out.load(Ordering::SeqCst) {
        return Outcome::Timeout;
    }
    match result {
        Ok(()) => Outcome::Pass,
        Err(message) => Outcome::Fail(message),
    }
}

/// Runs the test and judges it against `negative` and `async`.
fn execute<I: Isolate>(
    isolate: &mut I,
    harness: &Harness,
    case: &TestCase,
    meta: &Meta,
    strict: bool,
) -> Result<(), String> {
    let harness_text = harness.source_for(meta)?;
    if !meta.raw {
        // Engines without object support (engine-native) cannot run this; the test then fails on its own.
        let _ = isolate.eval_script(&source("prelude", PRELUDE.to_owned()));
    }
    let run = if meta.module {
        run_module(isolate, &harness_text, case)
    } else {
        isolate
            .eval_script(&script_source(&harness_text, case, strict))
            .map(drop)
            .map_err(|exception| exception.message)
            .and_then(|()| serve_imports(isolate))
    };
    match (&meta.negative, run) {
        (Some(negative), Err(message)) => host262::judge_negative(&negative.error_type, &message),
        (Some(negative), Ok(())) => Err(format!("expected {}, got success", negative.error_type)),
        (None, Err(message)) => Err(message),
        (None, Ok(())) if meta.is_async => check_async(isolate),
        (None, Ok(())) => Ok(()),
    }
}

/// Directive + harness + test body, exactly what a script test evaluates.
fn script_source(harness_text: &str, case: &TestCase, strict: bool) -> Source {
    let directive = if strict { "\"use strict\";\n" } else { "" };
    Source {
        name: case.abs_path.to_string_lossy().into_owned(),
        text: format!("{directive}{harness_text}{}", case.text),
    }
}

fn source(name: &str, text: String) -> Source {
    Source {
        name: name.to_owned(),
        text,
    }
}

fn check_async<I: Isolate>(isolate: &mut I) -> Result<(), String> {
    drain_microtasks(isolate);
    let lines = PRINTED.with_borrow(Clone::clone);
    if let Some(failure) = lines.iter().find_map(|l| l.strip_prefix(ASYNC_FAILURE)) {
        return Err(failure.to_owned());
    }
    if lines.iter().any(|line| line == ASYNC_COMPLETE) {
        Ok(())
    } else {
        Err("async test never called $DONE".to_owned())
    }
}

fn drain_microtasks<I: Isolate>(isolate: &mut I) {
    while isolate.perform_microtask_checkpoint() == MicrotaskProgress::Progressed {}
}

/// Harness as a script, then the test as an ES module; any failing stage yields its message.
fn run_module<I: Isolate>(
    isolate: &mut I,
    harness_text: &str,
    case: &TestCase,
) -> Result<(), String> {
    isolate
        .eval_script(&source("harness", harness_text.to_owned()))
        .map_err(|exception| format!("harness: {}", exception.message))?;
    let entry = Source {
        name: case.abs_path.to_string_lossy().into_owned(),
        text: case.text.clone(),
    };
    let module = isolate.compile_module(&entry).map_err(|e| e.message)?;
    isolate
        .instantiate_module(module, &mut FsHook)
        .map_err(|e| e.message)?;
    let promise = isolate.evaluate_module(module).map_err(|e| e.message)?;
    serve_imports(isolate)?;
    match isolate.promise_state(&promise) {
        Some(PromiseState::Fulfilled(_)) => Ok(()),
        Some(PromiseState::Rejected(reason)) => Err(describe(isolate, &reason)),
        _ => Err("module evaluation never settled".to_owned()),
    }
}

/// Drains microtasks and serves queued `import()` requests until both are quiet.
fn serve_imports<I: Isolate>(isolate: &mut I) -> Result<(), String> {
    loop {
        drain_microtasks(isolate);
        let requests = IMPORTS.with_borrow_mut(std::mem::take);
        if requests.is_empty() {
            return Ok(());
        }
        for (specifier, referrer, id) in requests {
            let resolver = ResolverId(id);
            match import_module(isolate, &specifier, &referrer) {
                Ok(name) => isolate.resolve(resolver, Value::String(name.as_str().into())),
                Err(reason) => {
                    let reason = rejection_value(isolate, reason);
                    isolate.reject(resolver, reason);
                }
            }
        }
    }
}

/// Compiles, links and evaluates the module; its name is what the engine settles the import from.
fn import_module<I: Isolate>(
    isolate: &mut I,
    specifier: &str,
    referrer: &str,
) -> Result<String, ImportError> {
    let source = FsHook.resolve(specifier, referrer).map_err(|err| {
        ImportError::Js(JsException {
            message: err.to_string(),
            stack: None,
            value: None,
        })
    })?;
    let module = isolate
        .compile_module(&source)
        .map_err(ImportError::Syntax)?;
    isolate
        .instantiate_module(module, &mut FsHook)
        .map_err(ImportError::Js)?;
    let promise = isolate.evaluate_module(module).map_err(ImportError::Js)?;
    drain_microtasks(isolate);
    match isolate.promise_state(&promise) {
        Some(PromiseState::Rejected(reason)) => Err(ImportError::Js(JsException {
            message: String::new(),
            stack: None,
            value: Some(reason),
        })),
        _ => Ok(source.name),
    }
}

enum ImportError {
    Syntax(JsException),
    Js(JsException),
}

fn rejection_value<I: Isolate>(isolate: &mut I, error: ImportError) -> Value {
    let (exception, constructor) = match error {
        ImportError::Syntax(e) => (e, "SyntaxError"),
        ImportError::Js(e) => (e, "Error"),
    };
    if let Some(value) = exception.value {
        return value;
    }
    let build = format!("(m => new {constructor}(m))");
    let message = Value::String(exception.message.as_str().into());
    isolate
        .eval_script(&source("import-error", build))
        .and_then(|func| isolate.call(&func, &Value::Undefined, std::slice::from_ref(&message)))
        .unwrap_or(message)
}

fn describe<I: Isolate>(isolate: &mut I, value: &Value) -> String {
    let rendered = isolate
        .eval_script(&source("describe", DESCRIBE.to_owned()))
        .and_then(|func| isolate.call(&func, &Value::Undefined, std::slice::from_ref(value)));
    match rendered {
        Ok(Value::String(text)) => text.to_string_lossy(),
        other => format!("unprintable rejection ({other:?})"),
    }
}

/// Resolves relative specifiers against the importing file; the normalized path is the module identity.
struct FsHook;

impl ModuleResolveHook for FsHook {
    fn resolve(&mut self, specifier: &str, referrer: &str) -> Result<Source, HostError> {
        let parent = Path::new(referrer).parent().unwrap_or(Path::new(""));
        let path = normalize(&parent.join(specifier));
        let text = std::fs::read_to_string(&path).map_err(|err| {
            HostError::Velyr(VelyrError::not_found(format!("{}: {err}", path.display())))
        })?;
        Ok(Source {
            name: path.to_string_lossy().into_owned(),
            text: if path.extension().is_some_and(|ext| ext == "json") {
                json_module_source(&text)
            } else {
                text
            },
        })
    }
}

/// A JSON module's default export is `ParseJSON(text)`; `JSON.parse` keeps `__proto__` as an own key
/// and throws the SyntaxError for invalid text when the module is evaluated.
fn json_module_source(text: &str) -> String {
    let literal = serde_json::to_string(text).expect("a string serializes");
    format!("export default JSON.parse({literal});")
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use velyr_engine_native::NativeEngine;
    use velyr_engine_v8::V8Engine;

    const TIMEOUT: Duration = Duration::from_millis(300);

    fn harness() -> Harness {
        Harness::from_files(&[
            (
                "assert.js",
                "var assert = { sameValue: function (a, b) { if (a !== b) throw new Test262Error('sameValue'); } };",
            ),
            (
                "sta.js",
                "function Test262Error(m) { this.message = m; } Test262Error.prototype.toString = function () { return 'Test262Error: ' + this.message; };",
            ),
            (
                "doneprintHandle.js",
                "function $DONE(e) { print(e ? 'Test262:AsyncTestFailure:' + e : 'Test262:AsyncTestComplete'); }",
            ),
            ("extra.js", "var extra = 42;"),
        ])
    }

    fn run(body: &str, frontmatter: &str) -> Outcome {
        let text = format!(
            "/*---
{frontmatter}
---*/
{body}"
        );
        let case = TestCase {
            abs_path: PathBuf::from("/virtual/test.js"),
            text,
        };
        run_test(&V8Engine::new(), &harness(), &case, TIMEOUT)
    }

    fn run_with_files(body: &str, frontmatter: &str, files: &[(&str, &str)]) -> [Outcome; 2] {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).expect("write fixture");
        }
        let text = format!("/*---\n{frontmatter}\n---*/\n{body}");
        let case = TestCase {
            abs_path: dir.path().join("test.js"),
            text,
        };
        [
            run_test(&V8Engine::new(), &harness(), &case, TIMEOUT),
            run_test(&NativeEngine, &harness(), &case, TIMEOUT),
        ]
    }

    #[test]
    fn dynamic_import_resolves_namespace() {
        let outcomes = run_with_files(
            "import('./dep_FIXTURE.js').then(ns => { assert.sameValue(ns.x, 1); }).then($DONE, $DONE);",
            "flags: [async]",
            &[("dep_FIXTURE.js", "export var x = 1;")],
        );
        assert_eq!(outcomes, [Outcome::Pass, Outcome::Pass]);
    }

    #[test]
    fn detach_array_buffer_is_available() {
        let body = "var b = new ArrayBuffer(8); $262.detachArrayBuffer(b); assert.sameValue(b.byteLength, 0);";
        assert_eq!(run(body, ""), Outcome::Pass);
    }

    #[test]
    fn passing_test() {
        assert_eq!(run("assert.sameValue(1 + 1, 2);", ""), Outcome::Pass);
    }

    #[test]
    fn failing_test_reports_thrown_message() {
        let Outcome::Fail(message) = run("assert.sameValue(1, 2);", "") else {
            panic!("expected fail");
        };
        assert_eq!(message, "Test262Error: sameValue");
    }

    #[test]
    fn includes_are_injected() {
        assert_eq!(
            run("assert.sameValue(extra, 42);", "includes: [extra.js]"),
            Outcome::Pass
        );
    }

    #[test]
    fn default_flags_run_strict_variant_too() {
        let body = "assert.sameValue((function () { return this; })(), undefined);";
        assert!(matches!(run(body, ""), Outcome::Fail(_)));
        assert_eq!(run(body, "flags: [onlyStrict]"), Outcome::Pass);
        assert!(matches!(run(body, "flags: [noStrict]"), Outcome::Fail(_)));
    }

    #[test]
    fn negative_syntax_error() {
        let fm = "negative:
  phase: parse
  type: SyntaxError";
        assert_eq!(run("var 1x;", fm), Outcome::Pass);
        assert!(matches!(run("var ok;", fm), Outcome::Fail(_)));
        assert!(matches!(
            run("throw new Test262Error('x');", fm),
            Outcome::Fail(_)
        ));
    }

    #[test]
    fn async_test_needs_done() {
        let fm = "flags: [async]";
        assert_eq!(
            run("Promise.resolve().then($DONE, $DONE);", fm),
            Outcome::Pass
        );
        assert!(matches!(
            run("Promise.reject('no').then($DONE, $DONE);", fm),
            Outcome::Fail(_)
        ));
        assert!(matches!(run("var never;", fm), Outcome::Fail(_)));
    }

    #[test]
    fn raw_test_has_no_harness() {
        assert_eq!(
            run(
                "if (typeof assert !== 'undefined') throw 1;",
                "flags: [raw]"
            ),
            Outcome::Pass
        );
    }

    #[test]
    fn infinite_loop_times_out() {
        assert_eq!(run("for (;;) {}", "flags: [noStrict]"), Outcome::Timeout);
    }

    #[test]
    fn unsupported_host_api_is_skipped() {
        assert!(matches!(run("$262.createRealm();", ""), Outcome::Skip(_)));
    }

    #[test]
    fn eval_script_runs_in_same_realm() {
        assert_eq!(
            run("$262.evalScript('var q = 3;'); assert.sameValue(q, 3);", ""),
            Outcome::Pass
        );
    }

    #[test]
    fn parse_negative_does_not_run_code() {
        let meta = "negative:\n  phase: parse\n  type: SyntaxError";
        assert!(matches!(
            run("$DONOTEVALUATE(); let a; let a;", meta),
            Outcome::Pass
        ));
        let case = TestCase {
            abs_path: PathBuf::from("/virtual/n.js"),
            text: format!("/*---\n{meta}\n---*/\nwith ({{}}) {{}}"),
        };
        let outcome = run_test(&NativeEngine, &harness(), &case, TIMEOUT);
        assert!(matches!(outcome, Outcome::Fail(_)), "{outcome:?}");
    }

    #[test]
    fn native_engine_runs_without_crashing() {
        let case = TestCase {
            abs_path: PathBuf::from("/virtual/n.js"),
            text: "/*---
flags: [raw]
---*/
1 + 1;"
                .to_owned(),
        };
        assert_eq!(
            run_test(&NativeEngine, &harness(), &case, TIMEOUT),
            Outcome::Pass
        );
    }

    #[test]
    fn normalize_collapses_dots() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d.js")),
            PathBuf::from("/a/c/d.js")
        );
    }
}
