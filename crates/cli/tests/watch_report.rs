//! The `--watch` report file is the runtime's own: a script cannot make velyr write to a file it
//! may not write, neither by handing a child the variable nor by running with it set.

use std::time::Duration;

use assert_cmd::Command;

const TIMEOUT: Duration = Duration::from_secs(30);

fn project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join("dep.mjs"), "export const value = 1;\n").unwrap();
    std::fs::write(
        root.join("main.mjs"),
        "import { value } from './dep.mjs';\nconsole.log('value=' + value);\n",
    )
    .unwrap();
    let victim = root.join("victim.txt");
    std::fs::write(&victim, "intact\n").unwrap();
    (dir, root, victim)
}

fn velyr(root: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("velyr").unwrap();
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .timeout(TIMEOUT);
    command
}

#[test]
fn a_script_cannot_hand_the_report_variable_to_a_child() {
    let (_guard, root, victim) = project();
    std::fs::write(
        root.join("spawn.js"),
        format!(
            "const cp = require('node:child_process');
             const out = cp.execFileSync('printenv', ['VELYR_WATCH_REPORT'], {{ env: {{ VELYR_WATCH_REPORT: {victim:?} }} }}, );
             console.log('child saw: ' + out.toString().trim());"
        ),
    )
    .unwrap();
    // `printenv` exits 1 when the variable is unset, which is the expected outcome.
    let output = velyr(&root)
        .args(["run", "--allow-run=printenv", "spawn.js"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("child saw"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "intact\n");
}

#[test]
fn velyr_ignores_a_report_that_is_not_the_supervisors() {
    let (_guard, root, victim) = project();
    let output = velyr(&root)
        .env("VELYR_WATCH_REPORT", &victim)
        .args(["run", "main.mjs"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "value=1\n");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "intact\n");
}
