//! The real transform chain must not claim plain ESM it would only copy.

#![allow(dead_code)]

#[path = "../src/transforms.rs"]
mod transforms;

use velyr_runtime_modules::{ModuleId, SourceTransform};

#[test]
fn chain_skips_plain_javascript() {
    let chain = transforms::source_transform(&Default::default(), Default::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let esm = ModuleId::from_path(&root.join("a.js")).unwrap();
    assert!(!chain.applies(&esm, "export const a = 1;"));
    assert!(chain.applies(&esm, "module.exports = 1;"));
}
