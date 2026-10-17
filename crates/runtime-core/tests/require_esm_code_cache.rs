//! Dependencies of a `require(esm)` graph consume the shared code cache (ML-07). One test only:
//! the layer is process-global once enabled.

use std::path::Path;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, require_esm, run_module_file};
use velyr_runtime_modules::SharedCache;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

fn require_host(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(path)) = args.first() else {
        return Err(HostError::TypeError("expected a path".into()));
    };
    let path = path.to_string_lossy();
    Ok(require_esm(ctx, Path::new(&path))?.unwrap_or(Value::Undefined))
}

fn run(dir: &Path) {
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

#[test]
fn required_graph_dependencies_consume_the_shared_cache() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("velyr-require-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.mjs"), "export const a = 40;").unwrap();
    std::fs::write(
        dir.join("b.mjs"),
        "import { a } from './a.mjs'; export const b = a + 2;",
    )
    .unwrap();
    let b = dir.join("b.mjs");
    std::fs::write(
        dir.join("main.mjs"),
        format!(
            "if (requireEsm({:?}).b !== 42) throw new Error('wrong');",
            b.to_string_lossy()
        ),
    )
    .unwrap();
    let shared = SharedCache::enable_global();

    run(&dir);
    assert_eq!(shared.uses(), (0, 0), "nothing to consume yet");

    run(&dir);
    // main and a hit; b is the root of the require call and is compiled uncached by design.
    assert_eq!(shared.uses(), (2, 0));
}
