use std::process::Command as Std;

use assert_cmd::Command;

fn host_triple() -> String {
    let out = Std::new("rustc").arg("-vV").output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_owned))
        .unwrap()
}

// Unix only: the embedded payload addresses files by POSIX path (`/$velyr/app/...`), which the Windows runtime cannot resolve yet.
#[cfg(unix)]
#[test]
fn compiled_executable_runs_without_sources_and_reads_assets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/greeting.txt"), "hello asset").unwrap();
    std::fs::write(dir.path().join("dep.ts"), "export const n: number = 41;").unwrap();
    std::fs::write(
        dir.path().join("main.ts"),
        "import { n } from './dep.ts';\nconsole.log('sum', n + 1);\n",
    )
    .unwrap();

    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .args(["compile", "main.ts", "-o", "app", "--include", "assets/**"])
        .assert()
        .success();

    // Sources are gone: the executable must be self-contained.
    std::fs::remove_file(dir.path().join("main.ts")).unwrap();
    std::fs::remove_file(dir.path().join("dep.ts")).unwrap();

    Command::new(dir.path().join("app"))
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout("sum 42\n");
}

// Unix only: the embedded payload addresses files by POSIX path (`/$velyr/app/...`), which the Windows runtime cannot resolve yet.
#[cfg(unix)]
#[test]
fn compile_bakes_permissions_and_rejects_allow_flags_at_runtime() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.js"),
        "console.log(typeof process.env.HOME);",
    )
    .unwrap();

    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .args(["compile", "main.js", "-o", "app", "--allow-env=HOME"])
        .assert()
        .success();

    Command::new(dir.path().join("app"))
        .assert()
        .success()
        .stdout("string\n");
    Command::new(dir.path().join("app"))
        .arg("--allow-all")
        .assert()
        .failure();
}

#[test]
fn tampered_payload_is_never_executed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), "console.log('ran');").unwrap();
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .args(["compile", "main.js", "-o", "app"])
        .assert()
        .success();

    // The trailer is 48 bytes (hash, length, magic); the byte before it is payload.
    let mut bytes = std::fs::read(dir.path().join("app")).unwrap();
    let at = bytes.len() - 49;
    bytes[at] ^= 0xff;
    std::fs::write(dir.path().join("app"), bytes).unwrap();

    let out = Command::new(dir.path().join("app"))
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("hash mismatch"));
}

#[test]
fn foreign_target_is_not_supported_yet() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.js"), "").unwrap();
    assert_ne!(host_triple(), "riscv64gc-unknown-linux-gnu");

    let out = Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "compile",
            "main.js",
            "-o",
            "app",
            "--target",
            "riscv64gc-unknown-linux-gnu",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&out.stderr).contains("not supported yet"));
}

/// `node_modules/cjs-dep` (CommonJS, requires a sibling file and a JSON file) next to the app.
#[cfg(unix)]
fn write_cjs_dependency(root: &std::path::Path) {
    let lib = root.join("node_modules/cjs-dep/lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(
        root.join("node_modules/cjs-dep/package.json"),
        r#"{"name":"cjs-dep","version":"1.0.0","main":"lib/index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        lib.join("index.js"),
        "const data = require('./data.json');\nconst util = require('./util');\nmodule.exports = { greet: (n) => `hello ${n} ${data.v}${util.mark}` };\n",
    )
    .unwrap();
    std::fs::write(lib.join("data.json"), r#"{"v":7}"#).unwrap();
    std::fs::write(lib.join("util.js"), "exports.mark = '!';\n").unwrap();
}

#[cfg(unix)]
fn compile_then_remove_sources(dir: &std::path::Path, entry: &str, extra: &[&str]) {
    let mut args = vec!["compile", entry, "-o", "app"];
    args.extend_from_slice(extra);
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir)
        .args(args)
        .assert()
        .success();
    for name in ["node_modules", "assets", "config.json", entry] {
        let path = dir.join(name);
        if path.is_dir() {
            std::fs::remove_dir_all(path).unwrap();
        } else if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }
}

