//! Runs one WPT test file on a fresh Velyr isolate per variant and turns the testharness.js
//! completion callback into [`Unit`]s.
//!
//! The test file is the entry script. Before it runs, the isolate gets a shim (`self`, `GLOBAL`,
//! `location`), `resources/testharness.js`, a reporter and the `META: script=` files. The reporter
//! posts the final results through one host function; `done()` is scheduled on a timer so it runs
//! once the test file has finished loading, like the window `load` event.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{
    CallContext, Engine, HostError, HostFunctionDef, Isolate, JsException, Source, Value,
};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, RuntimeConfig, run_file};
use velyr_runtime_ops::BOOTSTRAP_JS;
use velyr_runtime_permissions::CapabilitySet;

use crate::env;
use crate::meta;
use crate::report::{Outcome, Unit, dedupe_names};

const CPU_BUDGET_MS: u64 = 10_000;
const WALL_TIMEOUT: Duration = Duration::from_secs(30);
const REPORT_FN: &str = "__velyr_wpt_report";

const SHIM: &str = "globalThis.self = globalThis;\n\
    globalThis.GLOBAL = { isWindow: () => false, isWorker: () => true, isShadowRealm: () => false };\n";

const REPORTER: &str = r#"(function (report) {
  add_completion_callback(function (tests, status) {
    report(JSON.stringify({
      status: status.status,
      message: status.message,
      tests: tests.map(function (t) { return { name: t.name, status: t.status, message: t.message }; }),
    }));
  });
  setup({ explicit_done: true });
  setTimeout(function () { done(); }, 0);
})(globalThis.__velyr_wpt_report);
delete globalThis.__velyr_wpt_report;
"#;

pub struct Runner {
    root: PathBuf,
    harness: String,
}

/// What the isolate hands back: the reporter's JSON and any setup failure.
#[derive(Default)]
struct Shared {
    report: Mutex<Option<String>>,
    setup_error: Mutex<Option<String>>,
}

struct Sink(Arc<Shared>);

fn op_report(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(json)) = args.first() else {
        return Err(ctx.throw_type_error("report expects a JSON string"));
    };
    let sink = ctx
        .isolate_slot::<Sink>()
        .ok_or_else(|| HostError::TypeError("no report sink installed".into()))?;
    *sink.0.report.lock().unwrap() = Some(json.to_string_lossy());
    Ok(Value::Undefined)
}

#[derive(Deserialize)]
struct Report {
    status: u8,
    message: Option<String>,
    tests: Vec<Subtest>,
}

#[derive(Deserialize)]
struct Subtest {
    name: String,
    status: u8,
    message: Option<String>,
}

impl Runner {
    pub fn load(root: &Path) -> io::Result<Self> {
        let harness = fs::read_to_string(root.join("resources/testharness.js"))?;
        Ok(Self {
            root: root.to_owned(),
            harness,
        })
    }

    /// Runs `relative` once per `META: variant`; a test that cannot run yields a single unit.
    pub fn run(&self, relative: &str) -> Vec<Unit> {
        let path = self.root.join(relative);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => return vec![unit(relative, Outcome::Crash(format!("read: {err}")))],
        };
        let meta = meta::parse(&text);
        if let Some(reason) = meta::skip_reason(relative, &text, &meta) {
            return vec![unit(relative, Outcome::Skip(reason.to_owned()))];
        }
        let scripts = match self.read_scripts(&path, &meta.scripts) {
            Ok(scripts) => scripts,
            Err(message) => return vec![unit(relative, Outcome::Fail(message))],
        };
        let variants = if meta.variants.is_empty() {
            vec![String::new()]
        } else {
            meta.variants
        };
        variants
            .iter()
            .flat_map(|variant| {
                let id = format!("{relative}{variant}");
                self.run_variant(&id, &path, relative, variant, &scripts)
            })
            .collect()
    }

    /// `META: script=` files: `/x` is relative to the WPT root, anything else to the test.
    fn read_scripts(&self, test: &Path, scripts: &[String]) -> Result<Vec<Source>, String> {
        let dir = test.parent().unwrap_or(&self.root);
        scripts
            .iter()
            .map(|script| {
                let path = match script.strip_prefix('/') {
                    Some(rooted) => self.root.join(rooted),
                    None => dir.join(script),
                };
                let text = fs::read_to_string(&path)
                    .map_err(|err| format!("META script {script}: {err}"))?;
                Ok(Source {
                    name: script.clone(),
                    text,
                })
            })
            .collect()
    }

    fn run_variant(
        &self,
        id: &str,
        path: &Path,
        relative: &str,
        variant: &str,
        scripts: &[Source],
    ) -> Vec<Unit> {
        let mut sources = vec![
            Source {
                name: "velyr-wpt:shim".into(),
                text: format!("{SHIM}{}", location(relative, variant)),
            },
            Source {
                name: "resources/testharness.js".into(),
                text: self.harness.clone(),
            },
            Source {
                name: "velyr-wpt:reporter".into(),
                text: REPORTER.into(),
            },
        ];
        sources.extend(scripts.iter().map(|s| Source {
            name: s.name.clone(),
            text: s.text.clone(),
        }));
        let shared = Arc::new(Shared::default());
        let result = execute(path, sources, &shared);
        let report = shared.report.lock().unwrap().take();
        let setup_error = shared.setup_error.lock().unwrap().take();
        let mut units = match report {
            Some(json) => units_from_report(id, &json),
            None => vec![unit(id, no_report_outcome(setup_error, result))],
        };
        dedupe_names(&mut units);
        units
    }
}

