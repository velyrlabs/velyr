//! Lifecycle scripts of dependencies: denied by default, opt-in per package. Loopback registry only.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use assert_cmd::Command;
use base64::Engine;
use sha2::{Digest, Sha512};

const TOUCH: &str = "touch ran.txt";

fn tarball(name: &str, postinstall: &str) -> Vec<u8> {
    let manifest = format!(
        r#"{{"name":"{name}","version":"1.0.0","scripts":{{"postinstall":"{postinstall}","test":"touch test-ran.txt"}}}}"#
    );
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in [
        ("package/package.json", manifest.as_str()),
        ("package/index.js", "module.exports = 1;\n"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, body.as_bytes())
            .unwrap();
    }
    let raw = builder.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&raw).unwrap();
    gz.finish().unwrap()
}

fn start_registry(pkgs: &[(&str, &str)]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    for (name, script) in pkgs {
        let tgz = tarball(name, script);
        let integrity = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&tgz))
        );
        let packument = serde_json::json!({
            "name": name,
            "dist-tags": {"latest": "1.0.0"},
            "versions": {"1.0.0": {
                "name": name, "version": "1.0.0", "hasInstallScript": true,
                "dist": {"tarball": format!("{base}{name}/-/{name}-1.0.0.tgz"), "integrity": integrity},
            }},
        });
        routes.insert(format!("/{name}"), packument.to_string().into_bytes());
        routes.insert(format!("/{name}/-/{name}-1.0.0.tgz"), tgz);
    }
    let routes = Arc::new(routes);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let routes = routes.clone();
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head);
                let path = head.split(' ').nth(1).unwrap_or("/");
                let (status, body) = match routes.get(path) {
                    Some(body) => ("200 OK", body.as_slice()),
                    None => ("404 Not Found", &b"{}"[..]),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            });
        }
    });
    base
}

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    fn new(manifest: &str, pkgs: &[(&str, &str)]) -> Self {
        let registry = start_registry(pkgs);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("package.json"), manifest).unwrap();
        std::fs::write(root.join(".npmrc"), format!("registry={registry}\n")).unwrap();
        Self { _dir: dir, root }
    }

    fn velyr(&self, args: &[&str]) -> std::process::Output {
        let home = self.root.parent().unwrap();
        Command::cargo_bin("velyr")
            .unwrap()
            .current_dir(&self.root)
            .env("HOME", home)
            .env("VELYR_NPM_CACHE_DIR", home.join("cache"))
            .env("SECRET_TOKEN", "hunter2")
            .args(args)
            .timeout(std::time::Duration::from_secs(60))
            .output()
            .unwrap()
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }
}

fn manifest(deps: &[&str]) -> String {
    let deps: Vec<String> = deps.iter().map(|d| format!(r#""{d}":"^1.0.0""#)).collect();
    format!(
        r#"{{"name":"app","version":"1.0.0","dependencies":{{{}}},"scripts":{{"postinstall":"touch root-ran.txt"}}}}"#,
        deps.join(",")
    )
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const A_RAN: &str = "node_modules/pkga/ran.txt";
const B_RAN: &str = "node_modules/pkgb/ran.txt";

#[test]
fn scripts_do_not_run_by_default_and_the_warning_lists_the_package() {
    let p = Project::new(&manifest(&["pkga"]), &[("pkga", TOUCH)]);
    let out = p.velyr(&["install"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!p.exists(A_RAN));
    let err = stderr(&out);
    assert!(err.contains("pkga@1.0.0"), "{err}");
    assert!(err.contains("--allow-scripts=pkga"), "{err}");
    assert!(!p.exists("root-ran.txt"), "root scripts also need opt-in");
}

#[test]
fn allow_scripts_runs_only_the_named_package() {
    let p = Project::new(
        &manifest(&["pkga", "pkgb"]),
        &[("pkga", TOUCH), ("pkgb", TOUCH)],
    );
    let out = p.velyr(&["install", "--allow-scripts=pkga"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(p.exists(A_RAN));
    assert!(!p.exists(B_RAN));
    assert!(stderr(&out).contains("pkgb@1.0.0"));
    assert!(
        !p.exists("node_modules/pkga/test-ran.txt"),
        "only lifecycle events run"
    );
}

#[test]
fn allowing_after_a_plain_install_runs_the_script() {
    let p = Project::new(&manifest(&["pkga"]), &[("pkga", TOUCH)]);
    assert!(p.velyr(&["install"]).status.success());
    assert!(!p.exists(A_RAN));
    assert!(
        p.velyr(&["install", "--allow-scripts=pkga"])
            .status
            .success()
    );
    assert!(p.exists(A_RAN));
}

#[test]
fn the_script_environment_has_no_user_variables() {
    let p = Project::new(&manifest(&["pkga"]), &[("pkga", "env > env.txt")]);
    let out = p.velyr(&["install", "--allow-scripts=pkga"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let env = std::fs::read_to_string(p.root.join("node_modules/pkga/env.txt")).unwrap();
    assert!(
        !env.contains("SECRET_TOKEN") && !env.contains("hunter2"),
        "{env}"
    );
    assert!(
        env.contains("npm_package_name=pkga") && env.contains("npm_lifecycle_event=postinstall")
    );
}

#[test]
fn allow_all_scripts_runs_everything_with_a_warning() {
    let p = Project::new(
        &manifest(&["pkga", "pkgb"]),
        &[("pkga", TOUCH), ("pkgb", TOUCH)],
    );
    let out = p.velyr(&["install", "--allow-all-scripts"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(p.exists(A_RAN) && p.exists(B_RAN) && p.exists("root-ran.txt"));
    assert!(stderr(&out).contains("warning"), "{}", stderr(&out));
}

#[test]
fn allow_scripts_in_velyr_json_and_the_root_opt_in() {
    let p = Project::new(
        &manifest(&["pkga", "pkgb"]),
        &[("pkga", TOUCH), ("pkgb", TOUCH)],
    );
    std::fs::write(
        p.root.join("velyr.json"),
        r#"{"allowScripts":["pkga","app"]}"#,
    )
    .unwrap();
    let out = p.velyr(&["install"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(p.exists(A_RAN) && p.exists("root-ran.txt"));
    assert!(!p.exists(B_RAN));
}

#[test]
fn a_failing_script_fails_the_install() {
    let p = Project::new(&manifest(&["pkga"]), &[("pkga", "exit 7")]);
    let out = p.velyr(&["install", "--allow-scripts=pkga"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("pkga@1.0.0"), "{}", stderr(&out));
}

#[cfg(feature = "node")]
#[test]
fn node_in_a_script_runs_through_velyr() {
    let p = Project::new(
        &manifest(&["pkga"]),
        &[("pkga", "node index.js && touch ran.txt")],
    );
    let out = p.velyr(&["install", "--allow-scripts=pkga"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(p.exists(A_RAN));
}