// Unix only: the embedded payload addresses files by POSIX path (`/$velyr/app/...`), which the Windows runtime cannot resolve yet.
#[cfg(unix)]
#[test]
fn commonjs_entry_with_npm_dependency_and_json_runs_without_sources() {
    let dir = tempfile::tempdir().unwrap();
    write_cjs_dependency(dir.path());
    std::fs::write(dir.path().join("config.json"), r#"{"name":"cfg"}"#).unwrap();
    std::fs::write(
        dir.path().join("main.js"),
        "const dep = require('cjs-dep');\nconst cfg = require('./config.json');\nconsole.log(dep.greet('cjs'), cfg.name);\n",
    )
    .unwrap();

    compile_then_remove_sources(dir.path(), "main.js", &[]);

    Command::new(dir.path().join("app"))
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout("hello cjs 7! cfg\n");
}

// Unix only: the embedded payload addresses files by POSIX path (`/$velyr/app/...`), which the Windows runtime cannot resolve yet.
#[cfg(unix)]
#[test]
fn esm_entry_importing_commonjs_dependency_and_json_runs_without_sources() {
    let dir = tempfile::tempdir().unwrap();
    write_cjs_dependency(dir.path());
    std::fs::write(dir.path().join("config.json"), r#"{"name":"cfg"}"#).unwrap();
    std::fs::write(
        dir.path().join("main.mjs"),
        "import dep from 'cjs-dep';\nimport { createRequire } from 'node:module';\nconst cfg = createRequire(import.meta.url)('./config.json');\nconsole.log(dep.greet('esm'), cfg.name);\n",
    )
    .unwrap();

    compile_then_remove_sources(dir.path(), "main.mjs", &[]);

    Command::new(dir.path().join("app"))
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout("hello esm 7! cfg\n");
}

// Unix only: the embedded payload addresses files by POSIX path (`/$velyr/app/...`), which the Windows runtime cannot resolve yet.
#[cfg(unix)]
#[test]
fn included_asset_is_read_from_the_payload_with_velyr_read_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/greeting.txt"), "hello asset").unwrap();
    std::fs::write(
        dir.path().join("main.mjs"),
        "const url = new URL('./assets/greeting.txt', import.meta.url);\nconsole.log(await Velyr.readTextFile(url));\nconsole.log((await Velyr.readFile(url)).length);\n",
    )
    .unwrap();

    compile_then_remove_sources(dir.path(), "main.mjs", &["--include", "assets/**"]);

    Command::new(dir.path().join("app"))
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout("hello asset\n11\n");
}

#[cfg(unix)]
#[test]
fn include_with_a_directory_symlink_loop_terminates_and_keeps_the_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/greeting.txt"), "hello asset").unwrap();
    // Two loops branch the walk at every level: following both is exponential.
    std::os::unix::fs::symlink("../assets", dir.path().join("assets/loop_a")).unwrap();
    std::os::unix::fs::symlink("../assets", dir.path().join("assets/loop_b")).unwrap();
    std::fs::write(
        dir.path().join("main.mjs"),
        "console.log(await Velyr.readTextFile(new URL('./assets/greeting.txt', import.meta.url)));\n",
    )
    .unwrap();

    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .timeout(std::time::Duration::from_secs(60))
        .args(["compile", "main.mjs", "-o", "app", "--include", "assets"])
        .assert()
        .success();
    std::fs::remove_dir_all(dir.path().join("assets")).unwrap();

    Command::new(dir.path().join("app"))
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout("hello asset\n");
}

// Unix only: the embedded payload addresses files by POSIX path (`/$velyr/app/...`), which the Windows runtime cannot resolve yet.
#[cfg(unix)]
#[test]
fn commonjs_require_of_an_esm_package_runs_without_sources() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("node_modules/esm-dep");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        r#"{"name":"esm-dep","type":"module","exports":"./index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        pkg.join("index.js"),
        "import { n } from './n.js';\nexport const v = n + 1;\n",
    )
    .unwrap();
    std::fs::write(pkg.join("n.js"), "export const n = 41;\n").unwrap();
    std::fs::write(
        dir.path().join("main.js"),
        "console.log(require('esm-dep').v);\n",
    )
    .unwrap();

    compile_then_remove_sources(dir.path(), "main.js", &[]);

    Command::new(dir.path().join("app"))
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout("42\n");
}

// Unix only: the embedded payload addresses files by POSIX path (`/$velyr/app/...`), which the Windows runtime cannot resolve yet.
#[cfg(unix)]
#[test]
fn included_asset_is_read_from_the_payload_with_fs_read_file_sync() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/greeting.txt"), "hello asset").unwrap();
    std::fs::write(
        dir.path().join("main.js"),
        "const fs = require('node:fs');\nconst path = require('node:path');\nconsole.log(fs.readFileSync(path.join(__dirname, 'assets/greeting.txt'), 'utf8'));\n",
    )
    .unwrap();

    compile_then_remove_sources(dir.path(), "main.js", &["--include", "assets/**"]);

    Command::new(dir.path().join("app"))
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout("hello asset\n");
}

// Unix only: the embedded payload addresses files by POSIX path (`/$velyr/app/...`), which the Windows runtime cannot resolve yet.
#[cfg(unix)]
#[test]
fn fs_read_file_sync_keeps_enoent_inside_the_payload_and_denies_outside_the_app_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.js"),
        "const fs = require('node:fs');\nfor (const file of ['/$velyr/app/missing.txt', '/$velyr/other/secret.txt']) {\n  try { fs.readFileSync(file); console.log('read'); } catch (e) { console.log(e.code); }\n}\n",
    )
    .unwrap();

    compile_then_remove_sources(dir.path(), "main.js", &[]);

    let out = Command::new(dir.path().join("app"))
        .current_dir(dir.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let codes: Vec<&str> = stdout.lines().collect();
    assert_eq!(codes[0], "ENOENT");
    assert_ne!(codes[1], "ENOENT");
    assert_ne!(codes[1], "read");
}
