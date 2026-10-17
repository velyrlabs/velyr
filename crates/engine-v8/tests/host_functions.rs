use std::cell::Cell;

use velyr_engine_api::{
    CallContext, Engine, EngineError, HostError, HostFunctionDef, Isolate, IsolateOptions,
    JsString, MicrotaskProgress, ResolverId, Source, TickFlags, Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_errors::{ErrorCode, VelyrError};

fn isolate() -> V8Isolate {
    V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate")
}

fn eval(iso: &mut V8Isolate, text: &str) -> Value {
    let source = Source {
        name: "test.js".into(),
        text: text.into(),
    };
    iso.eval_script(&source).expect("eval")
}

fn register(iso: &mut V8Isolate, name: &'static str, callback: velyr_engine_api::HostFn) {
    let def = HostFunctionDef {
        name,
        arity: 0,
        callback,
    };
    iso.register_host_function(def).expect("register");
}

fn native_add(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    match args {
        [Value::Number(a), Value::Number(b)] => Ok(Value::Number(a + b)),
        _ => Err(HostError::TypeError("expected two numbers".into())),
    }
}

fn denied(_: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Err(VelyrError::permission_denied("read", "<redacted>").into())
}

fn throws_value(_: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Err(HostError::Thrown(Value::Number(7.0)))
}

struct Counter(u32);

fn bump(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let counter = ctx.isolate_slot::<Counter>().expect("counter slot");
    counter.0 += 1;
    Ok(Value::Number(f64::from(counter.0)))
}

thread_local! {
    static PENDING: Cell<Option<ResolverId>> = const { Cell::new(None) };
}

fn make_pending(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let (promise, id) = ctx.create_promise();
    PENDING.with(|p| p.set(Some(id)));
    Ok(promise)
}

fn already_resolved(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let (promise, id) = ctx.create_promise();
    ctx.resolve(id, Value::Number(5.0));
    Ok(promise)
}

fn build_object(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let object = ctx.create_object();
    let name = ctx.create_string(JsString::from("velyr"));
    ctx.set_property(&object, "name", name)?;
    let echoed = ctx.get_property(&args[0], "x")?;
    ctx.set_property(&object, "echo", echoed)?;
    let array = ctx.create_array(&[Value::Number(1.0), Value::Number(2.0)]);
    ctx.set_property(&object, "list", array)?;
    let buffer = ctx.create_array_buffer(&[1, 2, 3]);
    ctx.set_property(&object, "buf", buffer)?;
    Ok(object)
}

fn type_error_via_ctx(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Err(ctx.throw_type_error("bad input"))
}

#[test]
fn native_add_returns_sum() {
    let mut iso = isolate();
    register(&mut iso, "nativeAdd", native_add);
    assert_eq!(eval(&mut iso, "nativeAdd(20, 22)"), Value::Number(42.0));
}

#[test]
fn duplicate_name_is_already_exists() {
    let mut iso = isolate();
    register(&mut iso, "nativeAdd", native_add);
    let def = HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    };
    let err: EngineError = iso.register_host_function(def).unwrap_err();
    assert_eq!(err.code(), ErrorCode::AlreadyExists);
}

#[test]
fn arity_sets_function_length() {
    let mut iso = isolate();
    let def = HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    };
    iso.register_host_function(def).unwrap();
    assert_eq!(eval(&mut iso, "nativeAdd.length"), Value::Number(2.0));
}

#[test]
fn velyr_error_carries_code() {
    let mut iso = isolate();
    register(&mut iso, "denied", denied);
    let script =
        "try { denied(); 'no error' } catch (e) { e instanceof Error ? e.code : 'wrong type' }";
    assert_eq!(
        eval(&mut iso, script),
        Value::String(JsString::from("VELYR_PERMISSION_DENIED"))
    );
}

