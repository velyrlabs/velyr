//! 1. Exports the Node-API surface (`napi_*`) from the `velyr` binary so `.node` addons resolve
//!    those symbols in the host executable (docs/research/napi-scope.md section 5.3).
//!    `velyr-compat-napi` publishes the platform linker argument through its `links` metadata; the
//!    variable is only set once this crate depends on it directly, so until then this is a no-op.
//! 2. Bakes the startup snapshot (VELYR-232): this profile's host functions and bootstrap scripts,
//!    embedded by `profiles` with `include_bytes!`. Generated here, never at run time: the blob
//!    becomes heap and code, so a file a user could swap would be code execution.

use std::path::PathBuf;

use velyr_engine_api::{Engine, Source};
use velyr_engine_v8::{V8Engine, create_snapshot};

#[path = "src/profiles/bootstrap.rs"]
mod bootstrap;

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_VELYR_NAPI_LINK_ARG");
    if let Ok(arg) = std::env::var("DEP_VELYR_NAPI_LINK_ARG") {
        println!("cargo:rustc-link-arg-bins={arg}");
    }
    write_snapshot();
}

/// An empty file means "no snapshot": the CLI then boots every isolate the cold way.
fn write_snapshot() {
    println!("cargo:rerun-if-changed=src/profiles/bootstrap.rs");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    // The blob is tied to the architecture it was made on: only a native build can embed it.
    let native = std::env::var("HOST").ok() == std::env::var("TARGET").ok();
    let blob = if native { snapshot() } else { Vec::new() };
    std::fs::write(out.join("startup.snapshot"), blob).expect("write the startup snapshot");
}

fn snapshot() -> Vec<u8> {
    let features = V8Engine::new().features();
    let mut scripts = vec![Source {
        name: "<bootstrap>".into(),
        text: velyr_runtime_ops::BOOTSTRAP_JS.into(),
    }];
    scripts.extend(bootstrap::scripts(features));
    create_snapshot(&bootstrap::host_functions(features), &scripts)
        .unwrap_or_else(|err| panic!("the bootstrap scripts must snapshot cleanly: {err}"))
}
