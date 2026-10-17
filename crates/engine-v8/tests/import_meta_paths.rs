//! `import.meta.dirname` and `import.meta.filename` on engine-v8.

use velyr_engine_api::{
    Engine, HostError, Isolate, IsolateOptions, JsString, ModuleResolveHook, Source, Value,
};
use velyr_engine_v8::V8Engine;

struct NoImports;

impl ModuleResolveHook for NoImports {
    fn resolve(&mut self, specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        Err(HostError::TypeError(format!(
            "unexpected import `{specifier}`"
        )))
    }
}

fn text(s: &str) -> Value {
    Value::String(JsString::from(s))
}

fn run_probe(name: &str) -> Value {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: name.into(),
        text: "globalThis.out = [import.meta.dirname, import.meta.filename, import.meta.url].join('|');"
            .into(),
    };
    let module = iso.compile_module(&source).expect("compile");
    iso.instantiate_module(module, &mut NoImports)
        .expect("link");
    iso.evaluate_module(module).expect("evaluate");
    iso.eval_script(&Source {
        name: "probe.js".into(),
        text: "globalThis.out".into(),
    })
    .expect("probe")
}

#[test]
#[cfg(not(windows))]
fn file_url_module_gets_dirname_and_filename() {
    assert_eq!(
        run_probe("file:///tmp/app/main.mjs"),
        text("/tmp/app|/tmp/app/main.mjs|file:///tmp/app/main.mjs")
    );
}

#[test]
fn non_file_module_has_only_url() {
    assert_eq!(run_probe("main.js"), text("||main.js"));
}
