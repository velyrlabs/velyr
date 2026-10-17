//! Continuation context: V8's continuation-preserved embedder data, which V8 captures when a
//! promise reaction (or `await`) is registered and restores while it runs.

use velyr_engine_api::Value;

use crate::convert::{from_v8, to_v8};
use crate::shared::Shared;

pub(crate) fn set(scope: &mut v8::PinScope<'_, '_>, shared: &Shared, value: &Value) {
    // A stale handle cannot be converted; leaving the context as it was beats clearing it.
    if let Ok(data) = to_v8(scope, &shared.handles.borrow(), value) {
        scope.set_continuation_preserved_embedder_data(data);
    }
}

pub(crate) fn get(scope: &mut v8::PinScope<'_, '_>, shared: &Shared) -> Value {
    let data = scope.get_continuation_preserved_embedder_data();
    from_v8(scope, &mut shared.handles.borrow_mut(), data)
}

#[cfg(test)]
mod tests {
    use velyr_engine_api::{
        CallContext, Engine, EngineFeatures, HostError, HostFunctionDef, Isolate, IsolateOptions,
        Source, Value,
    };

    use crate::V8Engine;

    fn set(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
        ctx.set_continuation_context(args.first().cloned().unwrap_or(Value::Undefined));
        Ok(Value::Undefined)
    }

    fn get(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
        Ok(ctx.get_continuation_context())
    }

    fn eval(iso: &mut impl Isolate, text: &str) -> Value {
        let source = Source {
            name: "continuation.js".into(),
            text: text.into(),
        };
        iso.eval_script(&source).expect("eval")
    }

    #[test]
    fn the_engine_declares_the_feature() {
        assert!(
            V8Engine::new()
                .features()
                .contains(EngineFeatures::CONTINUATION_CONTEXT)
        );
    }

    #[test]
    fn reactions_and_await_see_the_context_of_their_registration() {
        let mut iso = V8Engine::new()
            .create_isolate(IsolateOptions::default())
            .unwrap();
        for (name, arity, callback) in [("setCtx", 1, set as _), ("getCtx", 0, get as _)] {
            iso.register_host_function(HostFunctionDef {
                name,
                arity,
                callback,
            })
            .unwrap();
        }
        eval(
            &mut iso,
            "globalThis.seen = [];
             setCtx('A');
             Promise.resolve().then(() => seen.push('then:' + getCtx()));
             (async () => { await null; seen.push('await:' + getCtx()); })();
             setCtx('B');
             Promise.resolve().then(() => seen.push('later:' + getCtx()));
             setCtx(undefined);",
        );
        iso.perform_microtask_checkpoint();
        let Value::String(seen) = eval(&mut iso, "seen.join(',')") else {
            panic!("not a string");
        };
        assert_eq!(seen.to_string_lossy(), "then:A,await:A,later:B");
    }
}
