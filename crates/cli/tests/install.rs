//! `velyr add/remove/install` against a loopback registry; every path lives in a tempdir.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use assert_cmd::Command;
use base64::Engine;
use sha2::{Digest, Sha512};

struct Pkg {
    name: &'static str,
    version: &'static str,
    deps: &'static [(&'static str, &'static str)],
}

const PKGS: &[Pkg] = &[
    Pkg {
        name: "left-pad",
        version: "1.3.0",
        deps: &[("tiny", "^1.0.0")],
    },
    Pkg {
        name: "tiny",
        version: "1.0.0",
        deps: &[],
    },
    Pkg {
        name: "devtool",
        version: "2.0.0",
        deps: &[],
    },
];

fn tarball(pkg: &Pkg) -> Vec<u8> {
    let manifest = format!(
        r#"{{"name":"{}","version":"{}","bin":{{"{}":"./cli.js"}}}}"#,
        pkg.name, pkg.version, pkg.name
    );
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in [
        ("package/package.json", manifest.as_str()),
        ("package/index.js", "module.exports = 1;\n"),
        ("package/cli.js", "#!/usr/bin/env node\n"),
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

fn packument(pkg: &Pkg, base: &str, tgz: &[u8]) -> String {
    let integrity = format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha512::digest(tgz))
    );
    let deps: HashMap<_, _> = pkg.deps.iter().copied().collect();
    serde_json::json!({
        "name": pkg.name,
        "dist-tags": {"latest": pkg.version},
        "versions": {pkg.version: {
            "name": pkg.name, "version": pkg.version, "dependencies": deps,
            "dist": {"tarball": format!("{base}{0}/-/{0}-{1}.tgz", pkg.name, pkg.version), "integrity": integrity},
        }},
    })
    .to_string()
}

/// Requests in flight now and the most there ever were at once.
#[derive(Default)]
struct Gauge {
    now: AtomicUsize,
    peak: AtomicUsize,
}

fn start_registry() -> String {
    start_registry_with(PKGS, Duration::ZERO, Arc::default())
}

/// Serves `pkgs`; every packument reply waits `delay` while counted in `gauge`.
fn start_registry_with(pkgs: &[Pkg], delay: Duration, gauge: Arc<Gauge>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    for pkg in pkgs {
        let tgz = tarball(pkg);
        routes.insert(
            format!("/{}", pkg.name),
            packument(pkg, &base, &tgz).into_bytes(),
        );
        routes.insert(format!("/{0}/-/{0}-{1}.tgz", pkg.name, pkg.version), tgz);
    }
    let routes = Arc::new(routes);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let routes = routes.clone();
            let gauge = gauge.clone();
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
                let counted = !path.contains("/-/");
                if counted {
                    let now = gauge.now.fetch_add(1, Ordering::SeqCst) + 1;
                    gauge.peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(delay);
                    gauge.now.fetch_sub(1, Ordering::SeqCst);
                }
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
    registry: String,
}

impl Project {
    fn new(manifest: &str) -> Self {
        Self::on(manifest, start_registry())
    }

    fn on(manifest: &str, registry: String) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("package.json"), manifest).unwrap();
        std::fs::write(root.join(".npmrc"), format!("registry={registry}\n")).unwrap();
        Self {
            _dir: dir,
            root,
            registry,
        }
    }

    fn velyr(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        let home = self.root.parent().unwrap();
        Command::cargo_bin("velyr")
            .unwrap()
            .current_dir(&self.root)
            .env("HOME", home)
            .env("VELYR_NPM_CACHE_DIR", home.join("cache"))
            .args(args)
            .timeout(std::time::Duration::from_secs(60))
            .assert()
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap()
    }

    /// Points `.npmrc` at a port nobody listens on: any registry call fails.
    fn cut_the_network(&self) {
        std::fs::write(self.root.join(".npmrc"), "registry=http://127.0.0.1:1/\n").unwrap();
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).symlink_metadata().is_ok()
    }
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::symlink_metadata(path).unwrap().modified().unwrap()
}

const MANIFEST: &str =
    "{\n    \"name\": \"app\",\n    \"version\": \"1.0.0\",\n    \"scripts\": {\"x\": \"y\"}\n}\n";

