use std::fmt::Debug;

use velyr_engine_api::{
    CallContext, Engine, EngineFeatures, HostError, HostFunctionDef, Isolate, IsolateOptions,
    JsString, MicrotaskProgress, ModuleResolveHook, PromiseState, Source, Value,
};

use crate::Case;

type CaseResult = Result<(), String>;

pub(crate) fn all<E: Engine>() -> Vec<Case<E>> {
    let plain = |name, run| Case {
        name,
        requires: None,
        run,
    };
    let gated = |name, label, flag, run| Case {
        name,
        requires: Some((label, flag)),
        run,
    };
    vec![
        plain("eval_primitive", eval_primitive::<E>),
        plain("utf16_strings", utf16_strings::<E>),
        plain("exceptions", exceptions::<E>),
        plain("host_function", host_function::<E>),
        plain("release_handle", release_handle::<E>),
        plain("scope", scope::<E>),
        gated(
            "promise_checkpoint",
            "PROMISES",
            EngineFeatures::PROMISES,
            promise_checkpoint::<E>,
        ),
        gated(
            "module_import",
            "MODULES",
            EngineFeatures::MODULES,
            module_import::<E>,
        ),
    ]
}

fn src(name: &str, text: &str) -> Source {
    Source {
        name: name.into(),
        text: text.into(),
    }
}

fn isolate<E: Engine>(engine: &E) -> Result<E::Isolate, String> {
    engine
        .create_isolate(IsolateOptions::default())
        .map_err(|e| format!("create_isolate: {e}"))
}

fn eval(iso: &mut impl Isolate, text: &str) -> Result<Value, String> {
    iso.eval_script(&src("conformance.js", text))
        .map_err(|e| format!("eval `{text}`: {}", e.message))
}

fn expect_eq<T: Debug + PartialEq>(what: &str, actual: T, expected: T) -> CaseResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}: expected {expected:?}, got {actual:?}"))
    }
}

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

fn utf16(units: &[u16]) -> Value {
    Value::String(JsString::from_utf16(units.to_vec()))
}

fn native_add(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    match args {
        [Value::Number(a), Value::Number(b)] => Ok(Value::Number(a + b)),
        _ => Err(HostError::TypeError("expected two numbers".into())),
    }
}

fn register_add(iso: &mut impl Isolate) -> CaseResult {
    iso.register_host_function(HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    })
    .map_err(|e| format!("register nativeAdd: {e}"))
}

fn eval_primitive<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    expect_eq("20 + 22", eval(&mut iso, "20 + 22")?, Value::Number(42.0))?;
    expect_eq("string concat", eval(&mut iso, "'a' + 1")?, text("a1"))?;
    expect_eq(
        "let completion",
        eval(&mut iso, "let a = 2; a * 21")?,
        Value::Number(42.0),
    )
}

fn echo(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    Ok(args.first().cloned().unwrap_or(Value::Undefined))
}

fn lone_surrogate(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(ctx.create_string(JsString::from_utf16(vec![0x61, 0xD800, 0x62])))
}

fn utf16_strings<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    for (name, callback) in [
        ("echo", echo as velyr_engine_api::HostFn),
        ("lone", lone_surrogate),
    ] {
        let def = HostFunctionDef {
            name,
            arity: 0,
            callback,
        };
        iso.register_host_function(def)
            .map_err(|e| format!("register {name}: {e}"))?;
    }
    expect_eq(
        "lone surrogate from host",
        eval(&mut iso, "lone()")?,
        utf16(&[0x61, 0xD800, 0x62]),
    )?;
    expect_eq(
        "lone surrogate survives concat",
        eval(&mut iso, "lone() + 'c'")?,
        utf16(&[0x61, 0xD800, 0x62, 0x63]),
    )?;
    let echo_fn = eval(&mut iso, "echo")?;
    for input in [
        utf16(&[0xD83D]),
        Value::String(JsString::from("h\u{1F600}")),
    ] {
        let out = iso
            .call(&echo_fn, &Value::Undefined, std::slice::from_ref(&input))
            .map_err(|e| format!("call echo: {}", e.message))?;
        expect_eq("string round trip", out, input)?;
    }
    Ok(())
}

fn exceptions<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    register_add(&mut iso)?;
    for (script, kind) in [
        ("nope", "ReferenceError"),
        ("let = ;", "SyntaxError"),
        ("nativeAdd('x')", "TypeError"),
    ] {
        let err = iso
            .eval_script(&src("conformance.js", script))
            .err()
            .ok_or_else(|| format!("`{script}` should throw"))?;
        if !err.message.contains(kind) {
            return Err(format!(
                "`{script}`: expected {kind}, got `{}`",
                err.message
            ));
        }
    }
    expect_eq(
        "usable after throw",
        eval(&mut iso, "1 + 1")?,
        Value::Number(2.0),
    )
}

