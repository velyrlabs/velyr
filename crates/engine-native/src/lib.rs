//! Native engine (ADR-009, ADR-012): Oxc front-end, register bytecode compiler and the
//! `velyr-engine-vm` interpreter. Constructs the bytecode compiler rejects as
//! `unsupported` fall back to the bootstrap stack compiler and tree VM (`compiler.rs`,
//! `vm.rs`). Promises run on the VM job queue; ES modules link and evaluate
//! in the VM (`modules/`).

// `compiler.rs` is the bootstrap compiler, so the bytecode compiler lives under `compiler/`.
#[path = "compiler/bytecode/mod.rs"]
pub mod bytecode_compiler;
mod compiler;
#[path = "builtins/date.rs"]
mod date;
#[path = "builtins/intl/mod.rs"]
mod intl;
mod isolate;
#[path = "builtins/iterator_helpers.rs"]
pub mod iterator_helpers;
#[path = "builtins/temporal/mod.rs"]
mod temporal;
mod modules;
mod number;
mod value_bridge;
mod vm;

use velyr_engine_api::*;

pub use isolate::NativeIsolate;

#[derive(Debug, Clone, Copy, Default)]
pub struct NativeEngine;

impl Engine for NativeEngine {
    type Isolate = NativeIsolate;

    fn features(&self) -> EngineFeatures {
        EngineFeatures::PROMISES | EngineFeatures::MODULES | EngineFeatures::CONTINUATION_CONTEXT
    }

    /// `heap_limit_bytes` bounds the GC heap, BigInt sizes and ArrayBuffer backing stores.
    fn create_isolate(&self, opts: IsolateOptions) -> Result<NativeIsolate, EngineError> {
        let mut isolate = NativeIsolate::new();
        if let Some(bytes) = opts.heap_limit_bytes {
            isolate.vm.limit_heap(bytes);
            isolate.vm.limit_bigints_to_heap_budget(bytes);
            isolate.vm.limit_buffers_to_heap_budget(bytes);
        }
        Ok(isolate)
    }
}
