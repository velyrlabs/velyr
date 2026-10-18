use std::collections::BTreeSet;
use std::path::Path;

use super::*;

fn package(project: &Path, name: &str, scripts: &str) -> Target {
    let dir = project.join("node_modules").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        format!(r#"{{"name":"{name}","version":"1.0.0","scripts":{scripts}}}"#),
    )
    .unwrap();
    Target {
        name: name.to_owned(),
        version: "1.0.0".to_owned(),
        dir,
        root: false,
    }
}

fn allow(names: &[&str]) -> Allow {
    Allow::Packages(
        names
            .iter()
            .map(|n| (*n).to_owned())
            .collect::<BTreeSet<_>>(),
    )
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// One `\n`-terminated line per line: `cmd` echoes with `\r\n` and keeps the space before `>>`.
fn read_log(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .map(|line| format!("{}\n", line.trim_end()))
        .collect()
}

#[test]
fn nothing_runs_by_default_and_the_package_is_listed() {
    let dir = tmp();
    let t = package(dir.path(), "a", r#"{"postinstall":"touch ran"}"#);
    let report = run(dir.path(), std::slice::from_ref(&t), &Allow::None, None).unwrap();
    assert!(!t.dir.join("ran").exists());
    assert!(report.ran.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].name, "a");
    assert_eq!(report.skipped[0].events, ["postinstall"]);
}

#[test]
fn only_the_allowed_package_runs() {
    let dir = tmp();
    let a = package(dir.path(), "a", r#"{"postinstall":"touch ran"}"#);
    let b = package(dir.path(), "b", r#"{"postinstall":"touch ran"}"#);
    let report = run(dir.path(), &[a.clone(), b.clone()], &allow(&["a"]), None).unwrap();
    assert!(a.dir.join("ran").exists());
    assert!(!b.dir.join("ran").exists());
    assert_eq!(report.ran, ["a@1.0.0"]);
    assert_eq!(report.skipped[0].name, "b");
}

#[test]
fn allow_all_runs_everything() {
    let dir = tmp();
    let b = package(dir.path(), "b", r#"{"install":"touch ran"}"#);
    run(dir.path(), std::slice::from_ref(&b), &Allow::All, None).unwrap();
    assert!(b.dir.join("ran").exists());
}

#[test]
fn events_run_in_lifecycle_order_and_other_scripts_never() {
    let dir = tmp();
    let t = package(
        dir.path(),
        "a",
        r#"{"test":"echo test >> log","postinstall":"echo post >> log","preinstall":"echo pre >> log","install":"echo install >> log"}"#,
    );
    run(dir.path(), std::slice::from_ref(&t), &Allow::All, None).unwrap();
    assert_eq!(read_log(&t.dir.join("log")), "pre\ninstall\npost\n");
}

#[test]
fn a_script_runs_once_per_installed_version() {
    let dir = tmp();
    let t = package(dir.path(), "a", r#"{"postinstall":"echo x >> log"}"#);
    for _ in 0..2 {
        run(dir.path(), std::slice::from_ref(&t), &Allow::All, None).unwrap();
    }
    assert_eq!(read_log(&t.dir.join("log")), "x\n");
    forget(dir.path(), &["a@1.0.0".to_owned()]);
    run(dir.path(), std::slice::from_ref(&t), &Allow::All, None).unwrap();
    assert_eq!(read_log(&t.dir.join("log")), "x\nx\n");
}

#[test]
fn a_failing_script_fails_the_install() {
    let dir = tmp();
    let t = package(dir.path(), "a", r#"{"postinstall":"exit 3"}"#);
    let err = run(dir.path(), &[t], &Allow::All, None).unwrap_err();
    assert!(err.to_string().contains("a@1.0.0"), "{err}");
}

// POSIX `env` and its shell-added variables; `cmd` has neither.
#[cfg(unix)]
#[test]
fn the_environment_is_minimal() {
    let dir = tmp();
    let t = package(dir.path(), "a", r#"{"postinstall":"env > env.txt"}"#);
    run(dir.path(), std::slice::from_ref(&t), &Allow::All, None).unwrap();
    let text = std::fs::read_to_string(t.dir.join("env.txt")).unwrap();
    let keys: BTreeSet<&str> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, _)| k)
        .collect();
    // The shell itself adds PWD, SHLVL, OLDPWD and `_`.
    let allowed = [
        "npm_package_name",
        "npm_lifecycle_event",
        "PATH",
        "HOME",
        SHIM_FLAGS_ENV,
        "PWD",
        "SHLVL",
        "OLDPWD",
        "_",
    ];
    let extra: Vec<_> = keys.iter().filter(|k| !allowed.contains(k)).collect();
    assert!(extra.is_empty(), "leaked: {extra:?}");
    assert!(
        text.contains("npm_package_name=a\n") && text.contains("npm_lifecycle_event=postinstall\n")
    );
}

// POSIX `$PATH` and `command -v`; Windows writes `node.cmd` instead of `node`.
#[cfg(unix)]
#[test]
fn path_has_node_modules_bin_and_node_is_the_shim() {
    let dir = tmp();
    let t = package(
        dir.path(),
        "a",
        r#"{"postinstall":"echo $PATH > path.txt; command -v node > node.txt"}"#,
    );
    run(
        dir.path(),
        std::slice::from_ref(&t),
        &Allow::All,
        Some(Path::new("/opt/velyr")),
    )
    .unwrap();
    let path = std::fs::read_to_string(t.dir.join("path.txt")).unwrap();
    assert!(
        path.contains(&dir.path().join("node_modules/.bin").display().to_string()),
        "{path}"
    );
    let node = std::fs::read_to_string(t.dir.join("node.txt")).unwrap();
    let shim = dir.path().join("node_modules/.velyr/bin/node");
    assert_eq!(node.trim(), shim.display().to_string());
    let body = std::fs::read_to_string(shim).unwrap();
    assert!(body.contains("run --node-shim"), "{body}");
    assert!(
        !body.contains(" -A"),
        "the shim must not grant -A itself: {body}"
    );
}

#[test]
fn scripts_hand_the_shim_explicit_flags_and_nothing_else_does() {
    let dir = tmp();
    let echo = if cfg!(windows) {
        "echo %VELYR_NODE_SHIM_FLAGS%> flags.txt"
    } else {
        "echo $VELYR_NODE_SHIM_FLAGS > flags.txt"
    };
    let t = package(dir.path(), "a", &format!(r#"{{"postinstall":"{echo}"}}"#));
    run(dir.path(), std::slice::from_ref(&t), &Allow::All, None).unwrap();
    let flags = std::fs::read_to_string(t.dir.join("flags.txt")).unwrap();
    assert_eq!(flags.trim(), "-A");
}

#[test]
fn allow_matching() {
    assert!(allow(&["a"]).permits("a", false));
    assert!(!allow(&["a"]).permits("b", false));
    assert!(allow(&["."]).permits("anything", true));
    assert!(!Allow::None.permits("a", false));
    assert!(Allow::All.permits("a", true));
}
