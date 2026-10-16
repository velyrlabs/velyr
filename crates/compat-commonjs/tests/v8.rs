//! CommonJS and its ESM interop running for real in a V8 isolate. The driver below does what the
//! runtime's module loader does, synchronously: files are read from a temp tree, passed through
//! `CommonJsTransform`, linked by the engine, and `import()` requests are served between runs.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use velyr_compat_commonjs::{CommonJsTransform, commonjs_host_functions, install_commonjs};
use velyr_engine_api::{
    CallContext, Engine, HostError, HostFunctionDef, Isolate, IsolateOptions, JsString,
    ModuleResolveHook, PromiseState, ResolverId, Source, Value,
};
use velyr_engine_v8::{V8Engine, V8Isolate};
use velyr_runtime_core::EsmRequire;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{
    FsResolver, MediaType, ModuleId, ModuleResolver, ResolveContext, SourceTransform,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

thread_local! {
    static PENDING: RefCell<Vec<(String, String, f64)>> = const { RefCell::new(Vec::new()) };
}

fn op_module_import(_ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (Some(Value::String(spec)), Some(Value::String(referrer)), Some(Value::Number(id))) =
        (args.first(), args.get(1), args.get(2))
    else {
        return Err(HostError::TypeError("bad import request".into()));
    };
    PENDING.with(|p| {
        p.borrow_mut()
            .push((spec.to_string_lossy(), referrer.to_string_lossy(), *id))
    });
    Ok(Value::Undefined)
}

struct Harness {
    iso: V8Isolate,
    root: PathBuf,
    _dir: tempfile::TempDir,
    transform: CommonJsTransform,
    compiled: Rc<RefCell<HashSet<String>>>,
}

struct Hook<'a> {
    transform: &'a CommonJsTransform,
    compiled: &'a RefCell<HashSet<String>>,
}

fn source_of(transform: &CommonJsTransform, path: &Path) -> Result<Source, VelyrError> {
    let text = fs::read_to_string(path).map_err(|e| VelyrError::not_found(e.to_string()))?;
    let name = ModuleId::from_path(path).unwrap().as_str().to_owned();
    let out = transform.transform(&path.to_string_lossy(), &text, MediaType::from_path(path))?;
    Ok(Source {
        name,
        text: out.code,
    })
}

impl ModuleResolveHook for Hook<'_> {
    fn resolve(&mut self, specifier: &str, referrer: &str) -> Result<Source, HostError> {
        let referrer = ModuleId::parse(referrer).unwrap();
        let module = FsResolver.resolve(specifier, &referrer, &ResolveContext::default())?;
        let source = source_of(self.transform, &module.path)?;
        self.compiled.borrow_mut().insert(source.name.clone());
        Ok(source)
    }
}

impl Harness {
    /// `files` are written under a fresh root; `allowed` is the grant (the root when `None`).
    fn new(files: &[(&str, &str)], allowed: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for (name, text) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let grant = allowed.map_or(root.clone(), |sub| root.join(sub));
        let permissions = CapabilitySet::builder()
            .allow(Capability::ImportLocal(PathPattern::new(
                grant.to_str().unwrap(),
            )))
            .build();
        let mut iso = V8Engine::new()
            .create_isolate(IsolateOptions::default())
            .unwrap();
        for def in commonjs_host_functions() {
            iso.register_host_function(def).unwrap();
        }
        iso.register_host_function(HostFunctionDef {
            name: "op_module_import",
            arity: 3,
            callback: op_module_import,
        })
        .unwrap();
        EsmRequire::new(None, Arc::clone(&permissions))
            .with_transform(Arc::new(CommonJsTransform::new(Arc::clone(&permissions))))
            .install(&mut iso);
        install_commonjs(&mut iso, Arc::clone(&permissions)).unwrap();
        Self {
            iso,
            root,
            _dir: dir,
            transform: CommonJsTransform::new(permissions),
            compiled: Rc::default(),
        }
    }