#[test]
fn add_creates_node_modules_lock_and_edits_package_json() {
    let p = Project::new(MANIFEST);
    let out = p
        .velyr(&["add", "left-pad"])
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("added 2"), "{out}");
    assert!(out.contains("removed 0"), "{out}");
    assert!(p.exists("node_modules/left-pad/index.js"));
    assert!(p.exists("node_modules/tiny/index.js"));
    assert!(!p.exists("node_modules/left-pad/.velyr-meta.json"));
    assert!(p.read("velyr.lock").contains("left-pad@1.3.0"));
    // Order and the 4-space indent of the original file survive.
    assert_eq!(
        p.read("package.json"),
        "{\n    \"name\": \"app\",\n    \"version\": \"1.0.0\",\n    \"scripts\": {\"x\": \"y\"},\n    \"dependencies\": {\n        \"left-pad\": \"^1.3.0\"\n    }\n}\n"
            .replace("{\"x\": \"y\"}", "{\n        \"x\": \"y\"\n    }")
    );
    #[cfg(unix)]
    assert_eq!(
        std::fs::read_link(p.root.join("node_modules/.bin/tiny")).unwrap(),
        Path::new("../tiny/cli.js")
    );
}

#[test]
fn add_dev_and_exact_choose_section_and_spec() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "-D", "--exact", "devtool"]).success();
    let manifest: serde_json::Value = serde_json::from_str(&p.read("package.json")).unwrap();
    assert_eq!(manifest["devDependencies"]["devtool"], "2.0.0");
    assert!(manifest.get("dependencies").is_none());
    p.velyr(&["add", "tiny@^1.0.0"]).success();
    assert!(p.read("package.json").contains("\"tiny\": \"^1.0.0\""));
}

#[test]
fn remove_clears_package_json_lock_and_node_modules() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    let out = p
        .velyr(&["remove", "left-pad"])
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&out).contains("removed 2"));
    assert!(!p.read("package.json").contains("left-pad"));
    assert!(!p.read("velyr.lock").contains("left-pad"));
    assert!(!p.exists("node_modules/left-pad"));
    assert!(!p.exists("node_modules/tiny"));
    assert!(!p.exists("node_modules/.bin/tiny"));
}

#[test]
fn second_install_rewrites_nothing() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    // Bins are symlinks on Unix and `.cmd` shims on Windows.
    let bin = if cfg!(windows) {
        "node_modules/.bin/tiny.cmd"
    } else {
        "node_modules/.bin/tiny"
    };
    let watched = [
        "velyr.lock",
        "package.json",
        "node_modules/left-pad/index.js",
        bin,
        "node_modules/.velyr-state.json",
    ];
    let before: Vec<_> = watched.iter().map(|w| mtime(&p.root.join(w))).collect();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let out = p.velyr(&["install"]).success().get_output().stdout.clone();
    assert!(String::from_utf8_lossy(&out).contains("added 0"));
    let after: Vec<_> = watched.iter().map(|w| mtime(&p.root.join(w))).collect();
    assert_eq!(before, after);
}

#[test]
fn install_rebuilds_a_missing_node_modules_from_the_lock() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    std::fs::remove_dir_all(p.root.join("node_modules")).unwrap();
    p.velyr(&["install", "--frozen"]).success();
    assert!(p.exists("node_modules/tiny/index.js"));
}

#[test]
fn frozen_fails_on_drift_and_leaves_files_alone() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    let manifest = p.read("package.json").replace("^1.3.0", "^1.2.0");
    std::fs::write(p.root.join("package.json"), manifest).unwrap();
    let lock = p.read("velyr.lock");
    p.velyr(&["install", "--frozen"]).failure();
    assert_eq!(p.read("velyr.lock"), lock);
}

#[test]
fn production_skips_dev_dependencies() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    p.velyr(&["add", "-D", "devtool"]).success();
    std::fs::remove_dir_all(p.root.join("node_modules")).unwrap();
    p.velyr(&["install", "--production"]).success();
    assert!(p.exists("node_modules/left-pad"));
    assert!(!p.exists("node_modules/devtool"));
}

#[test]
fn unknown_package_fails_without_touching_package_json() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "nope"]).failure();
    assert_eq!(p.read("package.json"), MANIFEST);
    assert!(!p.exists("velyr.lock"));
    let _ = &p.registry;
}

