//! Classic scripts: compile and, optionally, run.

use velyr_engine_api::{JsException, Source, Value};

use crate::convert::{exception_from, from_v8, internal_exception};
use crate::shared::Shared;

/// Origin of a classic script: `name` is what its stack frames show.
pub(crate) fn script_origin<'s>(
    scope: &v8::PinScope<'s, '_>,
    name: v8::Local<'s, v8::String>,
) -> v8::ScriptOrigin<'s> {
    v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    )
}

/// Compiles `source` as a classic script and, when `run`, runs it; `undefined` if not run.
pub(crate) fn script(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    source: &Source,
    run: bool,
) -> Result<Value, JsException> {
    v8::tc_scope!(let tc, scope);
    let (Some(name), Some(text)) = (
        v8::String::new(tc, &source.name),
        v8::String::new(tc, &source.text),
    ) else {
        return Err(internal_exception("source too long"));
    };
    let origin = script_origin(tc, name);
    let Some(compiled) = v8::Script::compile(tc, text, Some(&origin)) else {
        return Err(exception_from(tc, shared));
    };
    if !run {
        return Ok(Value::Undefined);
    }
    shared.microtasks_dirty.set(true);
    match compiled.run(tc) {
        Some(value) => Ok(from_v8(tc, &mut shared.handles.borrow_mut(), value)),
        None => Err(exception_from(tc, shared)),
    }
}
