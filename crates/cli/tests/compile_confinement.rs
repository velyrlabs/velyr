//! `velyr compile` embeds only what lives under the entry's folder (or a parent `node_modules`),
//! unless `--allow-outside-root` says otherwise.

use std::fs;
use std::path::Path;

use assert_cmd::Command;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn compile(dir: &Path, extra: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.join("app"))
        .args(["compile", "main.ts", "-o", "out"])
        .args(extra)
        .assert()
}

fn secret_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "outside/key.txt", "TOPSECRET_OUTSIDE_FILE");
    write(dir.path(), "app/main.ts", "import '../outside/key.txt';\n");
    dir
}

#[test]
fn import_of_a_file_outside_the_root_fails_and_embeds_nothing() {
    let dir = secret_project();
    let out = compile(dir.path(), &[]).failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(stderr.contains("outside the project root"), "{stderr}");
    assert!(!dir.path().join("app/out").exists());
}

#[test]
fn allow_outside_root_embeds_the_file() {
    let dir = secret_project();
    compile(dir.path(), &["--allow-outside-root"]).success();
    let bytes = fs::read(dir.path().join("app/out")).unwrap();
    let needle = b"TOPSECRET_OUTSIDE_FILE";
    assert!(bytes.windows(needle.len()).any(|w| w == needle));
}

#[test]
fn parent_node_modules_package_is_still_embedded() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "node_modules/dep/index.js",
        "export const d = 1;",
    );
    write(
        dir.path(),
        "node_modules/dep/package.json",
        r#"{"name":"dep","main":"index.js"}"#,
    );
    write(
        dir.path(),
        "app/main.ts",
        "import { d } from 'dep';\nconsole.log(d);\n",
    );
    compile(dir.path(), &[]).success();
}
