//! Node package resolution (exports, imports, conditions, self-reference) on temp `node_modules`
//! trees built from the cases in the Node.js "packages" documentation.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use velyr_compat_npm::resolve::NpmResolver;
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_modules::BareSpecifierResolver;

struct Tree {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Tree {
    /// `files`: `(relative path, content)`; `.json` content is written from the value verbatim.
    fn new(files: &[(&str, Value)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        for (rel, content) in files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = match content {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            fs::write(path, text).unwrap();
        }
        Self { _dir: dir, root }
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn resolve(&self, spec: &str) -> Result<PathBuf, VelyrError> {
        NpmResolver::new().resolve_bare(spec, &self.at("main.mjs"))
    }
}

fn pkg(name: &str, manifest: Value, files: &[&str]) -> Vec<(String, Value)> {
    let mut out = vec![(
        format!("node_modules/{name}/package.json"),
        merge(json!({"name": name, "type": "module"}), manifest),
    )];
    for f in files {
        out.push((format!("node_modules/{name}/{f}"), json!("export {};")));
    }
    out
}

fn merge(mut base: Value, extra: Value) -> Value {
    base.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    base
}

fn tree(parts: Vec<Vec<(String, Value)>>) -> Tree {
    let owned: Vec<(String, Value)> = parts.into_iter().flatten().collect();
    let refs: Vec<(&str, Value)> = owned.iter().map(|(p, v)| (p.as_str(), v.clone())).collect();
    Tree::new(&refs)
}

fn err_code(result: Result<PathBuf, VelyrError>) -> (ErrorCode, String) {
    let err = result.unwrap_err();
    (err.code(), err.message().to_owned())
}

fn same(a: &Path, b: &Path) -> bool {
    dunce::canonicalize(a).unwrap() == dunce::canonicalize(b).unwrap()
}

#[test]
fn exports_string_sugar() {
    let t = tree(vec![pkg(
        "p",
        json!({"exports": "./lib/i.js"}),
        &["lib/i.js"],
    )]);
    assert!(same(
        &t.resolve("p").unwrap(),
        &t.at("node_modules/p/lib/i.js")
    ));
}

#[test]
fn exports_subpaths_and_unexported_subpath() {
    let t = tree(vec![pkg(
        "p",
        json!({"exports": {".": "./main.js", "./feature.js": "./src/feature.js"}}),
        &["main.js", "src/feature.js", "internal/x.js"],
    )]);
    assert!(same(
        &t.resolve("p/feature.js").unwrap(),
        &t.at("node_modules/p/src/feature.js")
    ));
    let (code, message) = err_code(t.resolve("p/internal/x.js"));
    assert_eq!(code, ErrorCode::NotFound);
    assert!(message.contains("'./internal/x.js'"), "{message}");
    assert!(message.contains("exports"), "{message}");
    assert!(message.contains("node_modules/p"), "{message}");
}

#[test]
fn pkg_internal_is_not_exported() {
    let t = tree(vec![pkg(
        "pkg",
        json!({"exports": "./index.js"}),
        &["index.js", "internal.js"],
    )]);
    let (code, message) = err_code(t.resolve("pkg/internal"));
    assert_eq!(code, ErrorCode::NotFound);
    assert!(message.contains("pkg"), "{message}");
    assert!(message.contains("./internal"), "{message}");
}

#[test]
fn subpath_patterns_with_null_exclusion() {
    let t = tree(vec![pkg(
        "p",
        json!({"exports": {
            "./features/*.js": "./src/features/*.js",
            "./features/private-internal/*": null
        }}),
        &[
            "src/features/x.js",
            "src/features/y/z.js",
            "src/features/private-internal/a.js",
        ],
    )]);
    assert!(same(
        &t.resolve("p/features/x.js").unwrap(),
        &t.at("node_modules/p/src/features/x.js")
    ));
    assert!(same(
        &t.resolve("p/features/y/z.js").unwrap(),
        &t.at("node_modules/p/src/features/y/z.js")
    ));
    let (code, _) = err_code(t.resolve("p/features/private-internal/a.js"));
    assert_eq!(code, ErrorCode::NotFound);
}

#[test]
fn longest_pattern_prefix_wins() {
    let t = tree(vec![pkg(
        "p",
        json!({"exports": {"./a/*": "./generic/*.js", "./a/b/*": "./specific/*.js"}}),
        &["generic/b/c.js", "specific/c.js"],
    )]);
    assert!(same(
        &t.resolve("p/a/b/c").unwrap(),
        &t.at("node_modules/p/specific/c.js")
    ));
}

#[test]
fn conditions_import_default_and_velyr() {
    let t = tree(vec![pkg(
        "p",
        json!({"exports": {"require": "./c.cjs", "velyr": "./velyr.js", "import": "./i.js", "default": "./d.js"}}),
        &["c.cjs", "velyr.js", "i.js", "d.js"],
    )]);
    assert!(same(
        &t.resolve("p").unwrap(),
        &t.at("node_modules/p/velyr.js")
    ));

    let t = tree(vec![pkg(
        "p",
        json!({"exports": {"require": "./c.cjs", "import": "./i.js", "default": "./d.js"}}),
        &["c.cjs", "i.js", "d.js"],
    )]);
    assert!(same(&t.resolve("p").unwrap(), &t.at("node_modules/p/i.js")));

    let t = tree(vec![pkg(
        "p",
        json!({"exports": {"browser": "./b.js", "default": "./d.js"}}),
        &["b.js", "d.js"],
    )]);
    assert!(same(&t.resolve("p").unwrap(), &t.at("node_modules/p/d.js")));
}

#[test]
fn nested_conditions_and_node_condition_only_when_enabled() {
    let manifest = json!({"exports": {".": {"import": {"node": "./n.js", "default": "./d.js"}}}});
    let t = tree(vec![pkg("p", manifest, &["n.js", "d.js"])]);
    assert!(same(&t.resolve("p").unwrap(), &t.at("node_modules/p/d.js")));
    let node = NpmResolver::new().with_node_condition();
    let got = node.resolve_bare("p", &t.at("main.mjs")).unwrap();
    assert!(same(&got, &t.at("node_modules/p/n.js")));
}

#[test]
fn null_condition_blocks_later_conditions_but_array_falls_back() {
    let t = tree(vec![pkg(
        "p",
        json!({"exports": {"import": null, "default": "./d.js"}}),
        &["d.js"],
    )]);
    assert_eq!(err_code(t.resolve("p")).0, ErrorCode::NotFound);

    let t = tree(vec![pkg(
        "p",
        json!({"exports": ["../escape.js", "./ok.js"]}),
        &["ok.js"],
    )]);
    assert!(same(
        &t.resolve("p").unwrap(),
        &t.at("node_modules/p/ok.js")
    ));
}

#[test]
fn invalid_targets_are_rejected() {
    for target in [
        "../escape.js",
        "./a/../../escape.js",
        "./node_modules/x.js",
        "nope.js",
    ] {
        let t = tree(vec![pkg("p", json!({"exports": target}), &[])]);
        assert_eq!(
            err_code(t.resolve("p")).0,
            ErrorCode::InvalidData,
            "{target}"
        );
    }
}

#[test]
fn mixed_exports_keys_are_invalid_config() {
    let t = tree(vec![pkg(
        "p",
        json!({"exports": {".": "./a.js", "import": "./b.js"}}),
        &["a.js"],
    )]);
    assert_eq!(err_code(t.resolve("p")).0, ErrorCode::InvalidData);
}

#[test]
fn exported_file_that_does_not_exist_is_not_found() {
    let t = tree(vec![pkg("p", json!({"exports": "./missing.js"}), &[])]);
    let (code, message) = err_code(t.resolve("p"));
    assert_eq!(code, ErrorCode::NotFound);
    assert!(message.contains("missing.js"), "{message}");
}

#[test]
fn imports_hash_specifiers() {
    let mut files = pkg(
        "app",
        json!({"imports": {
            "#internal/*.js": "./src/internal/*.js",
            "#dep": {"node": "dep-node-polyfill", "default": "./dep-default.js"},
            "#ext": "other"
        }}),
        &["src/internal/x.js", "dep-default.js"],
    );
    files.extend(pkg("other", json!({"exports": "./o.js"}), &["o.js"]));
    let t = tree(vec![files]);
    let referrer = t.at("node_modules/app/src/main.mjs");
    let r = NpmResolver::new();
    let got = r.resolve_bare("#internal/x.js", &referrer).unwrap();
    assert!(same(&got, &t.at("node_modules/app/src/internal/x.js")));
    let got = r.resolve_bare("#dep", &referrer).unwrap();
    assert!(same(&got, &t.at("node_modules/app/dep-default.js")));
    let got = r.resolve_bare("#ext", &referrer).unwrap();
    assert!(same(&got, &t.at("node_modules/other/o.js")));

    let (code, message) = err_code(r.resolve_bare("#missing", &referrer));
    assert_eq!(code, ErrorCode::NotFound);
    assert!(message.contains("#missing"), "{message}");
    assert_eq!(
        err_code(r.resolve_bare("#", &referrer)).0,
        ErrorCode::InvalidData
    );
    assert_eq!(
        err_code(r.resolve_bare("#/x", &referrer)).0,
        ErrorCode::InvalidData
    );
}

#[test]
fn self_reference_by_package_name() {
    let t = Tree::new(&[
        (
            "package.json",
            json!({"name": "me", "type": "module", "exports": {".": "./index.js", "./sub": "./lib/sub.js"}}),
        ),
        ("index.js", json!("")),
        ("lib/sub.js", json!("")),
        ("lib/deep/user.mjs", json!("")),
    ]);
    let referrer = t.at("lib/deep/user.mjs");
    let r = NpmResolver::new();
    assert!(same(
        &r.resolve_bare("me", &referrer).unwrap(),
        &t.at("index.js")
    ));
    assert!(same(
        &r.resolve_bare("me/sub", &referrer).unwrap(),
        &t.at("lib/sub.js")
    ));
    assert_eq!(
        err_code(r.resolve_bare("me/nope", &referrer)).0,
        ErrorCode::NotFound
    );
}

#[test]
fn self_reference_needs_exports() {
    let t = Tree::new(&[
        (
            "package.json",
            json!({"name": "me", "type": "module", "main": "m.js"}),
        ),
        ("m.js", json!("")),
    ]);
    let (code, message) = err_code(NpmResolver::new().resolve_bare("me", &t.at("a.mjs")));
    assert_eq!(code, ErrorCode::NotFound);
    assert!(message.contains("'me'"), "{message}");
}

#[test]
fn legacy_main_and_index_fallbacks() {
    let t = tree(vec![
        pkg("withmain", json!({"main": "lib/entry"}), &["lib/entry.js"]),
        pkg("dirmain", json!({"main": "lib"}), &["lib/index.js"]),
        pkg("plain", json!({}), &["index.js", "sub/file.js"]),
        pkg("badmain", json!({"main": "gone.js"}), &["index.js"]),
    ]);
    assert!(same(
        &t.resolve("withmain").unwrap(),
        &t.at("node_modules/withmain/lib/entry.js")
    ));
    assert!(same(
        &t.resolve("dirmain").unwrap(),
        &t.at("node_modules/dirmain/lib/index.js")
    ));
    assert!(same(
        &t.resolve("plain").unwrap(),
        &t.at("node_modules/plain/index.js")
    ));
    assert!(same(
        &t.resolve("plain/sub/file.js").unwrap(),
        &t.at("node_modules/plain/sub/file.js")
    ));
    assert!(same(
        &t.resolve("badmain").unwrap(),
        &t.at("node_modules/badmain/index.js")
    ));
}

#[test]
fn scoped_packages_and_walking_up() {
    let mut files = pkg(
        "@s/lib",
        json!({"exports": {".": "./i.js", "./x": "./x.js"}}),
        &["i.js", "x.js"],
    );
    files.push(("deep/er/main.mjs".into(), json!("")));
    let t = tree(vec![files]);
    let referrer = t.at("deep/er/main.mjs");
    let r = NpmResolver::new();
    assert!(same(
        &r.resolve_bare("@s/lib", &referrer).unwrap(),
        &t.at("node_modules/@s/lib/i.js")
    ));
    assert!(same(
        &r.resolve_bare("@s/lib/x", &referrer).unwrap(),
        &t.at("node_modules/@s/lib/x.js")
    ));
}

#[test]
fn nearest_node_modules_shadows_outer() {
    let mut files = pkg("p", json!({"exports": "./o.js"}), &["o.js"]);
    files.extend(
        pkg("p", json!({"exports": "./i.js"}), &["i.js"])
            .into_iter()
            .map(|(path, v)| (format!("inner/{path}"), v)),
    );
    let t = tree(vec![files]);
    let got = NpmResolver::new()
        .resolve_bare("p", &t.at("inner/a.mjs"))
        .unwrap();
    assert!(same(&got, &t.at("inner/node_modules/p/i.js")));
}

#[test]
fn missing_package_and_invalid_names() {
    let t = Tree::new(&[("main.mjs", json!(""))]);
    let (code, message) = err_code(t.resolve("ghost"));
    assert_eq!(code, ErrorCode::NotFound);
    assert!(message.contains("'ghost'"), "{message}");
    for bad in ["", "@scope", ".hidden", "a\\b", "a%b"] {
        assert_eq!(
            err_code(t.resolve(bad)).0,
            ErrorCode::InvalidData,
            "{bad:?}"
        );
    }
}

#[test]
fn commonjs_targets_are_unsupported() {
    let t = tree(vec![
        pkg(
            "cjsdefault",
            json!({"type": "commonjs", "exports": "./i.js"}),
            &["i.js"],
        ),
        pkg("notype", json!({"exports": "./i.js"}), &["i.js"])
            .into_iter()
            .map(|(p, v)| {
                if p.ends_with("package.json") {
                    (p, json!({"name": "notype", "exports": "./i.js"}))
                } else {
                    (p, v)
                }
            })
            .collect(),
        pkg("dotcjs", json!({"exports": "./i.cjs"}), &["i.cjs"]),
        pkg(
            "dotmjs",
            json!({"type": "commonjs", "exports": "./i.mjs"}),
            &["i.mjs"],
        ),
    ]);
    for name in ["cjsdefault", "notype", "dotcjs"] {
        let (code, message) = err_code(t.resolve(name));
        assert_eq!(code, ErrorCode::Unsupported, "{name}");
        assert!(message.contains(name), "{message}");
    }
    assert!(same(
        &t.resolve("dotmjs").unwrap(),
        &t.at("node_modules/dotmjs/i.mjs")
    ));
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn commonjs_packages_resolve_when_allowed() {
    let t = tree(vec![
        pkg(
            "cjs-pkg",
            json!({"type": "commonjs", "main": "./lib/index.js"}),
            &["lib/index.js"],
        ),
        pkg("dot-cjs", json!({"exports": "./i.cjs"}), &["i.cjs"]),
    ]);
    let referrer = t.at("main.mjs");
    let relaxed = NpmResolver::new().allow_commonjs();
    assert!(same(
        &relaxed.resolve_bare("cjs-pkg", &referrer).unwrap(),
        &t.at("node_modules/cjs-pkg/lib/index.js")
    ));
    assert!(same(
        &relaxed.resolve_bare("dot-cjs", &referrer).unwrap(),
        &t.at("node_modules/dot-cjs/i.cjs")
    ));
    // The default stays ESM-only.
    assert_eq!(err_code(t.resolve("cjs-pkg")).0, ErrorCode::Unsupported);
}

#[test]
fn commonjs_resolution_matches_the_node_condition_like_node_does() {
    // The shape of rxjs: a `node` build and extensionless browser builds behind other conditions.
    let manifest = json!({"type": "commonjs", "exports": {"./operators": {
        "node": "./cjs/operators.js", "es2015": "./esm/operators.js", "default": "./esm5/operators.js"}}});
    let t = tree(vec![pkg(
        "rx",
        manifest,
        &["cjs/operators.js", "esm/operators.js", "esm5/operators.js"],
    )]);
    let referrer = t.at("main.mjs");
    let linked = NpmResolver::new().allow_commonjs().with_node_condition();
    assert!(same(
        &linked.resolve_bare("rx/operators", &referrer).unwrap(),
        &t.at("node_modules/rx/cjs/operators.js")
    ));
}

#[test]
fn conditions_are_configurable() {
    let t = tree(vec![pkg(
        "dual",
        json!({"exports": {".": {"import": "./i.mjs", "require": "./r.cjs", "default": "./d.js"}}}),
        &["i.mjs", "r.cjs", "d.js"],
    )]);
    let referrer = t.at("main.mjs");
    let pick = |resolver: NpmResolver| resolver.resolve_bare("dual", &referrer).unwrap();
    let require = NpmResolver::new()
        .with_conditions(&["velyr", "require", "node", "default"])
        .allow_commonjs();
    assert!(same(&pick(require), &t.at("node_modules/dual/r.cjs")));
    let import = NpmResolver::new().with_conditions(&["import"]);
    assert!(same(&pick(import), &t.at("node_modules/dual/i.mjs")));
    let neither = NpmResolver::new().with_conditions(&["other"]);
    assert!(same(&pick(neither), &t.at("node_modules/dual/d.js")));
    assert!(same(
        &pick(NpmResolver::new()),
        &t.at("node_modules/dual/i.mjs")
    ));
}

#[test]
fn public_matchers_resolve_exports_and_imports() {
    use velyr_compat_npm::resolve::exports::{match_exports, match_imports};
    let dir = Path::new("/pkg");
    let exports =
        json!({".": {"require": "./r.js", "import": "./i.js"}, "./f/*": "./lib/*.js", "./x": null});
    let req = strings(&["require"]);
    let get = |sub: &str, conds: &[String]| match_exports(&exports, sub, conds, dir).unwrap();
    assert_eq!(get(".", &req), Some(dir.join("r.js")));
    assert_eq!(get(".", &strings(&["import"])), Some(dir.join("i.js")));
    assert_eq!(get("./f/a", &req), Some(dir.join("lib/a.js")));
    assert_eq!(get("./x", &req), None);
    assert_eq!(get("./nope", &req), None);
    assert!(match_exports(&json!({".": "./a", "b": 1}), ".", &req, dir).is_err());

    let imports = json!({"#a": {"require": "./a-req.js", "default": "./a.js"}, "#ext": "other"});
    let external = |name: &str| Ok(PathBuf::from(format!("/ext/{name}")));
    assert_eq!(
        match_imports(&imports, "#a", &req, dir, &external).unwrap(),
        Some(dir.join("a-req.js"))
    );
    assert_eq!(
        match_imports(&imports, "#ext", &req, dir, &external).unwrap(),
        Some(PathBuf::from("/ext/other"))
    );
    assert_eq!(
        match_imports(&imports, "#none", &req, dir, &external).unwrap(),
        None
    );
}
