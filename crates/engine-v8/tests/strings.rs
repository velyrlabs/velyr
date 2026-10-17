use velyr_engine_api::{
    CallContext, Engine, HostError, HostFunctionDef, Isolate, IsolateOptions, JsString, Source,
    Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};

fn echo(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    Ok(args.first().cloned().unwrap_or(Value::Undefined))
}

fn eval(iso: &mut V8Isolate, text: &str) -> Value {
    let source = Source {
        name: "strings.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source).expect("eval")
}

fn isolate() -> V8Isolate {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let def = HostFunctionDef {
        name: "echo",
        arity: 1,
        callback: echo,
    };
    iso.register_host_function(def).expect("register");
    iso
}

fn string(value: Value) -> JsString {
    match value {
        Value::String(s) => s,
        other => panic!("expected string, got {other:?}"),
    }
}

#[test]
fn one_byte_string_crosses_as_latin1() {
    let mut iso = isolate();
    let s = string(eval(&mut iso, "echo('h\\u00e9llo')"));
    assert_eq!(s.as_latin1(), Some(&b"h\xe9llo"[..]));
    assert_eq!(s.to_string_lossy(), "h\u{e9}llo");
}

#[test]
fn two_byte_and_lone_surrogate_strings_are_unchanged() {
    let mut iso = isolate();
    let s = string(eval(&mut iso, "echo('a\\ud800b')"));
    assert_eq!(s.as_latin1(), None);
    assert_eq!(s.as_utf16(), &[0x61, 0xD800, 0x62]);
    let wide = string(eval(&mut iso, "echo('\\u20ac\\u{1F600}')"));
    assert_eq!(wide.as_utf16(), &[0x20AC, 0xD83D, 0xDE00]);
}

#[test]
fn nul_and_empty_strings_round_trip() {
    let mut iso = isolate();
    let nul = string(eval(&mut iso, "echo('a\\0b')"));
    assert_eq!(nul.as_utf16(), &[0x61, 0, 0x62]);
    let empty = string(eval(&mut iso, "echo('')"));
    assert!(empty.is_empty());
    assert_eq!(empty, JsString::default());
}

#[test]
fn latin1_input_reaches_js_intact() {
    let mut iso = isolate();
    let def = HostFunctionDef {
        name: "latin",
        arity: 0,
        callback: |_, _| Ok(Value::String(JsString::from_latin1(b"caf\xe9".to_vec()))),
    };
    iso.register_host_function(def).expect("register");
    assert_eq!(
        eval(&mut iso, "latin() === 'caf\\u00e9' && latin().length === 4"),
        Value::Bool(true)
    );
}

fn to_utf8(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(text)) = args.first() else {
        return Err(HostError::TypeError("expected string".into()));
    };
    let mut out = Vec::new();
    ctx.write_string_utf8(text, &mut out);
    Ok(Value::String(JsString::from_latin1(out)))
}

#[test]
fn latin1_string_writes_utf8_without_widening() {
    let mut iso = isolate();
    let def = HostFunctionDef {
        name: "toUtf8",
        arity: 1,
        callback: to_utf8,
    };
    iso.register_host_function(def).expect("register");
    assert_eq!(
        eval(&mut iso, "toUtf8('ab\\u00e9') === 'ab\\u00c3\\u00a9'"),
        Value::Bool(true)
    );
}
