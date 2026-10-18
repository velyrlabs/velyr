use std::path::Path;

use serde_json::json;
use velyr::{
    CallContext, Capability, CapabilityKind, Error, HostError, HostFunctionDef, PathPattern,
    Permissions, Profile, Request, Runtime, Value, authorize,
};

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_owned()
}

fn allow_dir(dir: &Path) -> Permissions {
    let pattern = PathPattern::new(dunce::canonicalize(dir).unwrap().to_str().unwrap());
    Permissions::new().allow(Capability::ImportLocal(pattern))
}

#[test]
fn run_module_executes_a_module_from_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let module = write(
        dir.path(),
        "main.mjs",
        "if (typeof console.log !== 'function') throw new Error('no console');",
    );
    let mut runtime = Runtime::builder()
        .permissions(allow_dir(dir.path()))
        .build()
        .unwrap();
    runtime.run_module(&module).unwrap();
}

#[test]
fn run_module_reports_uncaught_errors() {
    let dir = tempfile::tempdir().unwrap();
    let module = write(dir.path(), "boom.mjs", "throw new Error('boom');");
    let mut runtime = Runtime::builder()
        .permissions(allow_dir(dir.path()))
        .build()
        .unwrap();
    let err = runtime.run_module(&module).unwrap_err();
    assert!(err.to_string().contains("boom"), "{err}");
}

#[test]
fn call_returns_the_value_of_an_exported_function() {
    let dir = tempfile::tempdir().unwrap();
    let module = write(
        dir.path(),
        "math.mjs",
        "export function add(a, b) { return { sum: a + b, tags: ['x'] }; }\n\
         export async function later(n) { await new Promise((r) => setTimeout(r, 1)); return n * 2; }",
    );
    let mut runtime = Runtime::builder()
        .permissions(allow_dir(dir.path()))
        .build()
        .unwrap();
    runtime.run_module(&module).unwrap();
    assert_eq!(
        runtime.call("add", &[json!(2), json!(3)]).unwrap(),
        json!({ "sum": 5, "tags": ["x"] })
    );
    assert_eq!(runtime.call("later", &[json!(21)]).unwrap(), json!(42));
}

#[test]
fn call_result_cannot_be_forged_by_the_module() {
    let dir = tempfile::tempdir().unwrap();
    // The module grabs the private op at top level and forges a result after the wrapper's own
    // report, from a microtask, so the forged value would win if the op trusted any caller.
    let module = write(
        dir.path(),
        "forger.mjs",
        "const ret = globalThis.op_embed_return;\n\
         const forge = () => { for (const args of [['\"forged\"'], ['\"forged\"', '\"forged\"']]) { try { ret(...args); } catch (e) { if (!(e instanceof TypeError)) throw e; } } };\n\
         export function real() { return { toJSON() { queueMicrotask(forge); return 'real'; } }; }",
    );
    let mut runtime = Runtime::builder()
        .permissions(allow_dir(dir.path()))
        .build()
        .unwrap();
    runtime.run_module(&module).unwrap();
    assert_eq!(runtime.call("real", &[]).unwrap(), json!("real"));
}

#[test]
fn call_fails_for_a_missing_export_or_a_missing_module() {
    let dir = tempfile::tempdir().unwrap();
    let module = write(dir.path(), "m.mjs", "export const answer = 42;");
    let mut runtime = Runtime::builder()
        .permissions(allow_dir(dir.path()))
        .build()
        .unwrap();
    assert!(matches!(runtime.call("x", &[]), Err(Error::NoModule)));
    runtime.run_module(&module).unwrap();
    let err = runtime.call("answer", &[]).unwrap_err();
    assert!(err.to_string().contains("not a function"), "{err}");
    assert!(runtime.call("absent", &[]).is_err());
}

fn op_host_double(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    match args {
        [Value::Number(n)] => Ok(Value::Number(n * 2.0)),
        _ => Err(HostError::TypeError(
            "op_host_double expects a number".into(),
        )),
    }
}