    fn load(&mut self, path: &Path) -> Result<velyr_engine_api::Handle, String> {
        let source = source_of(&self.transform, path).map_err(|e| e.to_string())?;
        self.compiled.borrow_mut().insert(source.name.clone());
        let handle = self.iso.compile_module(&source).map_err(|e| e.message)?;
        let mut hook = Hook {
            transform: &self.transform,
            compiled: &self.compiled,
        };
        self.iso
            .instantiate_module(handle, &mut hook)
            .map_err(|e| e.message)?;
        Ok(handle)
    }

    /// Runs `entry` (relative to the root) and every `import()` it triggers.
    fn run(&mut self, entry: &str) -> Result<(), String> {
        let handle = self.load(&self.root.join(entry))?;
        let promise = self.iso.evaluate_module(handle).map_err(|e| e.message)?;
        self.iso.perform_microtask_checkpoint();
        self.serve_imports();
        match self.iso.promise_state(&promise) {
            Some(PromiseState::Rejected(reason)) => Err(self.describe(reason)),
            _ => Ok(()),
        }
    }

    fn describe(&mut self, reason: Value) -> String {
        let probe = self.iso.eval_script(&Source {
            name: "describe.js".into(),
            text: "(e => e && e.code ? e.code + ': ' + e.message : String(e))".into(),
        });
        match probe.and_then(|f| self.iso.call(&f, &Value::Undefined, &[reason])) {
            Ok(Value::String(text)) => text.to_string_lossy(),
            other => format!("{other:?}"),
        }
    }

    fn serve_imports(&mut self) {
        while let Some((spec, referrer, id)) = PENDING.with(|p| p.borrow_mut().pop()) {
            let referrer = ModuleId::parse(&referrer)
                .unwrap_or_else(|| ModuleId::from_path(&self.root.join("x")).unwrap());
            let module = FsResolver
                .resolve(&spec, &referrer, &ResolveContext::default())
                .unwrap();
            let name = module.id.as_str().to_owned();
            if !self.compiled.borrow().contains(&name) {
                self.load(&module.path).unwrap();
            }
            self.iso.resolve(
                ResolverId(id as u64),
                Value::String(JsString::from(name.as_str())),
            );
            self.iso.perform_microtask_checkpoint();
        }
    }

    /// `JSON.stringify(globalThis.out)`.
    fn out(&mut self) -> String {
        let value = self
            .iso
            .eval_script(&Source {
                name: "out.js".into(),
                text: "JSON.stringify(globalThis.out)".into(),
            })
            .unwrap();
        match value {
            Value::String(text) => text.to_string_lossy(),
            other => format!("{other:?}"),
        }
    }
}

fn run_out(files: &[(&str, &str)], entry: &str) -> String {
    let mut h = Harness::new(files, None);
    h.run(entry).unwrap();
    h.out()
}

