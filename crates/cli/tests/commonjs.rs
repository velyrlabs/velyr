//! CommonJS under `velyr run`: `.cjs` entries, `require` of relative files and JSON, and ESM
//! importing CommonJS with named exports. Only the node and full profiles have `require`.
#![cfg(feature = "node")]

use std::path::Path;

use assert_cmd::Command;

fn run(name: &str) -> assert_cmd::assert::Assert {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/commonjs");
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir)
        .args(["run", name])
        .assert()
}

#[test]
fn a_cjs_entry_requires_relative_files_and_json() {
    run("main.cjs")
        .success()
        .stdout("5 fixture object string\n");
}

#[test]
fn esm_imports_commonjs_with_named_exports() {
    run("main.mjs").success().stdout("1 b alpha,beta\n");
}

#[test]
fn typescript_imports_commonjs_too() {
    run("typed.ts").success().stdout("2\n");
}
