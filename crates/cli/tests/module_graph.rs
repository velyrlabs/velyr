//! Characterization net for module loading: pins resolution, caching and interop behavior that
//! the loader performance work must not change. Each test runs `velyr` on a temp copy of a fixture.
#![cfg(feature = "node")]

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn fixture(name: &str) -> TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/module-graph")
        .join(name);
    let tmp = TempDir::new().unwrap();
    copy_dir(&src, tmp.path());
    tmp
}

fn run_in(dir: &Path, script: &str) -> assert_cmd::assert::Assert {
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir)
        .args(["run", "--allow-read=.", "--allow-write=.", script])
        .assert()
}

fn stdout_of(dir: &Path, script: &str) -> String {
    let out = run_in(dir, script).success().get_output().stdout.clone();
    String::from_utf8(out).unwrap()
}

#[test]
fn diamond_loads_each_module_once() {
    let dir = fixture("esm-diamond");
    assert_eq!(stdout_of(dir.path(), "main.mjs"), "bd cd d\n");
}

#[test]
fn cjs_cycle_sees_partial_exports() {
    let dir = fixture("cjs-cycle");
    assert_eq!(stdout_of(dir.path(), "main.cjs"), "{\"early\":1} true\n");
}

#[test]
fn esm_imports_cjs_default_and_named() {
    let dir = fixture("mixed");
    assert_eq!(stdout_of(dir.path(), "main.mjs"), "2 1\n");
}

#[test]
fn require_of_esm_returns_namespace() {
    let dir = fixture("mixed");
    assert_eq!(
        stdout_of(dir.path(), "require_esm.cjs"),
        "default,x 7 dflt v\n"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_import_has_realpath_import_meta_url() {
    let dir = fixture("symlink");
    std::os::unix::fs::symlink("real.mjs", dir.path().join("link.mjs")).unwrap();
    fs::write(dir.path().join("main.mjs"), "import \"./link.mjs\";\n").unwrap();
    let url = stdout_of(dir.path(), "main.mjs");
    assert!(url.trim_end().ends_with("/real.mjs"), "got {url}");
}

#[test]
fn package_exports_condition_import() {
    let dir = fixture("pkg-exports");
    assert_eq!(stdout_of(dir.path(), "main.mjs"), "esm cjs\n");
}

#[test]
fn package_exports_condition_require() {
    let dir = fixture("pkg-exports");
    assert_eq!(stdout_of(dir.path(), "main.cjs"), "cjs\n");
}

#[test]
fn package_main_is_used_without_exports() {
    let dir = fixture("pkg-exports");
    let pkg = dir.path().join("node_modules/p/package.json");
    fs::write(pkg, r#"{"name":"p","main":"./main.cjs"}"#).unwrap();
    assert_eq!(stdout_of(dir.path(), "main.cjs"), "main\n");
}

const SECOND_LOAD_SCRIPT: &str = r#"
import { writeFileSync, rmSync, symlinkSync } from "node:fs";
const first = (await import("./a.mjs")).default;
SWAP;
const second = (await import("./b.mjs")).default;
console.log(first, second);
"#;

fn write_importer(dir: &Path, name: &str) {
    fs::write(dir.join(name), "import p from \"p\";\nexport default p;\n").unwrap();
}

#[test]
fn rewritten_package_json_is_observed_by_a_second_graph_load() {
    let dir = fixture("pkg-exports");
    let pkg = dir.path().join("node_modules/p");
    fs::write(
        pkg.join("package.json"),
        r#"{"name":"p","main":"./one.mjs"}"#,
    )
    .unwrap();
    fs::write(pkg.join("one.mjs"), "export default 'one';\n").unwrap();
    fs::write(pkg.join("two.mjs"), "export default 'two';\n").unwrap();
    write_importer(dir.path(), "a.mjs");
    write_importer(dir.path(), "b.mjs");
    let swap = format!(
        "writeFileSync({:?}, '{{\"name\":\"p\",\"main\":\"./two.mjs\"}}')",
        pkg.join("package.json").to_str().unwrap()
    );
    fs::write(
        dir.path().join("run.mjs"),
        SECOND_LOAD_SCRIPT.replace("SWAP", &swap),
    )
    .unwrap();
    assert_eq!(stdout_of(dir.path(), "run.mjs"), "one two\n");
}

#[cfg(unix)]
#[test]
fn symlink_retarget_between_dynamic_imports_is_observed() {
    let dir = fixture("pkg-exports");
    let root = dir.path();
    let modules = root.join("node_modules");
    for (v, d) in [("one", "d1"), ("two", "d2")] {
        fs::create_dir_all(modules.join(d)).unwrap();
        fs::write(
            modules.join(d).join("package.json"),
            r#"{"name":"p","main":"./i.mjs"}"#,
        )
        .unwrap();
        fs::write(
            modules.join(d).join("i.mjs"),
            format!("export default '{v}';\n"),
        )
        .unwrap();
    }
    fs::remove_dir_all(modules.join("p")).unwrap();
    std::os::unix::fs::symlink("d1", modules.join("p")).unwrap();
    write_importer(root, "a.mjs");
    write_importer(root, "b.mjs");
    let link = modules.join("p");
    let swap = format!(
        "rmSync({l:?}); symlinkSync('d2', {l:?})",
        l = link.to_str().unwrap()
    );
    fs::write(
        root.join("run.mjs"),
        SECOND_LOAD_SCRIPT.replace("SWAP", &swap),
    )
    .unwrap();
    assert_eq!(stdout_of(root, "run.mjs"), "one two\n");
}