#[test]
fn cycle_sees_partial_exports() {
    let out = run_out(
        &[
            (
                "a.js",
                "exports.early = 1; const b = require('./b'); exports.late = 2; exports.seenByB = b.sawA;",
            ),
            (
                "b.js",
                "const a = require('./a'); exports.sawA = JSON.stringify(a);",
            ),
            ("main.cjs", "globalThis.out = require('./a');"),
        ],
        "main.cjs",
    );
    assert_eq!(out, r#"{"early":1,"late":2,"seenByB":"{\"early\":1}"}"#);
}

#[test]
fn json_is_parsed_with_and_without_extension() {
    let files = [
        ("data.json", "\u{feff}{\"n\": 5}"),
        ("bad.json", "{oops"),
        (
            "main.cjs",
            "let bad; try { require('./bad.json') } catch (e) { bad = e.message.startsWith(__dirname + '/bad.json: ') }\n\
             globalThis.out = [require('./data.json').n, require('./data').n, bad];",
        ),
    ];
    assert_eq!(run_out(&files, "main.cjs"), "[5,5,true]");
}

#[test]
fn esm_imports_commonjs_with_detected_named_exports() {
    let files = [
        (
            "lib.cjs",
            "exports.a = 1; module.exports.b = 2; Object.defineProperty(exports, 'c', { enumerable: true, value: 3 }); exports['d-e'] = 4;",
        ),
        (
            "obj.cjs",
            "const y = 7; module.exports = { x: 1, y, 'z': 3, [`dyn`]: 9 };",
        ),
        ("re.cjs", "module.exports = require('./lib.cjs');"),
        (
            "main.mjs",
            "import def, { a, b, c } from './lib.cjs';\n\
             import { x, y, z } from './obj.cjs';\n\
             import { a as ra } from './re.cjs';\n\
             import * as ns from './lib.cjs';\n\
             globalThis.out = { a, b, c, x, y, z, ra, same: def === ns.default && def === ns['module.exports'], keys: Object.keys(ns).sort(), de: ns['d-e'] };",
        ),
    ];
    let out = run_out(&files, "main.mjs");
    assert_eq!(
        out,
        r#"{"a":1,"b":2,"c":3,"x":1,"y":7,"z":3,"ra":1,"same":true,"keys":["a","b","c","d-e","default","module.exports"],"de":4}"#
    );
}

#[test]
fn undetected_named_import_is_a_link_error() {
    let files = [
        (
            "lib.cjs",
            "module.exports = Object.assign({}, { hidden: 1 });",
        ),
        ("main.mjs", "import { hidden } from './lib.cjs';"),
    ];
    let err = Harness::new(&files, None).run("main.mjs").unwrap_err();
    assert!(err.contains("hidden"), "{err}");
}

#[test]
fn esm_and_require_share_one_instance() {
    let files = [
        (
            "count.cjs",
            "globalThis.runs = (globalThis.runs || 0) + 1; module.exports = { n: globalThis.runs };",
        ),
        (
            "main.mjs",
            "import c from './count.cjs'; import { createRequire } from './req.mjs'; globalThis.out = [c.n, createRequire(import.meta.url)('./count.cjs') === c, globalThis.runs];",
        ),
        (
            "req.mjs",
            "export const createRequire = (url) => globalThis[Symbol.for('velyr.commonjs')].createRequire(url.slice(7));",
        ),
    ];
    assert_eq!(run_out(&files, "main.mjs"), "[1,true,1]");
}

#[test]
fn require_of_esm_without_tla_returns_the_namespace() {
    let files = [
        ("dep.mjs", "export const value = 42; export default 'dflt';"),
        (
            "main.cjs",
            "const ns = require('./dep.mjs'); globalThis.out = [ns.value, ns.default, require('./dep.mjs') === ns];",
        ),
    ];
    assert_eq!(run_out(&files, "main.cjs"), r#"[42,"dflt",true]"#);
}

#[test]
fn require_of_esm_with_tla_throws_err_require_async_module() {
    let files = [
        ("late.mjs", "await 0; export const value = 1;"),
        ("wrap.mjs", "import './late.mjs'; export const x = 1;"),
        (
            "main.cjs",
            "const code = (f) => { try { f() } catch (e) { return e.code } };\n\
             globalThis.out = [code(() => require('./late.mjs')), code(() => require('./wrap.mjs'))];",
        ),
    ];
    assert_eq!(
        run_out(&files, "main.cjs"),
        r#"["ERR_REQUIRE_ASYNC_MODULE","ERR_REQUIRE_ASYNC_MODULE"]"#
    );
}

#[test]
fn require_of_throwing_esm_rethrows_and_is_not_unhandled() {
    let files = [
        ("boom.mjs", "throw new Error('boom');"),
        (
            "main.cjs",
            "try { require('./boom.mjs') } catch (e) { globalThis.out = e.message }",
        ),
    ];
    let mut h = Harness::new(&files, None);
    h.run("main.cjs").unwrap();
    assert_eq!(h.out(), r#""boom""#);
    assert!(h.iso.take_unhandled_rejections().is_empty());
}

#[test]
fn require_esm_graph_follows_cjs_imports_and_shares_with_import() {
    let files = [
        (
            "count.mjs",
            "globalThis.runs = (globalThis.runs || 0) + 1; export const n = globalThis.runs;",
        ),
        (
            "lib.mjs",
            "import { n } from './count.mjs'; import legacy from './legacy.cjs'; export { n, legacy };",
        ),
        ("legacy.cjs", "module.exports = { kind: 'cjs' };"),
        (
            "main.cjs",
            "const ns = require('./lib.mjs');\n\
             import('./count.mjs').then((c) => { globalThis.out = [ns.n, ns.legacy.kind, c.n, globalThis.runs]; });",
        ),
    ];
    assert_eq!(run_out(&files, "main.cjs"), r#"[1,"cjs",1,1]"#);
}

#[test]
fn package_type_and_extension_pick_the_loader() {
    let files = [
        ("package.json", r#"{"type":"module"}"#),
        ("esm.js", "export const kind = 'esm';"),
        ("cjs.cjs", "exports.kind = 'cjs';"),
        ("legacy/package.json", r#"{"type":"commonjs"}"#),
        ("legacy/old.js", "exports.kind = 'old';"),
        ("sniffed/new.js", "export const kind = 'sniffed-esm';"),
        ("sniffed/old.js", "exports.kind = 'sniffed-cjs';"),
        (
            "main.mjs",
            "import { kind as a } from './esm.js'; import { kind as b } from './cjs.cjs'; import { kind as c } from './legacy/old.js';\n\
             globalThis.out = [a, b, c];",
        ),
    ];
    let mut h = Harness::new(&files, None);
    h.run("main.mjs").unwrap();
    assert_eq!(h.out(), r#"["esm","cjs","old"]"#);
    // Without a package `type`, the source is sniffed.
    let loose = [
        ("new.js", "export const kind = 'sniffed-esm';"),
        ("old.js", "exports.kind = 'sniffed-cjs';"),
        (
            "main.mjs",
            "import { kind as a } from './new.js'; import { kind as b } from './old.js'; globalThis.out = [a, b];",
        ),
    ];
    assert_eq!(
        run_out(&loose, "main.mjs"),
        r#"["sniffed-esm","sniffed-cjs"]"#
    );
}

#[test]
fn resolve_cache_and_wrapper_arguments() {
    let files = [
        (
            "dep.js",
            "globalThis.runs = (globalThis.runs || 0) + 1; module.exports = this;",
        ),
        (
            "main.cjs",
            "const path = require.resolve('./dep');\n\
             const first = require('./dep');\n\
             const cached = require.cache[path];\n\
             delete require.cache[path];\n\
             const second = require('./dep');\n\
             globalThis.out = { isAbs: path === __dirname + '/dep.js', file: __filename === __dirname + '/main.cjs', runs: globalThis.runs, cachedExports: cached.exports === first, fresh: first !== second, thisIsExports: this === module.exports, loaded: cached.loaded };",
        ),
    ];
    assert_eq!(
        run_out(&files, "main.cjs"),
        r#"{"isAbs":true,"file":true,"runs":2,"cachedExports":true,"fresh":true,"thisIsExports":true,"loaded":true}"#
    );
}

#[test]
fn packages_resolve_through_exports_conditions() {
    let files = [
        (
            "node_modules/pkg/package.json",
            r#"{"name":"pkg","exports":{".":{"import":"./esm.mjs","require":"./cjs.js"},"./sub/*":"./lib/*.js","./hidden":null}}"#,
        ),
        ("node_modules/pkg/cjs.js", "module.exports = 'cjs-entry';"),
        ("node_modules/pkg/esm.mjs", "export default 'esm-entry';"),
        ("node_modules/pkg/lib/x.js", "module.exports = 'sub-x';"),
        ("node_modules/old/package.json", r#"{"main":"./lib/main"}"#),
        (
            "node_modules/old/lib/main.js",
            "module.exports = 'old-main';",
        ),
        (
            "main.cjs",
            "let blocked; try { require('pkg/hidden') } catch (e) { blocked = e.code }\n\
             globalThis.out = [require('pkg'), require('pkg/sub/x'), require('old'), blocked];",
        ),
    ];
    assert_eq!(
        run_out(&files, "main.cjs"),
        r#"["cjs-entry","sub-x","old-main","MODULE_NOT_FOUND"]"#
    );
}

#[test]
fn parent_relative_requires_resolve_from_the_real_file_behind_an_exports_subpath() {
    // The shape of rxjs: `exports` maps `./operators` to a nested file that requires `../internal/..`.
    let files = [
        (
            "node_modules/rx/package.json",
            r#"{"name":"rx","exports":{"./operators":{"node":"./dist/cjs/operators/index.js","default":"./dist/esm5/operators/index.js"}}}"#,
        ),
        (
            "node_modules/rx/dist/cjs/operators/index.js",
            "var audit = require('../internal/operators/audit'); exports.audit = audit.audit;",
        ),
        (
            "node_modules/rx/dist/cjs/internal/operators/audit.js",
            "exports.audit = function audit() { return 'audited'; };",
        ),
        (
            "main.cjs",
            "globalThis.out = require('rx/operators').audit();",
        ),
    ];
    assert_eq!(run_out(&files, "main.cjs"), r#""audited""#);
}

#[test]
fn missing_module_reports_the_require_stack_and_failures_are_not_cached() {
    let files = [
        ("broken.js", "exports.ok = 1; throw new Error('boom');"),
        (
            "main.cjs",
            "let missing, again;\n\
             try { require('./nope') } catch (e) { missing = [e.code, e.message.includes('Require stack'), e.requireStack[0] === __filename] }\n\
             try { require('./broken') } catch (e) { again = e.message }\n\
             try { require('./broken') } catch (e) { again += '/' + e.message }\n\
             globalThis.out = { missing, again };",
        ),
    ];
    assert_eq!(
        run_out(&files, "main.cjs"),
        r#"{"missing":["MODULE_NOT_FOUND",true,true],"again":"boom/boom"}"#
    );
}

#[test]
fn files_outside_the_grant_are_denied() {
    let files = [
        ("outside/secret.js", "module.exports = 'secret';"),
        (
            "app/main.cjs",
            "try { require('../outside/secret') } catch (e) { globalThis.out = e.code }",
        ),
    ];
    let mut h = Harness::new(&files, Some("app"));
    h.run("app/main.cjs").unwrap();
    assert_eq!(h.out(), r#""VELYR_PERMISSION_DENIED""#);
}

#[test]
fn syntax_error_surfaces_at_require_time() {
    let files = [
        ("bad.js", "const = 1;"),
        (
            "main.cjs",
            "try { require('./bad') } catch (e) { globalThis.out = e.name }",
        ),
    ];
    assert_eq!(run_out(&files, "main.cjs"), r#""SyntaxError""#);
}

#[test]
fn package_imports_resolve_in_require() {
    let files = [
        (
            "package.json",
            r##"{"imports":{"#util":"./lib/util.js","#cond":{"require":"./lib/req.js","import":"./lib/imp.js"},"#dep":"dep","#lib/*":"./lib/*.js"}}"##,
        ),
        ("lib/util.js", "module.exports = 'util';"),
        ("lib/req.js", "module.exports = 'req';"),
        ("lib/imp.js", "module.exports = 'imp';"),
        ("lib/inner.js", "module.exports = 'inner';"),
        ("node_modules/dep/package.json", r#"{"main":"./i.js"}"#),
        ("node_modules/dep/i.js", "module.exports = 'dep';"),
        (
            "main.cjs",
            "let missing; try { require('#nope') } catch (e) { missing = e.code }\n\
             globalThis.out = [require('#util'), require('#cond'), require('#dep'), require('#lib/inner'), missing];",
        ),
    ];
    assert_eq!(
        run_out(&files, "main.cjs"),
        r#"["util","req","dep","inner","MODULE_NOT_FOUND"]"#
    );
}