fn unit(test: &str, outcome: Outcome) -> Unit {
    Unit {
        test: test.to_owned(),
        subtest: None,
        outcome,
    }
}

fn location(relative: &str, variant: &str) -> String {
    let href = format!("http://web-platform.test/{relative}{variant}");
    format!(
        "globalThis.location = {{ search: {}, href: {}, pathname: {}, origin: 'http://web-platform.test' }};\n",
        json(variant),
        json(&href),
        json(&format!("/{relative}")),
    )
}

fn json(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

fn execute(path: &Path, sources: Vec<Source>, shared: &Arc<Shared>) -> Result<(), RunError> {
    let token = CancellationToken::new();
    let (stop, stopped) = mpsc::channel::<()>();
    let watchdog = {
        let token = token.clone();
        thread::spawn(move || {
            if stopped.recv_timeout(WALL_TIMEOUT) == Err(mpsc::RecvTimeoutError::Timeout) {
                token.cancel();
            }
        })
    };
    let config = RuntimeConfig {
        platform_init: Some(V8Engine::init_platform),
        capabilities: Some(CapabilitySet::builder().build()),
        cpu_budget_ms: Some(CPU_BUDGET_MS),
        shutdown: Some(token),
        ..RuntimeConfig::default()
    };
    let engine = V8Engine::new();
    let features = engine.features();
    let mut functions = env::host_functions(features);
    functions.push(HostFunctionDef {
        name: REPORT_FN,
        arity: 1,
        callback: op_report,
    });
    let shared = Arc::clone(shared);
    let install = move |isolate: &mut _, sender, timers| {
        env::install(isolate, sender, timers, features);
        isolate.set_slot(Sink(Arc::clone(&shared)));
        for source in &sources {
            if let Err(JsException { message, .. }) = isolate.eval_script(source) {
                *shared.setup_error.lock().unwrap() = Some(format!("{}: {message}", source.name));
                return;
            }
        }
    };
    let result = run_file(
        engine,
        config,
        functions,
        BOOTSTRAP_JS,
        install,
        path.to_owned(),
    );
    let _ = stop.send(());
    let _ = watchdog.join();
    result
}

fn no_report_outcome(setup_error: Option<String>, result: Result<(), RunError>) -> Outcome {
    if let Some(message) = setup_error {
        return Outcome::Fail(format!("setup: {message}"));
    }
    match result {
        Err(RunError::Terminated | RunError::QuotaExceeded { .. }) => Outcome::Timeout,
        Err(RunError::Setup(message)) => Outcome::Crash(message),
        Err(err) => Outcome::Fail(err.to_string()),
        Ok(()) => Outcome::Timeout,
    }
}

fn units_from_report(id: &str, json: &str) -> Vec<Unit> {
    let report: Report = match serde_json::from_str(json) {
        Ok(report) => report,
        Err(err) => return vec![unit(id, Outcome::Crash(format!("bad report: {err}")))],
    };
    let mut units: Vec<Unit> = report
        .tests
        .into_iter()
        .map(|test| Unit {
            test: id.to_owned(),
            subtest: Some(test.name),
            outcome: subtest_outcome(test.status, test.message),
        })
        .collect();
    match report.status {
        0 => {}
        2 => units.push(unit(id, Outcome::Timeout)),
        _ => units.push(unit(
            id,
            Outcome::Fail(format!(
                "harness: {}",
                report.message.unwrap_or_else(|| "error".into())
            )),
        )),
    }
    units
}

fn subtest_outcome(status: u8, message: Option<String>) -> Outcome {
    match status {
        0 => Outcome::Pass,
        2 => Outcome::Timeout,
        3 => Outcome::Fail("not run".into()),
        _ => Outcome::Fail(message.unwrap_or_else(|| "failed".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Just enough testharness.js for the plumbing: `test`, completion callbacks and `done`.
    const STUB_HARNESS: &str = r#"
(function () {
  const tests = [];
  const callbacks = [];
  globalThis.setup = function () {};
  globalThis.test = function (fn, name) {
    try { fn(); tests.push({ name, status: 0, message: null }); }
    catch (e) { tests.push({ name, status: 1, message: String(e) }); }
  };
  globalThis.add_completion_callback = (cb) => callbacks.push(cb);
  globalThis.done = function () { callbacks.forEach((cb) => cb(tests, { status: 0, message: null })); };
})();
"#;

    fn tree(files: &[(&str, &str)]) -> (tempfile::TempDir, Runner) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(root.join("resources")).unwrap();
        fs::write(root.join("resources/testharness.js"), STUB_HARNESS).unwrap();
        for (name, text) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let runner = Runner::load(&root).unwrap();
        (dir, runner)
    }

    fn summary(units: &[Unit]) -> Vec<(String, Outcome)> {
        units.iter().map(|u| (u.key(), u.outcome.clone())).collect()
    }

    #[test]
    fn collects_subtests_and_loads_meta_scripts() {
        let (_dir, runner) = tree(&[
            ("common/rooted.js", "globalThis.rooted = 1;"),
            ("t/near.js", "globalThis.near = 2;"),
            (
                "t/a.any.js",
                "// META: script=/common/rooted.js\n// META: script=near.js\n\
                 test(() => {}, 'passes');\n\
                 test(() => { if (rooted + near !== 3 || self !== globalThis) throw new Error('shim'); }, 'meta and shim');\n\
                 test(() => { throw new Error('boom'); }, 'fails');",
            ),
        ]);
        let units = runner.run("t/a.any.js");
        let got = summary(&units);
        assert_eq!(got[0], ("t/a.any.js::passes".into(), Outcome::Pass));
        assert_eq!(got[1], ("t/a.any.js::meta and shim".into(), Outcome::Pass));
        assert!(
            matches!(&got[2], (key, Outcome::Fail(m)) if key == "t/a.any.js::fails" && m.contains("boom"))
        );
    }

    #[test]
    fn each_variant_runs_with_its_query_string() {
        let (_dir, runner) = tree(&[(
            "t/v.any.js",
            "// META: variant=?one\n// META: variant=?two\ntest(() => {}, location.search);",
        )]);
        let keys: Vec<_> = runner.run("t/v.any.js").iter().map(Unit::key).collect();
        assert_eq!(keys, ["t/v.any.js?one::?one", "t/v.any.js?two::?two"]);
    }

    #[test]
    fn uncaught_error_and_missing_script_fail_the_file() {
        let (_dir, runner) = tree(&[
            ("t/throws.any.js", "throw new Error('top level');"),
            (
                "t/missing.any.js",
                "// META: script=nope.js\ntest(() => {}, 'x');",
            ),
        ]);
        let thrown = runner.run("t/throws.any.js");
        assert!(matches!(&thrown[0].outcome, Outcome::Fail(m) if m.contains("top level")));
        let missing = runner.run("t/missing.any.js");
        assert!(matches!(&missing[0].outcome, Outcome::Fail(m) if m.contains("nope.js")));
    }

    #[test]
    fn server_dependent_test_is_skipped_with_a_reason() {
        let (_dir, runner) = tree(&[("t/x.sub.any.js", "test(() => {}, 'x');")]);
        let units = runner.run("t/x.sub.any.js");
        assert!(matches!(&units[0].outcome, Outcome::Skip(_)));
        assert_eq!(units[0].subtest, None);
    }

    #[test]
    fn a_hanging_loop_is_stopped_as_timeout() {
        let (_dir, runner) = tree(&[("t/spin.any.js", "while (true) {}")]);
        assert_eq!(runner.run("t/spin.any.js")[0].outcome, Outcome::Timeout);
    }
}
