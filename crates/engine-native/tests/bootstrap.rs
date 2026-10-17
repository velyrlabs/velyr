use velyr_engine_api::*;
use velyr_engine_native::NativeEngine;
use velyr_runtime_errors::ErrorCode;

fn isolate() -> impl Isolate {
    NativeEngine
        .create_isolate(IsolateOptions::default())
        .unwrap()
}

fn src(text: &str) -> Source {
    Source {
        name: "test.js".into(),
        text: text.into(),
    }
}

fn eval(text: &str) -> Result<Value, JsException> {
    isolate().eval_script(&src(text))
}

fn error_message(text: &str) -> String {
    eval(text).unwrap_err().message
}

fn native_add(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    match args {
        [Value::Number(a), Value::Number(b)] => Ok(Value::Number(a + b)),
        _ => Err(HostError::TypeError("nativeAdd expects two numbers".into())),
    }
}

fn str_value(s: &str) -> Value {
    Value::String(JsString::from_str(s))
}

#[test]
fn host_function_native_add() {
    let mut iso = isolate();
    iso.register_host_function(HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    })
    .unwrap();
    assert_eq!(
        iso.eval_script(&src("nativeAdd(20, 22)")).unwrap(),
        Value::Number(42.0)
    );
}

#[test]
fn let_binding_and_multiplication() {
    assert_eq!(eval("let a = 2; a * 21").unwrap(), Value::Number(42.0));
}

#[test]
fn string_concatenation_with_number() {
    assert_eq!(eval("'a' + 1").unwrap(), str_value("a1"));
}

#[test]
fn number_to_string_follows_spec() {
    assert_eq!(eval("'' + 1/3").unwrap(), str_value("0.3333333333333333"));
    assert_eq!(eval("'' + 1.5").unwrap(), str_value("1.5"));
    assert_eq!(eval("'' + 0/0").unwrap(), str_value("NaN"));
    assert_eq!(eval("'' + 1/0").unwrap(), str_value("Infinity"));
    assert_eq!(eval("'' + 1e21").unwrap(), str_value("1e+21"));
    assert_eq!(eval("'' + 0.000001").unwrap(), str_value("0.000001"));
    assert_eq!(eval("'' + 1e-7").unwrap(), str_value("1e-7"));
}

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(eval("(1 + 2) * 3 - 4 / 2").unwrap(), Value::Number(7.0));
    assert_eq!(eval("'6' * '7'").unwrap(), Value::Number(42.0));
}

#[test]
fn completion_value_is_last_expression_statement() {
    assert_eq!(eval("1; 2; let x = 3;").unwrap(), Value::Number(2.0));
    assert_eq!(eval("let x = 3;").unwrap(), Value::Undefined);
}

#[test]
fn assignment_to_let_updates_binding() {
    assert_eq!(eval("let a = 1; a = a + 1; a").unwrap(), Value::Number(2.0));
}

#[test]
fn globals_persist_across_scripts() {
    let mut iso = isolate();
    iso.eval_script(&src("let a = 40")).unwrap();
    assert_eq!(iso.eval_script(&src("a + 2")).unwrap(), Value::Number(42.0));
}

#[test]
fn class_evaluates() {
    let value = eval("class X { m() { return 41 + 1; } } new X().m()").unwrap();
    assert!(matches!(value, Value::Number(n) if n == 42.0), "{value:?}");
}

#[test]
fn unsupported_expression_is_syntax_error() {
    assert_eq!(
        error_message("class C { accessor x = 1; }"),
        "SyntaxError: unsupported: auto-accessor"
    );
}

#[test]
fn parse_error_is_syntax_error() {
    assert!(error_message("let = ;").starts_with("SyntaxError: "));
}

#[test]
fn const_reassignment_is_type_error() {
    assert!(error_message("const a = 1; a = 2").starts_with("TypeError: "));
}

#[test]
fn unknown_identifier_is_reference_error() {
    assert_eq!(error_message("nope"), "ReferenceError: nope is not defined");
}

#[test]
fn let_redeclaration_is_syntax_error() {
    assert!(error_message("let a = 1; let a = 2").starts_with("SyntaxError: "));
}

#[test]
fn calling_non_function_is_type_error() {
    assert!(error_message("let a = 1; a()").starts_with("TypeError: "));
}

#[test]
fn host_type_error_becomes_js_type_error() {
    let mut iso = isolate();
    iso.register_host_function(HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    })
    .unwrap();
    let err = iso.eval_script(&src("nativeAdd('x')")).unwrap_err();
    assert_eq!(err.message, "TypeError: nativeAdd expects two numbers");
}

#[test]
fn duplicate_host_function_fails() {
    let mut iso = isolate();
    let def = HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    };
    iso.register_host_function(def).unwrap();
    assert_eq!(
        iso.register_host_function(def).unwrap_err().code(),
        ErrorCode::AlreadyExists
    );
}

#[test]
fn host_function_reads_isolate_slot() {
    struct Counter(f64);
    fn bump(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
        let counter = ctx.isolate_slot::<Counter>().unwrap();
        counter.0 += 1.0;
        Ok(Value::Number(counter.0))
    }
    let mut iso = isolate();
    iso.set_slot(Counter(0.0));
    iso.register_host_function(HostFunctionDef {
        name: "bump",
        arity: 0,
        callback: bump,
    })
    .unwrap();
    assert_eq!(
        iso.eval_script(&src("bump(); bump()")).unwrap(),
        Value::Number(2.0)
    );
}

#[test]
fn isolate_call_invokes_host_function() {
    let mut iso = isolate();
    iso.register_host_function(HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    })
    .unwrap();
    let func = iso.eval_script(&src("nativeAdd")).unwrap();
    let result = iso
        .call(
            &func,
            &Value::Undefined,
            &[Value::Number(1.0), Value::Number(2.0)],
        )
        .unwrap();
    assert_eq!(result, Value::Number(3.0));
}

#[test]
fn features_are_promises_modules_and_continuation_context_and_stale_handles_do_not_panic() {
    assert_eq!(
        NativeEngine.features(),
        EngineFeatures::PROMISES | EngineFeatures::MODULES | EngineFeatures::CONTINUATION_CONTEXT
    );
    let mut iso = isolate();
    assert!(iso.compile_module(&src("export {}")).is_ok());
    let stale = Handle {
        id: 7,
        generation: 3,
    };
    assert!(iso.evaluate_module(stale).is_err());
    assert!(iso.promise_state(&Value::Undefined).is_none());
    assert_eq!(iso.perform_microtask_checkpoint(), MicrotaskProgress::Idle);
    assert!(iso.call(&Value::Undefined, &Value::Undefined, &[]).is_err());
}

#[test]
fn termination_stops_execution() {
    let mut iso = isolate();
    iso.termination_handle().terminate();
    let err = iso.eval_script(&src("1 + 1")).unwrap_err();
    assert!(err.message.contains("terminated"), "{}", err.message);
}
