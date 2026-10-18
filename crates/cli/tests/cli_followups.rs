use assert_cmd::Command;

fn velyr() -> Command {
    let mut cmd = Command::cargo_bin("velyr").unwrap();
    cmd.arg("run");
    cmd
}

fn fixture(name: &str) -> String {
    format!(
        "{}/../../tests/fixtures/cli-followups/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn structured_clone_clones_a_map() {
    velyr()
        .arg(fixture("clone.js"))
        .assert()
        .success()
        .stdout("Map 2\n");
}

#[test]
fn allow_env_grants_only_the_named_variable() {
    velyr()
        .args(["--allow-env=FOO", &fixture("env.js")])
        .env("FOO", "bar")
        .assert()
        .success()
        .stdout("bar\nVELYR_PERMISSION_DENIED\n");
}

#[cfg(unix)]
#[test]
fn entry_reached_through_a_symlink_can_import_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(dir.path()).unwrap();
    let real = base.join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("lib.js"), "export const v = 'lib-ok';").unwrap();
    std::fs::write(
        real.join("main.js"),
        "import { v } from './lib.js';\nconsole.log(v);",
    )
    .unwrap();
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    velyr()
        .arg(link.join("main.js"))
        .current_dir(&base)
        .assert()
        .success()
        .stdout("lib-ok\n");
}
