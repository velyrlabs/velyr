//! Node-API addons under `velyr run`: `require('./x.node')` and `process.dlopen` load the C test
//! addon of `velyr-compat-napi`, and without `--allow-native` nothing is ever `dlopen`ed.
// Unix only: the C addon leaves `napi_*` undefined for the host executable to provide, which a
// Windows DLL cannot do without an import library for the exe.
#![cfg(all(feature = "node", unix))]

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::OnceLock;

use assert_cmd::Command;

/// Printed by the probe library's static constructor, which runs only if it is `dlopen`ed.
const PROBE_MARKER: &str = "probe constructor ran";
const PROBE_SOURCE: &str = r#"
#include <stdio.h>
__attribute__((constructor)) static void loaded(void) { fputs("probe constructor ran\n", stderr); }
"#;

/// A directory with `addon.node` (the compat-napi test addon) and `probe.node`, built once.
fn fixtures() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("napi-cli");
        std::fs::create_dir_all(&dir).unwrap();
        let addon_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../compat-napi/tests/addons");
        let mut sources: Vec<PathBuf> = std::fs::read_dir(&addon_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "c"))
            .collect();
        sources.sort();
        compile(
            &sources,
            &addon_dir.join("include"),
            &dir.join("addon.node"),
        );
        let probe = dir.join("probe.c");
        std::fs::write(&probe, PROBE_SOURCE).unwrap();
        compile(
            &[probe],
            &addon_dir.join("include"),
            &dir.join("probe.node"),
        );
        dunce::canonicalize(dir).unwrap()
    })
}

fn compile(sources: &[PathBuf], include: &Path, out: &Path) {
    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let mut cmd = Process::new(compiler);
    cmd.args(["-shared", "-fPIC", "-std=c11", "-Wall", "-Werror", "-I"])
        .arg(include);
    if cfg!(target_os = "macos") {
        // Resolve `napi_*` in the host executable at load time, as node-gyp does.
        cmd.args(["-undefined", "dynamic_lookup"]);
    }
    let status = cmd.args(sources).arg("-o").arg(out).status().unwrap();
    assert!(status.success(), "compiling {} failed", out.display());
}

fn run(script: &str, code: &str, flags: &[&str]) -> assert_cmd::assert::Assert {
    let dir = fixtures();
    std::fs::write(dir.join(script), code).unwrap();
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir)
        .arg("run")
        .args(flags)
        .arg(script)
        .assert()
}

#[test]
fn require_loads_the_addon_with_allow_native() {
    run(
        "require.cjs",
        "const addon = require('./addon.node');\n\
         console.log(addon.getInstanceData(), addon.getVersion(), addon.makeAdder(2)(3));\n\
         console.log(require('./addon.node') === addon);\n",
        &["--allow-native"],
    )
    .success()
    .stdout("42 8 5\ntrue\n");
}

#[test]
fn process_dlopen_sets_module_exports() {
    run(
        "dlopen.cjs",
        "const m = { exports: {} };\n\
         process.dlopen(m, __dirname + '/addon.node');\n\
         console.log(m.exports.makeAdder(1)(1));\n",
        &["--allow-native"],
    )
    .success()
    .stdout("2\n");
}

#[test]
fn without_allow_native_the_library_is_never_opened() {
    let script = "try { require('./probe.node'); } catch (e) { console.log(e.code); }\n";
    let denied = run("probe.cjs", script, &[]).success();
    let output = denied.get_output();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "VELYR_PERMISSION_DENIED\n"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains(PROBE_MARKER));

    // The probe does run its constructor once allowed (it has no registration, so require fails).
    let allowed = run("probe.cjs", script, &["--allow-native"]).success();
    assert!(String::from_utf8_lossy(&allowed.get_output().stderr).contains(PROBE_MARKER));
}
