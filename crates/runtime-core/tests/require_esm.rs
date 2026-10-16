//! `require_esm` inside a running entry module shares instances with `import`.

use std::path::Path;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, require_esm, run_module_file};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

fn require_host(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(path)) = args.first() else {
        return Err(HostError::TypeError("expected a path".into()));
    };
    let path = path.to_string_lossy();
    Ok(require_esm(ctx, Path::new(&path))?.unwrap_or(Value::Undefined))
}

#[test]
fn require_esm_shares_the_instance_with_import() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-require-esm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("dep.mjs"),
        "globalThis.runs = (globalThis.runs || 0) + 1; export const n = globalThis.runs;",
    )
    .unwrap();
    std::fs::write(dir.join("late.mjs"), "await 0; export const n = 1;").unwrap();
    let main = format!(
        "import {{ n }} from './dep.mjs';\n\
         const ns = requireEsm({dep:?});\n\
         if (ns.n !== n || globalThis.runs !== 1) throw new Error('duplicate instance');\n\
         if (requireEsm({late:?}) !== undefined) throw new Error('tla should be pending');",
        dep = dir.join("dep.mjs").to_string_lossy(),
        late = dir.join("late.mjs").to_string_lossy(),
    );
    std::fs::write(dir.join("main.mjs"), main).unwrap();
    let config = RuntimeConfig {
        capabilities: Some(
            CapabilitySet::builder()
                .allow(Capability::ImportLocal(PathPattern::new(
                    dir.to_str().unwrap(),
                )))
                .build(),
        ),
        platform_init: Some(V8Engine::init_platform),
        ..RuntimeConfig::default()
    };
    let host_functions = vec![HostFunctionDef {
        name: "requireEsm",
        arity: 1,
        callback: require_host,
    }];
    run_module_file(
        V8Engine::new(),
        config,
        host_functions,
        "",
        |_, _, _| {},
        dir.join("main.mjs"),
    )
    .expect("run");
}