#[test]
fn host_type_error_is_js_type_error() {
    let mut iso = isolate();
    register(&mut iso, "nativeAdd", native_add);
    register(&mut iso, "viaCtx", type_error_via_ctx);
    let script = "let out = []; \
        try { nativeAdd('a') } catch (e) { out.push(e instanceof TypeError && e.message) } \
        try { viaCtx() } catch (e) { out.push(e instanceof TypeError && e.message) } \
        out.join('|')";
    assert_eq!(
        eval(&mut iso, script),
        Value::String(JsString::from("expected two numbers|bad input"))
    );
}

#[test]
fn thrown_value_is_rethrown_as_is() {
    let mut iso = isolate();
    register(&mut iso, "thrower", throws_value);
    assert_eq!(
        eval(&mut iso, "try { thrower() } catch (e) { e }"),
        Value::Number(7.0)
    );
}

#[test]
fn host_fn_sees_slot_state() {
    let mut iso = isolate();
    iso.set_slot(Counter(0));
    register(&mut iso, "bump", bump);
    assert_eq!(eval(&mut iso, "bump(); bump()"), Value::Number(2.0));
    assert_eq!(eval(&mut iso, "bump()"), Value::Number(3.0));
}

#[test]
fn context_builds_objects() {
    let mut iso = isolate();
    register(&mut iso, "build", build_object);
    let script = "const o = build({ x: 9 }); \
        [o.name, o.echo, o.list.join(), o.buf.byteLength].join('|')";
    assert_eq!(
        eval(&mut iso, script),
        Value::String(JsString::from("velyr|9|1,2|3"))
    );
}

#[test]
fn then_runs_only_after_checkpoint() {
    let mut iso = isolate();
    register(&mut iso, "makePending", make_pending);
    eval(
        &mut iso,
        "globalThis.seen = 'none'; makePending().then(v => { globalThis.seen = v })",
    );
    let id = PENDING.with(Cell::take).expect("resolver id");
    iso.resolve(id, Value::String(JsString::from("done")));
    let seen = |iso: &mut V8Isolate| eval(iso, "globalThis.seen");
    assert_eq!(seen(&mut iso), Value::String(JsString::from("none")));
    assert_eq!(
        iso.perform_microtask_checkpoint(),
        MicrotaskProgress::Progressed
    );
    assert_eq!(seen(&mut iso), Value::String(JsString::from("done")));
}

#[test]
fn resolve_from_call_context() {
    let mut iso = isolate();
    register(&mut iso, "resolved", already_resolved);
    eval(
        &mut iso,
        "globalThis.seen = 0; resolved().then(v => { globalThis.seen = v })",
    );
    assert_eq!(eval(&mut iso, "globalThis.seen"), Value::Number(0.0));
    iso.perform_microtask_checkpoint();
    assert_eq!(eval(&mut iso, "globalThis.seen"), Value::Number(5.0));
}

fn echo_bytes(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    match ctx.read_bytes(&args[0]) {
        Some(bytes) => Ok(ctx.create_array_buffer(&bytes)),
        None => Ok(Value::Null),
    }
}

fn utf8_bytes(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("expected a string".into()));
    };
    let mut out = vec![0x3c];
    ctx.write_string_utf8(text, &mut out);
    Ok(ctx.create_array_buffer(&out))
}

fn utf8_matches_lossy(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(text)] = args else {
        return Err(HostError::TypeError("expected a string".into()));
    };
    let mut out = Vec::new();
    ctx.write_string_utf8(text, &mut out);
    Ok(Value::Bool(out == text.to_string_lossy().into_bytes()))
}

