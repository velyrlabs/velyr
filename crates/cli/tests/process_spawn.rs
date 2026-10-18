//! Spawning children through the real `velyr` binary: `--allow-run` decides what may start, `exec`
//! needs the shell granted, and a child's default environment is `process.env`, which only holds
//! the variables `--allow-env` granted. Each run has a timeout, so a hang fails the test.

// Unix only: the scripts spawn `sh`, `echo` and `ls`, and the environment pins a POSIX `PATH`.
#![cfg(unix)]

use std::time::Duration;

use assert_cmd::Command;

const TIMEOUT: Duration = Duration::from_secs(60);

/// Runs `script` with `flags` on every engine this build has; returns (success, stdout, stderr).
/// The host environment is cleared except for a PATH, plus `FOO` and `BAR`.
fn velyr(flags: &[&str], script: &str) -> Vec<(bool, String, String)> {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.js");
    std::fs::write(&file, script).unwrap();
    let mut engines = vec![vec![]];
    if cfg!(feature = "engine-native") {
        engines.push(vec!["--engine", "native"]);
    }
    engines
        .into_iter()
        .map(|engine| {
            let output = Command::cargo_bin("velyr")
                .unwrap()
                .current_dir(dir.path())
                .env_clear()
                .env("PATH", "/bin:/usr/bin")
                .env("FOO", "1")
                .env("BAR", "2")
                // Harmless to the runtime itself; used to check loader variables never pass on.
                .env("LD_LIBRARY_PATH", "/nonexistent")
                .args(["run"])
                .args(&engine)
                .args(flags)
                .arg(&file)
                .timeout(TIMEOUT)
                .output()
                .unwrap();
            (
                output.status.success(),
                String::from_utf8(output.stdout).unwrap(),
                String::from_utf8(output.stderr).unwrap(),
            )
        })
        .collect()
}

fn expect(flags: &[&str], script: &str, expected: &str) {
    for (ok, stdout, stderr) in velyr(flags, script) {
        assert!(ok, "stdout: {stdout}\nstderr: {stderr}");
        assert_eq!(stdout, expected, "stderr: {stderr}");
    }
}

const CODE_OF: &str =
    "const codeOf = (fn) => { try { fn(); return 'allowed'; } catch (e) { return e.code; } };
     const cp = require('node:child_process');";

#[test]
fn without_allow_run_spawning_is_denied() {
    let script = format!(
        "{CODE_OF}
         console.log(codeOf(() => cp.execFileSync('echo', ['hi'])));
         new Velyr.Command('echo').output().catch((e) => console.log(e.code));"
    );
    expect(&[], &script, "ERR_ACCESS_DENIED\nVELYR_PERMISSION_DENIED\n");
}

#[test]
fn allow_run_echo_runs_echo_and_denies_ls() {
    let script = format!(
        "{CODE_OF}
         console.log(cp.execFileSync('echo', ['hi']).toString().trim());
         console.log(codeOf(() => cp.execFileSync('ls', ['/'])));
         (async () => {{
           const out = await new Velyr.Command('echo', {{ args: ['velyr'] }}).output();
           console.log(new TextDecoder().decode(out.stdout).trim());
           await new Velyr.Command('ls').output().catch((e) => console.log(e.code));
         }})();"
    );
    expect(
        &["--allow-run=echo"],
        &script,
        "hi\nERR_ACCESS_DENIED\nvelyr\nVELYR_PERMISSION_DENIED\n",
    );
}

#[test]
fn exec_fails_unless_the_shell_is_granted() {
    let script =
        format!("{CODE_OF} console.log(codeOf(() => cp.execSync('echo hi').toString().trim()));");
    expect(&["--allow-run=echo"], &script, "ERR_ACCESS_DENIED\n");
    let script =
        "console.log(require('node:child_process').execSync('echo hi').toString().trim());";
    expect(&["--allow-run=/bin/sh"], script, "hi\n");
}

#[test]
fn a_child_inherits_only_the_env_that_was_granted() {
    let script = "console.log(require('node:child_process')
        .execFileSync('sh', ['-c', 'echo \"${FOO-unset}:${BAR-unset}\"']).toString().trim());";
    expect(&["--allow-run=sh", "--allow-env=FOO"], script, "1:unset\n");
    // No env grant at all: the child gets nothing.
    expect(&["--allow-run=sh"], script, "unset:unset\n");
    // An explicit env is what it is, whatever process.env holds.
    let explicit = "console.log(require('node:child_process')
        .execFileSync('sh', ['-c', 'echo \"${FOO-unset}:${BAR-unset}\"'], { env: { BAR: 'x' } }).toString().trim());";
    expect(
        &["--allow-run=sh", "--allow-env=FOO"],
        explicit,
        "unset:x\n",
    );
}

#[test]
fn a_loader_variable_in_the_inherited_env_is_refused() {
    let script =
        format!("{CODE_OF} console.log(codeOf(() => cp.execFileSync('sh', ['-c', 'echo hi'])));");
    // Granting LD_LIBRARY_PATH to the script makes it part of process.env, which a child inherits:
    // the spawn is refused rather than passing it on.
    expect(
        &["--allow-run=sh", "--allow-env=FOO,LD_LIBRARY_PATH"],
        &script,
        "ERR_ACCESS_DENIED\n",
    );
    expect(&["--allow-run=sh", "--allow-env=FOO"], &script, "allowed\n");
}

#[test]
fn the_runtime_cannot_be_spawned_to_gain_permissions() {
    let exe = env!("CARGO_BIN_EXE_velyr");
    let run_flag = format!("--allow-run={exe}");
    // `process.execPath` is the runtime itself: granted by path, it is still refused, so a script
    // cannot start `velyr run -A x.js` (or `fork('x.js', ['-A'])`) and escape its permissions.
    let script = format!(
        "{CODE_OF}
         console.log(codeOf(() => cp.execFileSync(process.execPath, ['run', '-A', 'x.js'])));
         console.log(codeOf(() => cp.execFileSync({exe:?}, ['--version'])));
         console.log(codeOf(() => cp.fork('x.js', ['-A'])));
         const child = cp.spawn(process.execPath, ['run', '-A', 'x.js']);
         child.on('error', (e) => console.log(e.code, /runtime/i.test(e.message)));"
    );
    expect(
        &[&run_flag],
        &script,
        "ERR_ACCESS_DENIED\nERR_ACCESS_DENIED\nERR_VELYR_UNSUPPORTED\nERR_ACCESS_DENIED true\n",
    );
}