fn host_function<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    register_add(&mut iso)?;
    expect_eq(
        "nativeAdd(20, 22)",
        eval(&mut iso, "nativeAdd(20, 22)")?,
        Value::Number(42.0),
    )?;
    let again = iso.register_host_function(HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    });
    if again.is_ok() {
        return Err("duplicate host function name must fail".into());
    }
    let func = eval(&mut iso, "nativeAdd")?;
    let sum = iso
        .call(
            &func,
            &Value::Undefined,
            &[Value::Number(1.0), Value::Number(2.0)],
        )
        .map_err(|e| format!("call: {}", e.message))?;
    expect_eq("call(nativeAdd)", sum, Value::Number(3.0))
}

fn call_returns<I: Isolate>(iso: &mut I, func: &Value, expected: f64) -> CaseResult {
    let args = [Value::Number(1.0), Value::Number(2.0)];
    let out = iso
        .call(func, &Value::Undefined, &args)
        .map_err(|e| format!("call: {}", e.message))?;
    expect_eq("call result", out, Value::Number(expected))
}

fn release_handle<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    register_add(&mut iso)?;
    let Value::Object(first) = eval(&mut iso, "nativeAdd")? else {
        return Err("a function must be an Object handle".into());
    };
    call_returns(&mut iso, &Value::Object(first), 3.0)?;
    iso.release(first);
    let second = eval(&mut iso, "nativeAdd")?;
    // Stale and double release are ignored and must not free a newer handle.
    iso.release(first);
    call_returns(&mut iso, &second, 3.0)
}

fn scope<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    register_add(&mut iso)?;
    let inside = iso.scope(|iso| {
        let func = eval(iso, "nativeAdd")?;
        call_returns(iso, &func, 3.0)?;
        Ok::<_, String>(42.0)
    })?;
    expect_eq("scope result", inside, 42.0)?;
    expect_eq(
        "usable after scope",
        eval(&mut iso, "nativeAdd(1, 2)")?,
        Value::Number(3.0),
    )
}

fn promise_checkpoint<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    let (promise, resolver) = iso.create_promise();
    expect_eq(
        "new promise",
        iso.promise_state(&promise),
        Some(PromiseState::Pending),
    )?;
    iso.resolve(resolver, Value::Number(5.0));
    iso.resolve(resolver, Value::Number(6.0));
    expect_eq(
        "settled once",
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Number(5.0))),
    )?;
    eval(
        &mut iso,
        "globalThis.x = 0; Promise.resolve().then(() => { globalThis.x = 1; });",
    )?;
    expect_eq(
        "then waits for checkpoint",
        eval(&mut iso, "globalThis.x")?,
        Value::Number(0.0),
    )?;
    expect_eq(
        "first checkpoint",
        iso.perform_microtask_checkpoint(),
        MicrotaskProgress::Progressed,
    )?;
    expect_eq(
        "second checkpoint",
        iso.perform_microtask_checkpoint(),
        MicrotaskProgress::Idle,
    )?;
    expect_eq(
        "then ran",
        eval(&mut iso, "globalThis.x")?,
        Value::Number(1.0),
    )
}

struct OneModule(Source);

impl ModuleResolveHook for OneModule {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        if specifier == self.0.name {
            Ok(self.0.clone())
        } else {
            Err(HostError::TypeError(format!(
                "module `{specifier}` not found"
            )))
        }
    }
}

fn module_import<E: Engine>(engine: &E) -> CaseResult {
    let mut iso = isolate(engine)?;
    let mut hook = OneModule(src("b.js", "export const b = 41;"));
    let entry = src("a.js", "import { b } from 'b.js'; globalThis.out = b + 1;");
    let js = |e: velyr_engine_api::JsException| e.message;
    let module = iso.compile_module(&entry).map_err(js)?;
    iso.instantiate_module(module, &mut hook).map_err(js)?;
    let promise = iso.evaluate_module(module).map_err(js)?;
    iso.perform_microtask_checkpoint();
    expect_eq(
        "evaluate promise",
        iso.promise_state(&promise),
        Some(PromiseState::Fulfilled(Value::Undefined)),
    )?;
    expect_eq("import result", eval(&mut iso, "out")?, Value::Number(42.0))
}
