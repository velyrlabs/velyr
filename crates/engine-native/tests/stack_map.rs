//! Stack frames mapped through the host's `StackFrameMapper` on engine-native.

use std::sync::Arc;

use velyr_engine_api::{
    Engine, HostError, Isolate, IsolateOptions, MappedPosition, ModuleResolveHook, Source, Value,
};
use velyr_engine_native::NativeEngine;

const MODULE: &str = "file:///a/m.mjs";

struct NoImports;

impl ModuleResolveHook for NoImports {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        Err(HostError::TypeError(format!(
            "unexpected import {specifier}"
        )))
    }
}

fn stack(map: bool) -> String {
    let mut iso = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    if map {
        iso.set_prepare_stack_trace(Arc::new(|file, line, column| {
            (file == MODULE).then(|| MappedPosition {
                file: "file:///a/m.ts".into(),
                line: line + 10,
                column,
            })
        }));
    }
    let source = Source {
        name: MODULE.into(),
        text: "export const e = new Error('x'); globalThis.s = e.stack;".into(),
    };
    let module = iso.compile_module(&source).expect("compile");
    iso.instantiate_module(module, &mut NoImports)
        .expect("link");
    iso.evaluate_module(module).expect("evaluate");
    let value = iso
        .eval_script(&Source {
            name: "probe.js".into(),
            text: "globalThis.s".into(),
        })
        .expect("probe");
    match value {
        Value::String(s) => s.to_string_lossy(),
        other => panic!("stack is not a string: {other:?}"),
    }
}

#[test]
fn frames_are_mapped_to_the_original_position() {
    let stack = stack(true);
    assert!(stack.contains("file:///a/m.ts:11:"), "{stack}");
    assert!(!stack.contains("m.mjs:1:"), "{stack}");
}

#[test]
fn without_a_mapper_positions_are_unchanged() {
    assert!(stack(false).contains("file:///a/m.mjs:1:"));
}
