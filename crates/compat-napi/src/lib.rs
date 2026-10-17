//! Node-API for native `.node` addons.
//!
//! [`load`] checks `NativeLoad` for the canonical path before `dlopen`, then runs the module's
//! registration. The `napi_*` functions are exported with `#[no_mangle]` so addons resolve them
//! from the host binary (`build.rs` writes the dynamic export list). The runtime calls
//! [`load_native`] and [`shutdown_native`], which take an engine-api `NativeScope`.
//!
//! Direct V8 use: Node-API hands raw handles (`napi_value`) and non-lexical handle scopes to C,
//! which `velyr-engine-api` cannot express. This crate therefore uses `v8` directly for:
//! - `napi_value` <-> `v8::Local` (same bit pattern, see `ffi::to_napi`/`ffi::local`);
//! - heap-allocated handle scopes (`scopes`);
//! - weak handles for wrap/external/finalizer tracking (`env::Env::track`);
//! - function templates for native callbacks and classes (`env`, `api::functions`).

mod api;
mod bridge;
mod env;
mod ffi;
mod loader;
mod scopes;
mod types;

pub use bridge::{load_native, shutdown_native};
pub use loader::{LoadError, authorize, load, shutdown};
