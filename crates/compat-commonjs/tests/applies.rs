//! `CommonJsTransform::applies`: only files that need a facade are rewritten.

use std::path::Path;

use velyr_compat_commonjs::CommonJsTransform;
use velyr_runtime_modules::{ModuleId, SourceTransform};

fn applies(path: &Path, text: &str) -> bool {
    let transform = CommonJsTransform::new(Default::default());
    transform.applies(&ModuleId::from_path(path).unwrap(), text)
}

#[test]
fn embedded_paths_are_never_transformed() {
    let helper = "/$velyr/oxc-runtime/helpers/decorate.js";
    assert!(!applies(Path::new(helper), "module.exports = {};"));
}

#[test]
fn only_commonjs_and_json_need_a_facade() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    assert!(applies(&root.join("a.js"), "module.exports = 1;"));
    assert!(applies(&root.join("a.json"), "{}"));
    assert!(!applies(&root.join("a.js"), "export const a = 1;"));
    assert!(!applies(&root.join("a.mjs"), "module.exports = 1;"));
}
