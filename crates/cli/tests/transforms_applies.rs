//! The real transform chain must not claim plain ESM it would only copy.

#![allow(dead_code)]

// transforms.rs reads `crate::config`, so the harness carries the modules it needs. Telemetry and
// the node facade are stubbed: the real ones pull in the whole binary.
#[path = "../src/config/mod.rs"]
mod config;
#[path = "../src/permissions/mod.rs"]
mod permissions;
#[path = "../src/transforms.rs"]
mod transforms;

mod telemetry {
    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    pub struct TelemetryConfig {
        pub endpoint: Option<String>,
        pub service_name: Option<String>,
    }
}

#[cfg(feature = "node")]
mod node {
    pub fn facade_dir() -> std::io::Result<&'static std::path::Path> {
        Err(std::io::ErrorKind::NotFound.into())
    }
}

use velyr_runtime_modules::ModuleId;

#[test]
fn chain_skips_plain_javascript() {
    let chain = transforms::source_transform(&Default::default(), Default::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let esm = ModuleId::from_path(&root.join("a.js")).unwrap();
    assert!(!chain.applies(&esm, "export const a = 1;"));
    assert!(chain.applies(&esm, "module.exports = 1;"));
}
