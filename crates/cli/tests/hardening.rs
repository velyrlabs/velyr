//! Opt-in hardening flags: `--disallow-code-generation-from-strings` and `--frozen-intrinsics`.

use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;

fn run(flags: &[&str], file: &Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("velyr")
        .unwrap()
        .timeout(Duration::from_secs(30))
        .args(flags)
        .arg("run")
        .arg(file)
        .assert()
}

fn script(dir: &tempfile::TempDir, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, text).unwrap();
    path
}

const EVAL_PROBE: &str =
    r#"try { console.log(eval("1 + 1")); } catch (e) { console.log(e.name); }"#;

#[test]
fn code_generation_from_strings_is_allowed_by_default() {
    let dir = tempfile::tempdir().unwrap();
    run(&[], &script(&dir, "eval.js", EVAL_PROBE))
        .success()
        .stdout("2\n");
}

#[test]
fn codegen_flag_blocks_eval() {
    let dir = tempfile::tempdir().unwrap();
    let file = script(&dir, "eval.js", EVAL_PROBE);
    run(&["--disallow-code-generation-from-strings"], &file)
        .success()
        .stdout("EvalError\n");
}

#[test]
fn frozen_intrinsics_blocks_prototype_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let file = script(
        &dir,
        "freeze.js",
        r#""use strict";
try { Object.prototype.polluted = 1; console.log("mutated"); } catch (e) { console.log(e.name); }"#,
    );
    run(&[], &file).success().stdout("mutated\n");
    run(&["--frozen-intrinsics"], &file)
        .success()
        .stdout("TypeError\n");
}

#[test]
fn cjs_graph_runs_with_codegen_disabled() {
    let main = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/module-graph/cjs-cycle/main.cjs");
    run(&["--disallow-code-generation-from-strings"], &main)
        .success()
        .stdout("{\"early\":1} true\n");
}

#[test]
fn worker_inherits_the_codegen_flag() {
    let dir = tempfile::tempdir().unwrap();
    script(
        &dir,
        "worker.js",
        "try { eval('1'); console.log('ran'); } catch (e) { console.log(e.name); }",
    );
    let main = script(
        &dir,
        "main.mjs",
        r#"const worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
setTimeout(() => worker.terminate(), 500);"#,
    );
    run(&["--disallow-code-generation-from-strings"], &main)
        .success()
        .stdout("EvalError\n");
}
