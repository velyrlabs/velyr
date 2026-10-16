#![no_main]

//! Arbitrary `exports` JSON and subpath through `NpmResolver`: resolution must never panic,
//! and a resolved file must live inside the package directory, even for `..` targets,
//! absolute targets, `node_modules` targets and a sibling package that shares a name prefix.
//!
//! Input: the first line is the subpath (empty means the package root), the rest is the
//! `exports` value as JSON.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use serde_json::json;
use velyr_compat_npm::resolve::NpmResolver;
use velyr_runtime_modules::BareSpecifierResolver;

struct Fixture {
    _dir: tempfile::TempDir,
    package: PathBuf,
    referrer: PathBuf,
    resolver: NpmResolver,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonical root");
        let proj = root.join("proj");
        let modules = proj.join("node_modules");
        let package = modules.join("pkg");
        for file in [
            package.join("index.js"),
            package.join("a.js"),
            package.join("lib/x.js"),
            package.join("lib/sub/y.mjs"),
            modules.join("pkg2/a.js"),
            modules.join("secret.js"),
            proj.join("main.js"),
            root.join("outside.js"),
        ] {
            fs::create_dir_all(file.parent().expect("parent")).expect("fixture dirs");
            fs::write(file, "export {};").expect("fixture file");
        }
        Fixture {
            referrer: proj.join("main.js"),
            package,
            resolver: NpmResolver::new(),
            _dir: dir,
        }
    })
}

fuzz_target!(|text: &str| {
    let (subpath, raw) = text.split_once('\n').unwrap_or(("", text));
    let Ok(exports) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    let fx = fixture();
    let manifest = json!({ "name": "pkg", "type": "module", "exports": exports });
    fs::write(fx.package.join("package.json"), manifest.to_string()).expect("write manifest");

    let specifier = if subpath.is_empty() {
        "pkg".to_owned()
    } else {
        format!("pkg/{subpath}")
    };
    let Ok(resolved) = fx.resolver.resolve_bare(&specifier, &fx.referrer) else {
        return;
    };
    let real = fs::canonicalize(&resolved).expect("resolved file exists");
    assert!(
        real.starts_with(&fx.package),
        "{specifier:?} with exports {raw:?} resolved outside the package: {resolved:?}"
    );
});
