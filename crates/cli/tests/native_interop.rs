//! `velyr run --engine native` gets the CommonJS loader and the `URL` global (VELYR-194) and
//! the same profile globals as V8 (VELYR-168).
#![cfg(all(feature = "engine-native", feature = "node"))]

use std::process::{Command, Output};

fn run(engine: &str, script: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args(["run", "--engine", engine])
        .arg(script)
        .output()
        .unwrap()
}

#[test]
fn native_engine_matches_v8_on_timers_url_and_denied_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("main.mjs");
    std::fs::write(
        &script,
        "const order = [];
         setTimeout(() => order.push('timeout'), 0);
         Promise.resolve().then(() => order.push('micro'));
         const url = new URL('../b?q=1#h', 'https://example.com/a/c');
         let fetched;
         try { await fetch('https://example.com/'); fetched = 'allowed'; }
         catch (error) { fetched = `${error.name} ${error.code}: ${error.message}`; }
         await new Promise((resolve) => setTimeout(resolve, 5));
         console.log(order.join(), url.href, typeof clearTimeout, fetched);",
    )
    .unwrap();
    let v8 = run("v8", &script);
    let native = run("native", &script);
    let stderr = String::from_utf8_lossy(&native.stderr);
    assert!(native.status.success(), "{stderr}");
    let v8_stdout = String::from_utf8_lossy(&v8.stdout);
    assert!(v8_stdout.starts_with("micro,timeout https://example.com/b?q=1#h function "));
    assert!(v8_stdout.contains("access denied"), "{v8_stdout}");
    assert_eq!(String::from_utf8_lossy(&native.stdout), v8_stdout);
}

#[test]
fn native_engine_imports_commonjs_and_has_url() {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        (
            "main.mjs",
            "import lib, { twice } from './lib.cjs';
             const url = new URL('../b?q=1#h', 'https://example.com/a/c');
             console.log(lib.name, twice(21), url.href, url.searchParams.get('q'));",
        ),
        (
            "lib.cjs",
            "const { double } = require('./math.cjs');
             exports.name = 'lib';
             exports.twice = (n) => double(n);",
        ),
        ("math.cjs", "module.exports = { double: (n) => n * 2 };"),
    ];
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_velyr"))
        .args(["run", "-A", "--engine", "native"])
        .arg(dir.path().join("main.mjs"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "lib 42 https://example.com/b?q=1#h 1\n");
}