#[test]
fn write_string_utf8_appends_utf8_and_replaces_lone_surrogates() {
    let mut iso = isolate();
    register(&mut iso, "utf8Bytes", utf8_bytes);
    register(&mut iso, "utf8MatchesLossy", utf8_matches_lossy);
    let script = r#"
        const hex = (v) => Array.from(new Uint8Array(v), (b) => b.toString(16)).join(",");
        const long = "abcé".repeat(500) + "€😀".repeat(300) + "\udc00x\ud800";
        [
          hex(utf8Bytes("")),
          hex(utf8Bytes("hi")),
          hex(utf8Bytes("café")),
          hex(utf8Bytes("€😀")),
          hex(utf8Bytes("a\ud800b")),
          hex(utf8Bytes("\ude00")),
          utf8MatchesLossy("x".repeat(70000)),
          utf8MatchesLossy(long),
        ].join("|")
    "#;
    let Value::String(out) = eval(&mut iso, script) else {
        panic!("expected string");
    };
    assert_eq!(
        out.to_string_lossy(),
        "3c|3c,68,69|3c,63,61,66,c3,a9|3c,e2,82,ac,f0,9f,98,80|3c,61,ef,bf,bd,62|3c,ef,bf,bd|true|true"
    );
}

#[test]
fn read_bytes_copies_buffers_and_view_windows_and_rejects_others() {
    let mut iso = isolate();
    register(&mut iso, "echoBytes", echo_bytes);
    let script = r#"
        const ab = Uint8Array.from([0, 1, 0x7f, 0x80, 0xff]).buffer;
        const hex = (v) => v === null ? "null" : Array.from(new Uint8Array(v), (b) => b.toString(16)).join(",");
        [
          hex(echoBytes(ab)),
          hex(echoBytes(new Uint8Array(ab, 1, 3))),
          hex(echoBytes(new DataView(ab, 2, 2))),
          hex(echoBytes(new Uint16Array(ab, 0, 2))),
          hex(echoBytes(new Uint8Array(0))),
          hex(echoBytes("0001")),
          hex(echoBytes({})),
        ].join("|")
    "#;
    let Value::String(out) = eval(&mut iso, script) else {
        panic!("expected string");
    };
    assert_eq!(
        out.to_string_lossy(),
        "0,1,7f,80,ff|1,7f,80|7f,80|0,1,7f,80||null|null"
    );
}

fn eval_script_hook(cx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Value::String(text) = &args[0] else {
        unreachable!()
    };
    cx.request_gc()?;
    cx.eval_script(&Source {
        name: "<evalScript>".into(),
        text: text.to_string_lossy(),
    })
}

#[test]
fn eval_script_from_host_function_shares_global() {
    let mut iso = isolate();
    let def = HostFunctionDef {
        name: "evalScript",
        arity: 1,
        callback: eval_script_hook,
    };
    iso.register_host_function(def).expect("register");
    assert_eq!(
        eval(&mut iso, "evalScript('var z = 41;'); z + 1"),
        Value::Number(42.0)
    );
}

#[test]
fn check_script_reports_early_errors_without_running() {
    let mut iso = isolate();
    let source = |text: &str| Source {
        name: "t".into(),
        text: text.into(),
    };
    assert!(iso.check_script(&source("throw 1; let x; let x;")).is_err());
    assert!(iso.check_script(&source("globalThis.ran = 1;")).is_ok());
    assert_eq!(
        eval(&mut iso, "typeof ran"),
        Value::String(JsString::from("undefined"))
    );
}

fn bind_flags(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let flags = ctx
        .isolate_slot::<TickFlags>()
        .cloned()
        .expect("flags slot");
    Ok(ctx.bind_tick_flags(&flags).unwrap_or(Value::Undefined))
}

#[test]
fn bound_tick_flags_share_memory_with_js() {
    let mut iso = isolate();
    let flags = TickFlags::new();
    iso.set_slot(flags.clone());
    register(&mut iso, "bind_flags", bind_flags);
    eval(&mut iso, "globalThis.bytes = bind_flags(); bytes[1] = 1;");
    assert!(flags.armed() && !flags.has_tick());
    flags.set_tick();
    assert_eq!(
        eval(&mut iso, "bytes[0] + bytes.length"),
        Value::Number(3.0)
    );
    eval(&mut iso, "bytes[0] = 0;");
    assert!(!flags.has_tick());
}