fn op_host_secret(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    authorize(
        ctx,
        "op_host_secret",
        Request::EnvRead("VELYR_EMBED_SECRET"),
    )?;
    Ok(Value::Bool(true))
}

fn host_ops(permissions: Permissions) -> Runtime {
    Runtime::builder()
        .permissions(permissions)
        .op(
            HostFunctionDef {
                name: "op_host_double",
                arity: 1,
                callback: op_host_double,
            },
            CapabilityKind::None,
        )
        .op(
            HostFunctionDef {
                name: "op_host_secret",
                arity: 0,
                callback: op_host_secret,
            },
            CapabilityKind::EnvRead,
        )
        .build()
        .unwrap()
}

#[test]
fn host_op_is_callable_from_js() {
    let dir = tempfile::tempdir().unwrap();
    let module = write(
        dir.path(),
        "ops.mjs",
        "export const run = (n) => op_host_double(n);",
    );
    let mut runtime = host_ops(allow_dir(dir.path()));
    runtime.run_module(&module).unwrap();
    assert_eq!(runtime.call("run", &[json!(21)]).unwrap(), json!(42));
}

#[test]
fn host_op_permission_checks_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let module = write(
        dir.path(),
        "ops.mjs",
        "export const run = () => op_host_secret();",
    );
    let mut denied = host_ops(allow_dir(dir.path()));
    denied.run_module(&module).unwrap();
    let err = denied.call("run", &[]).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("permission"),
        "{err}"
    );

    let allowed = allow_dir(dir.path()).allow(Capability::EnvRead(velyr::NamePattern::new(
        "VELYR_EMBED_SECRET",
    )));
    let mut runtime = host_ops(allowed);
    runtime.run_module(&module).unwrap();
    assert_eq!(runtime.call("run", &[]).unwrap(), json!(true));
}

#[test]
fn module_outside_the_granted_tree_is_denied() {
    let granted = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let module = write(other.path(), "m.mjs", "export const x = 1;");
    let mut runtime = Runtime::builder()
        .permissions(allow_dir(granted.path()))
        .build()
        .unwrap();
    assert!(runtime.run_module(&module).is_err());
}

#[test]
fn builder_rejects_invalid_and_duplicate_op_names() {
    let def = |name| HostFunctionDef {
        name,
        arity: 1,
        callback: op_host_double,
    };
    let bad = Runtime::builder()
        .op(def("notAnOp"), CapabilityKind::None)
        .build();
    assert!(matches!(bad, Err(Error::Op(_))));
    let dup = Runtime::builder()
        .op(def("op_host_double"), CapabilityKind::None)
        .op(def("op_host_double"), CapabilityKind::None)
        .build();
    assert!(matches!(dup, Err(Error::Op(_))));
}

#[test]
fn profile_selects_the_installed_globals() {
    let dir = tempfile::tempdir().unwrap();
    let module = write(
        dir.path(),
        "g.mjs",
        "export const has = () => typeof setTimeout;",
    );
    for (profile, expected) in [(Profile::Core, "undefined"), (Profile::Minimal, "function")] {
        let mut runtime = Runtime::builder()
            .permissions(allow_dir(dir.path()))
            .profile(profile)
            .build()
            .unwrap();
        runtime.run_module(&module).unwrap();
        assert_eq!(runtime.call("has", &[]).unwrap(), json!(expected));
    }
}

#[test]
fn cpu_budget_stops_runaway_code() {
    let dir = tempfile::tempdir().unwrap();
    let module = write(
        dir.path(),
        "spin.mjs",
        "export const spin = () => { for (;;) {} };",
    );
    let mut runtime = Runtime::builder()
        .permissions(allow_dir(dir.path()))
        .cpu_budget(std::time::Duration::from_millis(100))
        .build()
        .unwrap();
    runtime.run_module(&module).unwrap();
    assert!(runtime.call("spin", &[]).is_err());
}
