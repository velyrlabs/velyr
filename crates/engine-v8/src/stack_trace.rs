//! `Isolate::set_prepare_stack_trace`: remaps frame positions through a [`StackFrameMapper`].
//!
//! Registering V8's prepare-stack-trace callback disables `Error.prepareStackTrace`, so this
//! module calls the user's function itself when one is defined (V8 compat).

use velyr_engine_api::StackFrameMapper;

struct MapperSlot(StackFrameMapper);

pub(crate) fn install(isolate: &mut v8::Isolate, mapper: StackFrameMapper) {
    isolate.set_slot(MapperSlot(mapper));
    isolate.set_prepare_stack_trace_callback(prepare);
}

fn prepare<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    error: v8::Local<v8::Value>,
    sites: v8::Local<v8::Array>,
) -> v8::Local<'s, v8::Value> {
    if let Some(result) = call_user_hook(scope, error, sites) {
        return result;
    }
    let mut stack = error_header(scope, error);
    for index in 0..sites.length() {
        let Some(site) = sites
            .get_index(scope, index)
            .and_then(|s| v8::Local::<v8::Object>::try_from(s).ok())
        else {
            continue;
        };
        stack.push_str("\n    at ");
        stack.push_str(&frame(scope, site));
    }
    match v8::String::new(scope, &stack) {
        Some(text) => text.into(),
        None => v8::undefined(scope).into(),
    }
}

/// `Some` when the page defined `Error.prepareStackTrace`; its result is the stack as-is.
// shortcut: the hook receives V8's unmapped CallSites, remapping them needs wrapper objects.
fn call_user_hook<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    error: v8::Local<v8::Value>,
    sites: v8::Local<v8::Array>,
) -> Option<v8::Local<'s, v8::Value>> {
    let global = scope.get_current_context().global(scope);
    let ctor_key = v8::String::new(scope, "Error")?;
    let ctor = v8::Local::<v8::Object>::try_from(global.get(scope, ctor_key.into())?).ok()?;
    let hook_key = v8::String::new(scope, "prepareStackTrace")?;
    let hook = v8::Local::<v8::Function>::try_from(ctor.get(scope, hook_key.into())?).ok()?;
    Some(
        hook.call(scope, ctor.into(), &[error, sites.into()])
            .unwrap_or_else(|| v8::undefined(scope).into()),
    )
}

fn error_header(scope: &mut v8::PinScope<'_, '_>, error: v8::Local<v8::Value>) -> String {
    v8::tc_scope!(let tc, scope);
    error
        .to_string(tc)
        .map_or_else(|| "Error".to_owned(), |s| s.to_rust_string_lossy(tc))
}

/// V8's own rendering of the frame, with `file:line:column` swapped for the mapped position.
fn frame(scope: &mut v8::PinScope<'_, '_>, site: v8::Local<v8::Object>) -> String {
    let text = call_method(scope, site, "toString")
        .map(|v| v.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let (Some(file), Some(line), Some(column)) = (
        call_method(scope, site, "getFileName").filter(|v| v.is_string()),
        call_method(scope, site, "getLineNumber").and_then(|v| v.uint32_value(scope)),
        call_method(scope, site, "getColumnNumber").and_then(|v| v.uint32_value(scope)),
    ) else {
        return text;
    };
    let file = file.to_rust_string_lossy(scope);
    let mapper = match scope.get_slot::<MapperSlot>() {
        Some(slot) => slot.0.clone(),
        None => return text,
    };
    match mapper(&file, line, column) {
        Some(to) => text.replace(
            &format!("{file}:{line}:{column}"),
            &format!("{}:{}:{}", to.file, to.line, to.column),
        ),
        None => text,
    }
}

fn call_method<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    v8::tc_scope!(let tc, scope);
    let key = v8::String::new(tc, name)?;
    let method = v8::Local::<v8::Function>::try_from(object.get(tc, key.into())?).ok()?;
    method.call(tc, object.into(), &[])
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use velyr_engine_api::{Engine, Isolate, IsolateOptions, MappedPosition, Source, Value};

    use crate::{V8Engine, V8Isolate};

    fn isolate_mapping_x_js() -> V8Isolate {
        let mut iso = V8Engine::new()
            .create_isolate(IsolateOptions::default())
            .unwrap();
        let mapper = Arc::new(|file: &str, line: u32, _column: u32| {
            file.ends_with("x.js").then(|| MappedPosition {
                file: "orig.ts".into(),
                line: line + 100,
                column: 7,
            })
        });
        super::install(iso.isolate.get_mut(), mapper);
        iso
    }

    fn eval_string(iso: &mut V8Isolate, text: &str) -> String {
        let source = Source {
            name: "file:///x.js".into(),
            text: text.into(),
        };
        match iso.eval_script(&source).unwrap() {
            Value::String(s) => s.to_string_lossy(),
            other => panic!("not a string: {other:?}"),
        }
    }

    #[test]
    fn frames_are_remapped() {
        let mut iso = isolate_mapping_x_js();
        let stack = eval_string(
            &mut iso,
            "function f() { return new Error('boom').stack }\nf()",
        );
        assert!(
            stack.starts_with("Error: boom\n    at f (orig.ts:101:7)"),
            "{stack}"
        );
        assert!(stack.contains("at orig.ts:102:7"), "{stack}");
    }

    #[test]
    fn user_prepare_stack_trace_still_runs() {
        let mut iso = isolate_mapping_x_js();
        let text = "Error.prepareStackTrace = (e, sites) => 'custom:' + sites.length;\nnew Error('x').stack";
        assert_eq!(eval_string(&mut iso, text), "custom:1");
    }
}
