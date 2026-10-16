//! `velyr compat [dir]`: static compatibility report from package.json and a scan of the sources.

use std::fs;
use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use serde_json::Value;

fn velyr(dir: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir)
        .env("HOME", dir)
        .args(args)
        .output()
        .unwrap()
}

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in files {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    dir
}

fn report(dir: &Path) -> Value {
    let out = velyr(dir, &["compat", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn entry<'a>(report: &'a Value, list: &str, name: &str) -> &'a Value {
    report[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("{name} missing from {list}: {report}"))
}

#[test]
fn supported_node_module_import_is_reported_with_its_file() {
    let dir = project(&[(
        "src/a.mjs",
        "import fs from 'node:fs';\nimport { join } from 'path';",
    )]);
    let json = report(dir.path());
    let fs_module = entry(&json, "modules", "fs");
    assert_eq!(fs_module["status"], "experimental");
    assert_eq!(fs_module["files"][0], "src/a.mjs");
    assert_eq!(entry(&json, "modules", "path")["status"], "experimental");
}

#[test]
fn require_dynamic_import_and_typescript_are_scanned() {
    let dir = project(&[
        ("a.cjs", "const os = require('node:os');"),
        (
            "b.ts",
            "const z: number = 1; export const m = import('node:zlib');",
        ),
    ]);
    let json = report(dir.path());
    entry(&json, "modules", "os");
    entry(&json, "modules", "zlib");
}

#[test]
fn unsupported_module_is_flagged_and_node_modules_is_skipped() {
    let dir = project(&[
        ("a.js", "import dns from 'node:cluster';"),
        ("node_modules/dep/index.js", "require('node:inspector');"),
    ]);
    let json = report(dir.path());
    assert_eq!(entry(&json, "modules", "cluster")["status"], "unsupported");
    assert!(
        json["modules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["name"] != "inspector")
    );
}

#[test]
fn dependencies_use_the_matrix_and_native_addons_suggest_allow_native() {
    let dir = project(&[(
        "package.json",
        r#"{"dependencies":{"express":"^5.0.0","sharp":"1.0.0","left-pad":"1.0.0"},"devDependencies":{"pg":"8"}}"#,
    )]);
    let json = report(dir.path());
    assert_eq!(
        entry(&json, "packages", "express")["status"],
        "experimental"
    );
    assert_eq!(
        entry(&json, "packages", "pg")["status"],
        "partial (no server)"
    );
    assert_eq!(entry(&json, "packages", "left-pad")["status"], "unknown");
    let sharp = entry(&json, "packages", "sharp");
    assert_eq!(sharp["native"], true);
    assert!(json["suggestions"].to_string().contains("--allow-native"));
}

#[test]
fn json_shape_and_exit_code_on_empty_project() {
    let dir = project(&[]);
    let json = report(dir.path());
    for key in ["dir", "packages", "modules", "suggestions"] {
        assert!(json.get(key).is_some(), "{key}");
    }
}

#[test]
fn pretty_output_lists_modules_and_packages() {
    let dir = project(&[
        ("a.js", "import 'node:cluster'; import 'node:fs';"),
        ("package.json", r#"{"dependencies":{"express":"5"}}"#),
    ]);
    let out = velyr(dir.path(), &["compat"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("node:cluster") && text.contains("unsupported"),
        "{text}"
    );
    assert!(text.contains("express"), "{text}");
}

#[test]
fn invalid_directory_exits_non_zero() {
    let dir = project(&[]);
    let out = velyr(dir.path(), &["compat", "does-not-exist"]);
    assert!(!out.status.success());
}
