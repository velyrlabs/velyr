//! `velyr x <bin>` and the `node` shim; every path lives in a tempdir.
#![cfg(all(feature = "node", unix))]

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use assert_cmd::Command;

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    /// A project whose `node_modules/tool` is the fixture package, with `.bin/tool` linked.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        let pkg = root.join("node_modules/tool");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        std::fs::write(root.join("package.json"), r#"{"name":"app"}"#).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bins/tool");
        for entry in std::fs::read_dir(fixture).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), pkg.join(entry.file_name())).unwrap();
        }
        let project = Self { _dir: dir, root };
        project.link("tool", "../tool/cli.js");
        project
    }

    fn link(&self, name: &str, target: &str) {
        let link = self.root.join("node_modules/.bin").join(name);
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    fn velyr(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::cargo_bin("velyr").unwrap();
        cmd.current_dir(&self.root)
            .env("HOME", self.root.parent().unwrap())
            .env("VELYR_LEAK_CHECK", "1")
            .args(args)
            .timeout(Duration::from_secs(30));
        cmd
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn x_runs_a_cjs_bin_with_its_arguments() {
    let p = Project::new();
    let help = p.velyr(&["x", "tool", "--help"]);
    assert!(help.status.success(), "{}", err(&help));
    assert!(out(&help).contains("usage: tool"), "{}", out(&help));
    let args = p.velyr(&["x", "tool", "a", "-b", "--c=d"]);
    assert!(args.status.success(), "{}", err(&args));
    assert!(
        out(&args).contains(r#"args:["a","-b","--c=d"]"#),
        "{}",
        out(&args)
    );
}

#[test]
fn x_runs_a_bin_without_extension_through_a_wrapper() {
    let p = Project::new();
    p.link("noext", "../tool/noext");
    let o = p.velyr(&["x", "noext", "a", "b"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("noext:a,b"), "{}", out(&o));
}

#[test]
fn x_finds_the_bin_from_a_subdirectory() {
    let p = Project::new();
    let sub = p.root.join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    let o = Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(&sub)
        .env("HOME", p.root.parent().unwrap())
        .args(["x", "tool", "--help"])
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("usage: tool"));
}

#[test]
fn x_rejects_names_that_are_not_a_plain_bin() {
    let p = Project::new();
    for name in ["../tool/cli.js", "node_modules/.bin/tool", "", ".."] {
        let o = p.velyr(&["x", name]);
        assert!(!o.status.success(), "{name:?} ran");
        assert!(err(&o).contains("bin name"), "{name:?}: {}", err(&o));
    }
}

#[test]
fn x_reports_a_missing_bin() {
    let p = Project::new();
    let o = p.velyr(&["x", "nope"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("nope"), "{}", err(&o));
}

#[test]
fn x_rejects_a_symlink_that_leaves_the_project() {
    let p = Project::new();
    let outside = p.root.parent().unwrap().join("outside.js");
    std::fs::write(&outside, "console.log('escaped');\n").unwrap();
    p.link("evil", outside.to_str().unwrap());
    let o = p.velyr(&["x", "evil"]);
    assert!(!o.status.success());
    assert!(!out(&o).contains("escaped"));
    assert!(err(&o).contains("outside the project"), "{}", err(&o));
}

#[test]
fn x_follows_a_symlink_outside_the_project_when_fs_read_allows_it() {
    let p = Project::new();
    let outside = p.root.parent().unwrap().join("outside.js");
    std::fs::write(&outside, "console.log('escaped');\n").unwrap();
    p.link("evil", outside.to_str().unwrap());
    let flag = format!("--allow-read={}", p.root.parent().unwrap().display());
    let o = p.velyr(&["x", &flag, "evil"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("escaped"));
}

#[test]
fn x_rejects_a_shebang_that_is_not_node() {
    let p = Project::new();
    p.link("sh", "../tool/shell.js");
    let o = p.velyr(&["x", "sh"]);
    assert!(!o.status.success());
    assert!(!out(&o).contains("not node"));
    assert!(err(&o).contains("not a Node.js script"), "{}", err(&o));
}

#[test]
fn run_accepts_cjs_entries() {
    let p = Project::new();
    std::fs::write(
        p.root.join("a.cjs"),
        "console.log('cjs:' + typeof require);\n",
    )
    .unwrap();
    let o = p.velyr(&["run", "a.cjs"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("cjs:function"), "{}", out(&o));
    // A `.js` entry inside a `"type": "commonjs"` package.
    let o = p.velyr(&["run", "node_modules/tool/cli.js", "--allow-read=."]);
    assert!(o.status.success(), "{}", err(&o));
}

/// The shim file exists once `velyr x` ran; scripts call it as `node`.
fn shim(p: &Project) -> PathBuf {
    let shim = p.root.join("node_modules/.velyr/bin/node");
    assert!(shim.exists(), "no shim at {}", shim.display());
    shim
}

fn run_shim(p: &Project, flags: Option<&str>, script: &str, arg: &str) -> Output {
    let mut cmd = std::process::Command::new(shim(p));
    cmd.current_dir(&p.root)
        .env_clear()
        .env("HOME", p.root.parent().unwrap())
        .args([script, arg]);
    if let Some(flags) = flags {
        cmd.env("VELYR_NODE_SHIM_FLAGS", flags);
    }
    // The shim execs the velyr under test through the path baked in at generation.
    cmd.output().unwrap()
}

#[test]
fn x_writes_a_node_shim_that_defaults_to_deny_all() {
    let p = Project::new();
    std::fs::write(p.root.join("data.txt"), "secret\n").unwrap();
    assert!(p.velyr(&["x", "tool"]).status.success());
    let script = "node_modules/tool/read.cjs";
    let denied = run_shim(&p, None, script, "data.txt");
    assert!(!denied.status.success(), "{}", out(&denied));
    assert!(!out(&denied).contains("secret"));
}

#[test]
fn the_shim_inherits_only_the_flags_of_its_caller() {
    let p = Project::new();
    std::fs::write(p.root.join("data.txt"), "secret\n").unwrap();
    // `velyr x` with no permissions: the child it spawns must not gain any.
    assert!(p.velyr(&["x", "tool"]).status.success());
    let script = "node_modules/tool/read.cjs";
    let allowed = run_shim(&p, Some("--allow-read=."), script, "data.txt");
    assert!(allowed.status.success(), "{}", err(&allowed));
    assert!(out(&allowed).contains("read:secret"), "{}", out(&allowed));
    let other = run_shim(&p, Some("--allow-read=./nowhere"), script, "data.txt");
    assert!(!other.status.success());
}

/// `velyr x <flags> spawner` makes the bin spawn the shim with an explicit env asking for `-A`.
fn spawn_shim_asking_for_a(p: &Project, flags: &[&str]) -> Output {
    spawn_shim_with(p, flags, "spawner")
}

fn spawn_shim_with(p: &Project, flags: &[&str], bin: &str) -> Output {
    std::fs::write(p.root.join("data.txt"), "secret\n").unwrap();
    p.link(bin, &format!("../tool/{bin}.cjs"));
    assert!(p.velyr(&["x", "tool"]).status.success());
    let shim = shim(p);
    let mut args = vec!["x"];
    args.extend_from_slice(flags);
    args.extend([bin, shim.to_str().unwrap()]);
    args.extend(["node_modules/tool/read.cjs", "data.txt"]);
    p.velyr(&args)
}

#[test]
fn a_child_cannot_raise_the_shim_flags_through_its_env() {
    let p = Project::new();
    let o = spawn_shim_asking_for_a(&p, &["--allow-read=node_modules", "--allow-run"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("status:"), "{}", out(&o));
    assert!(!out(&o).contains("read:secret"), "escalated: {}", out(&o));
}

#[test]
fn allow_run_warns_once_for_full_trust_programs_only() {
    let p = Project::new();
    std::fs::write(p.root.join("a.cjs"), "console.log('ok');\n").unwrap();
    let count = |flag: &str| {
        err(&p.velyr(&["run", "a.cjs", flag]))
            .matches("full trust")
            .count()
    };
    assert_eq!(count("--allow-run=sh"), 1);
    assert_eq!(count("--allow-run"), 1);
    assert_eq!(count("--allow-run=echo"), 0);
    assert_eq!(count("--allow-read=."), 0);
}

#[test]
fn a_worker_cannot_pass_the_process_flags_to_the_shim() {
    let p = Project::new();
    // The process may read everything, but the worker's children get no shim flags at all.
    let o = spawn_shim_with(&p, &["--allow-read=.", "--allow-run"], "worker-spawner");
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("status:"), "{}{}", out(&o), err(&o));
    assert!(!out(&o).contains("read:secret"), "inherited: {}", out(&o));
}

#[test]
fn a_spawned_shim_gets_the_flags_of_its_parent() {
    let p = Project::new();
    let o = spawn_shim_asking_for_a(&p, &["--allow-read=.", "--allow-run"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("read:secret"), "{}{}", out(&o), err(&o));
}