#[test]
fn a_complete_lock_with_a_warm_cache_installs_without_the_network() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    assert!(p.read("velyr.lock").contains("\"complete\": true"));
    p.cut_the_network();
    std::fs::remove_dir_all(p.root.join("node_modules")).unwrap();
    p.velyr(&["install", "--frozen"]).success();
    assert!(p.exists("node_modules/tiny/index.js"));
    let lock = p.read("velyr.lock");
    p.velyr(&["install"]).success();
    assert_eq!(p.read("velyr.lock"), lock);
}

#[test]
fn a_complete_lock_without_its_tarball_fails_instead_of_guessing() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    p.cut_the_network();
    // The tarball URLs of the lock point at the dead registry too.
    let lock = p
        .read("velyr.lock")
        .replace(&p.registry, "http://127.0.0.1:1/");
    std::fs::write(p.root.join("velyr.lock"), lock).unwrap();
    std::fs::remove_dir_all(p.root.join("node_modules")).unwrap();
    std::fs::remove_dir_all(p.root.parent().unwrap().join("cache")).unwrap();
    p.velyr(&["install"]).failure();
    assert!(!p.exists("node_modules/left-pad"));
}

#[test]
fn an_old_lock_without_the_graph_is_resolved_again_and_upgraded() {
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    let mut lock: serde_json::Value = serde_json::from_str(&p.read("velyr.lock")).unwrap();
    lock.as_object_mut().unwrap().remove("complete");
    for package in lock["packages"].as_object_mut().unwrap().values_mut() {
        let keep = ["integrity", "resolved"];
        package
            .as_object_mut()
            .unwrap()
            .retain(|k, _| keep.contains(&k.as_str()));
    }
    std::fs::write(p.root.join("velyr.lock"), lock.to_string()).unwrap();
    std::fs::remove_dir_all(p.root.join("node_modules")).unwrap();
    p.velyr(&["install", "--frozen"]).success();
    assert!(p.exists("node_modules/tiny/index.js"));
    assert!(p.read("velyr.lock").contains("\"complete\": true"));
}

#[test]
fn fifty_packuments_are_fetched_in_parallel() {
    let pkgs: Vec<Pkg> = (0..50)
        .map(|i| Pkg {
            name: Box::leak(format!("wide{i}").into_boxed_str()),
            version: "1.0.0",
            deps: &[],
        })
        .collect();
    let gauge = Arc::new(Gauge::default());
    let registry = start_registry_with(&pkgs, Duration::from_millis(40), gauge.clone());
    let deps: Vec<String> = pkgs
        .iter()
        .map(|p| format!("\"{}\": \"^1.0.0\"", p.name))
        .collect();
    let manifest = format!(
        "{{\"name\":\"app\",\"dependencies\":{{{}}}}}",
        deps.join(",")
    );
    let p = Project::on(&manifest, registry);
    p.velyr(&["install"]).success();
    assert!(p.exists("node_modules/wide49/index.js"));
    let peak = gauge.peak.load(Ordering::SeqCst);
    assert!(peak > 1, "packuments were fetched one at a time");
    assert!(peak <= 16, "peak {peak} exceeds the limit");
}

#[cfg(unix)]
#[test]
fn linking_a_bin_leaves_the_cache_file_mode_alone() {
    use std::os::unix::fs::PermissionsExt;
    let p = Project::new(MANIFEST);
    p.velyr(&["add", "left-pad"]).success();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode(&p.root.join("node_modules/tiny/cli.js")) & 0o111,
        0o111
    );
    let cache = p.root.parent().unwrap().join("cache");
    let cached: Vec<_> = std::fs::read_dir(cache)
        .unwrap()
        .map(|e| e.unwrap().path().join("cli.js"))
        .filter(|f| f.exists())
        .collect();
    assert!(!cached.is_empty());
    for file in cached {
        assert_eq!(mode(&file) & 0o111, 0, "{}", file.display());
    }
    // A relinked package keeps its executable bin too.
    std::fs::remove_dir_all(p.root.join("node_modules/tiny")).unwrap();
    p.velyr(&["install"]).success();
    assert_eq!(
        mode(&p.root.join("node_modules/tiny/cli.js")) & 0o111,
        0o111
    );
}
