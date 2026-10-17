//! Engine-agnostic contract between `runtime-core` and a JS engine.
//!
//! Implemented by `engine-v8` and by the `engine-native` bootstrap. Nothing in
//! this crate may expose a concrete engine type or an engine scope lifetime
//! (ADR-009): values are owned primitives or isolate-scoped [`Handle`]s.
//!
//! Threading: an [`Isolate`] is `!Send` and lives on the single thread that
//! created it (ADR-010). The only cross-thread object is [`TerminationHandle`].

mod code_cache;
mod error;
mod features;
mod isolate;
mod native;
mod serialize;
mod stack_trace;
mod value;

pub use code_cache::{
    CodeCacheStats, open as open_code_cache, seal as seal_code_cache, source_digest,
};
pub use error::{EngineError, HostError};
pub use features::EngineFeatures;
pub use isolate::*;
pub use native::{NativeError, NativeScope, NativeScopeFn, NativeTeardown, NativeValue, V8_ENGINE};
pub use serialize::{SerializedValue, TransferredBuffer};
pub use stack_trace::{MappedPosition, StackFrameMapper};
pub use value::{Handle, JsBigInt, JsString, Value};
