//! M3 security acceptance: default-deny end to end through the `velyr` binary.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

const DENIED: &str = "VELYR_PERMISSION_DENIED";
const SECRET_VALUE: &str = "s3cr3t-value-7f3a9c1e";

fn fixtures(name: &str) -> PathBuf {
    dunce::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/m3-security")
            .join(name),
    )
    .unwrap()
}

fn velyr(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("velyr").unwrap();
    cmd.current_dir(cwd)
        .env_remove("VELYR_LOG")
        .env_remove("VELYR_LEAK_CHECK")
        .arg("run")
        .args(args);
    cmd
}

/// Runs the script and parses the single JSON line it prints on stdout.
fn probe(cwd: &Path, args: &[&str]) -> Value {
    let output = velyr(cwd, args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn script_without_flags_is_denied_every_ambient_capability() {
    let results = probe(&fixtures("default-deny"), &["main.js"]);
    for key in [
        "readFile",
        "stat",
        "writeFile",
        "envGet",
        "envHas",
        "envToObject",
        "envSet",
        "importOutside",
        "importAbsolute",
    ] {
        assert_eq!(results[key], DENIED, "{key}");
    }
    assert_eq!(results["outsideLoaded"], false);
}

/// Every network API that ships rejects the call without flags (the probe's `call ` keys).
#[test]
fn script_without_flags_is_denied_every_network_call() {
    let results = probe(&fixtures("default-deny"), &["main.js"]);
    for key in [
        "fetch",
        "connect",
        "listen",
        "serve",
        "connectTls",
        "listenTls",
        "Command",
    ] {
        assert_eq!(results[format!("call {key}")], DENIED, "{key}");
    }
    assert_eq!(results["call processEnv"], DENIED);
    // The WebSocket constructor never throws; the denial shows as a failed dial, recorded as a
    // permission-denied event (a plain refused connection would not produce one).
    assert_eq!(results["call WebSocket"], "closed-without-open");
    let output = velyr(&fixtures("default-deny"), &["main.js"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("velyr.permission.denied") && stderr.contains("op_ws_connect"),
        "{stderr}"
    );
}

#[test]
fn script_without_flags_has_no_spawn_ffi_or_other_ambient_apis() {
    let results = probe(&fixtures("default-deny"), &["main.js"]);
    let present: Vec<_> = results
        .as_object()
        .unwrap()
        .iter()
        .filter(|(key, value)| key.starts_with("typeof ") && *value != "undefined")
        .collect();
    assert!(present.is_empty(), "unexpected APIs exposed: {present:?}");
}

/// When an API above ships it must be denied without flags, so this guards the probe list: it
/// fails if the probe is not actually reporting `typeof` results.
#[test]
fn absence_probe_discriminates_between_present_and_absent_apis() {
    let results = probe(&fixtures("default-deny"), &["main.js"]);
    assert_eq!(results["typeof Velyr.spawn"], "undefined");
    let dir = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(cwd.join("main.js"), "console.log(typeof Velyr.readFile);").unwrap();
    let output = velyr(&cwd, &["main.js"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "function\n");
}

#[test]
fn script_with_grants_proves_default_deny_assertions_discriminate() {
    let results = probe(
        &fixtures("default-deny"),
        &["--allow-read=.", "--allow-env", "main.js"],
    );
    assert_eq!(results["readFile"], "allowed");
    assert_eq!(results["envGet"], "allowed");
    assert_eq!(results["writeFile"], DENIED);
    assert_eq!(results["importOutside"], DENIED);
}

#[test]
fn deny_read_wins_over_allow_read() {
    let cwd = fixtures("deny-wins");
    let results = probe(&cwd, &["--allow-read=.", "--deny-read=./secret", "main.js"]);
    assert_eq!(results["publicFile"], "public\n");
    assert_eq!(results["secretFile"], DENIED);
    assert_eq!(results["secretDir"], DENIED);

    let control = probe(&cwd, &["--allow-read=.", "main.js"]);
    assert_eq!(control["secretFile"], "top secret\n");
}

#[cfg(unix)]
#[test]
fn symlink_inside_root_pointing_outside_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(dir.path()).unwrap();
    let (root, outside) = (base.join("root"), base.join("outside"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "outside secret").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link-dir")).unwrap();
    std::fs::write(
        root.join("main.js"),
        r#"const attempt = async (path) => {
  try { return await Velyr.readTextFile(path); } catch (e) { return e.code; }
};
console.log(JSON.stringify([await attempt("link.txt"), await attempt("link-dir/secret.txt")]));"#,
    )
    .unwrap();

    let denied = velyr(&root, &["--allow-read=.", "main.js"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&denied.stdout),
        format!("[\"{DENIED}\",\"{DENIED}\"]\n")
    );

    let outside_flag = format!("--allow-read=.,{}", outside.display());
    let control = velyr(&root, &[&outside_flag, "main.js"]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&control.stdout),
        "[\"outside secret\",\"outside secret\"]\n"
    );
}

#[test]
fn denial_events_and_messages_do_not_leak_env_values() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(
        cwd.join("main.js"),
        r#"for (const call of [() => Velyr.env.get("VELYR_M3_SECRET"), () => Velyr.env.toObject()]) {
  try { call(); } catch (e) { console.log(e.code, e.message); }
}"#,
    )
    .unwrap();

    for allow in [None, Some("--allow-env=UNRELATED")] {
        let mut args: Vec<&str> = allow.into_iter().collect();
        args.push("main.js");
        let output = velyr(&cwd, &args)
            .env("VELYR_M3_SECRET", SECRET_VALUE)
            .env("VELYR_LOG", "debug")
            .args(["--log-format", "json"])
            .output()
            .unwrap();
        let (stdout, stderr) = (
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(output.status.success(), "{stderr}");
        assert!(stdout.contains(DENIED), "{stdout}");
        assert!(stderr.contains("velyr.permission.denied"), "{stderr}");
        assert!(!stdout.contains(SECRET_VALUE), "{stdout}");
        assert!(!stderr.contains(SECRET_VALUE), "{stderr}");
    }
}

#[test]
fn env_value_is_visible_to_script_when_granted() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(
        cwd.join("main.js"),
        r#"console.log(Velyr.env.get("VELYR_M3_SECRET"));"#,
    )
    .unwrap();
    let output = velyr(&cwd, &["--allow-env=VELYR_M3_SECRET", "main.js"])
        .env("VELYR_M3_SECRET", SECRET_VALUE)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains(SECRET_VALUE));
}

#[test]
fn leak_check_reports_no_open_resources_after_run() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(
        cwd.join("main.js"),
        r#"await Velyr.writeTextFile("a.txt", "x");
console.log(await Velyr.readTextFile("a.txt"));
setTimeout(() => console.log("timer"), 1);"#,
    )
    .unwrap();
    let output = velyr(&cwd, &["--allow-read=.", "--allow-write=.", "main.js"])
        .env("VELYR_LEAK_CHECK", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("leak"));
}
