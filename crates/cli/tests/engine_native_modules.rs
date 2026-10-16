//! `velyr run --engine native` with local ES module imports (VELYR-110).
#![cfg(feature = "engine-native")]

use std::process::Command;

#[test]
fn native_engine_runs_a_module_graph() {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        (
            "main.mjs",
            "import { add } from './math.mjs';
             import greet, * as ns from './greet.mjs';
             console.log(greet('native'), add(2, 3), Object.keys(ns).join(','));",
        ),
        ("math.mjs", "export function add(a, b) { return a + b; }"),
        (
            "greet.mjs",
            "export default (who) => `hello ${who}`;
             export const extra = await Promise.resolve(1);",
        ),
    ];
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args(["run", "--engine", "native"])
        .arg(dir.path().join("main.mjs"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "hello native 5 default,extra\n");
}

#[test]
fn native_engine_reports_a_missing_import() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main.mjs");
    std::fs::write(&main, "import './absent.mjs';").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args(["run", "--engine", "native"])
        .arg(&main)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("absent.mjs"), "{stderr}");
}
