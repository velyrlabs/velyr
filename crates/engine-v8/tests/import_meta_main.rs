//! `import.meta.main` on engine-v8.

use velyr_engine_api::{
    Engine, HostError, Isolate, IsolateOptions, JsString, ModuleResolveHook, Source, Value,
};
use velyr_engine_v8::V8Engine;

const MAIN: &str = "file:///a/main.mjs";
const DEP: &str = "file:///a/dep.mjs";

struct Dep;

impl ModuleResolveHook for Dep {
    fn resolve(&mut self, _specifier: &str, _referrer: &str) -> Result<Source, HostError> {
        Ok(Source {
            name: DEP.into(),
            text: "globalThis.dep = String(import.meta.main);".into(),
        })
    }
}

fn run(entry: Option<&str>) -> Value {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    if let Some(entry) = entry {
        iso.set_entry_module(entry);
    }
    let source = Source {
        name: MAIN.into(),
        text: "import './dep.mjs'; globalThis.main = String(import.meta.main);".into(),
    };
    let module = iso.compile_module(&source).expect("compile");
    iso.instantiate_module(module, &mut Dep).expect("link");
    iso.evaluate_module(module).expect("evaluate");
    iso.eval_script(&Source {
        name: "probe.js".into(),
        text: "globalThis.main + '|' + globalThis.dep".into(),
    })
    .expect("probe")
}

#[test]
fn only_the_entry_module_is_main() {
    assert_eq!(run(Some(MAIN)), Value::String(JsString::from("true|false")));
}

#[test]
fn main_is_undefined_without_an_entry() {
    assert_eq!(
        run(None),
        Value::String(JsString::from("undefined|undefined"))
    );
}
