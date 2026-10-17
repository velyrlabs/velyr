//! Exports the Node-API surface (`napi_*`) from the `velyr` binary so `.node` addons resolve
//! those symbols in the host executable (docs/research/napi-scope.md section 5.3).
//!
//! `velyr-compat-napi` publishes the platform linker argument through its `links` metadata; the
//! variable is only set once this crate depends on it directly, so until then this is a no-op.

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_VELYR_NAPI_LINK_ARG");
    if let Ok(arg) = std::env::var("DEP_VELYR_NAPI_LINK_ARG") {
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
