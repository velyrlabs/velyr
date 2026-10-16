//! The builtins wired the way a host wires them: ESM `import` through `NodeBuiltinResolver`,
//! CommonJS `require` through compat-commonjs, and the facade export lists against the real modules.

use std::sync::Arc;

use velyr_compat_commonjs::{commonjs_host_functions, install_commonjs};
use velyr_compat_node::{BUILTINS, NodeBuiltinResolver, install_compat_node};
use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_modules::FsResolver;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

const ESM_MAIN: &str = r#"
import path, { join, win32 } from 'node:path';
import * as bare from 'path';
import util, { format, types } from 'node:util';
import EventEmitter, { once } from 'events';
import assert, { strictEqual } from 'node:assert/strict';
import { isPromise } from 'node:util/types';
import posix from 'node:path/posix';

strictEqual(join('a', 'b'), 'a/b');
strictEqual(path.join, join);
strictEqual(bare.default, path);
strictEqual(posix, path.posix);
strictEqual(win32.resolve('C:\\a', '..\\b'), 'C:\\b');
strictEqual(format('%s-%d', 'x', 2), 'x-2');
strictEqual(util.types, types);
strictEqual(isPromise(Promise.resolve()), true);
const emitter = new EventEmitter();
const pending = once(emitter, 'go');
emitter.emit('go', 7);
strictEqual((await pending)[0], 7);
assert.throws(() => assert.strictEqual(1, 2), { code: 'ERR_ASSERTION' });
"#;

fn canonical(dir: &tempfile::TempDir) -> std::path::PathBuf {
    std::fs::canonicalize(dir.path()).unwrap()
}

#[test]
fn esm_imports_resolve_with_and_without_the_node_prefix() {
    let work = tempfile::tempdir().unwrap();
    let facades = tempfile::tempdir().unwrap();
    let (work, facades) = (canonical(&work), canonical(&facades));
    std::fs::write(work.join("main.mjs"), ESM_MAIN).unwrap();
    let resolver = NodeBuiltinResolver::new(Arc::new(FsResolver), &facades).unwrap();
    let capabilities = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            work.to_str().unwrap(),
        )))
        .allow(Capability::ImportLocal(PathPattern::new(
            resolver.facade_dir().to_str().unwrap(),
        )))
        .build();
    let config = RuntimeConfig {
        capabilities: Some(capabilities),
        platform_init: Some(V8Engine::init_platform),
        module_resolver: Some(Arc::new(resolver)),
        ..RuntimeConfig::default()
    };
    run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |isolate, _, _| install_compat_node(isolate).expect("install"),
        work.join("main.mjs"),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
}

#[test]
fn builtins_outside_the_facade_grant_are_denied() {
    let work = tempfile::tempdir().unwrap();
    let facades = tempfile::tempdir().unwrap();
    let (work, facades) = (canonical(&work), canonical(&facades));
    std::fs::write(work.join("main.mjs"), "import 'node:path';").unwrap();
    let resolver = NodeBuiltinResolver::new(Arc::new(FsResolver), &facades).unwrap();
    // Only the working directory is granted: the facade directory is not.
    let capabilities = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            work.to_str().unwrap(),
        )))
        .build();
    let config = RuntimeConfig {
        capabilities: Some(capabilities),
        platform_init: Some(V8Engine::init_platform),
        module_resolver: Some(Arc::new(resolver)),
        ..RuntimeConfig::default()
    };
    let result = run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |isolate, _, _| install_compat_node(isolate).expect("install"),
        work.join("main.mjs"),
    );
    assert!(result.is_err(), "facade import must need its own grant");
}

#[test]
fn non_builtin_specifiers_fall_through_to_the_inner_resolver() {
    let work = tempfile::tempdir().unwrap();
    let facades = tempfile::tempdir().unwrap();
    let (work, facades) = (canonical(&work), canonical(&facades));
    std::fs::write(work.join("dep.mjs"), "export const v = 1;").unwrap();
    std::fs::write(
        work.join("main.mjs"),
        "import { v } from './dep.mjs'; import { sep } from 'path'; if (v !== 1 || sep !== '/') throw new Error('bad');",
    )
    .unwrap();
    let resolver = NodeBuiltinResolver::new(Arc::new(FsResolver), &facades).unwrap();
    let capabilities = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            work.to_str().unwrap(),
        )))
        .allow(Capability::ImportLocal(PathPattern::new(
            resolver.facade_dir().to_str().unwrap(),
        )))
        .build();
    let config = RuntimeConfig {
        capabilities: Some(capabilities),
        platform_init: Some(V8Engine::init_platform),
        module_resolver: Some(Arc::new(resolver)),
        ..RuntimeConfig::default()
    };
    run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |isolate, _, _| install_compat_node(isolate).expect("install"),
        work.join("main.mjs"),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
}

fn eval_string(iso: &mut impl Isolate, text: &str) -> String {
    match iso.eval_script(&Source {
        name: "probe.js".into(),
        text: text.into(),
    }) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn commonjs_require_finds_builtins_with_and_without_the_prefix() {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .unwrap();
    for def in commonjs_host_functions() {
        iso.register_host_function(def).unwrap();
    }
    install_commonjs(&mut iso, Arc::new(CapabilitySet::default())).unwrap();
    install_compat_node(&mut iso).unwrap();
    let out = eval_string(
        &mut iso,
        r#"
        const require = globalThis[Symbol.for("velyr.commonjs")].createRequire("/virtual/main.js");
        const same = require("node:path") === require("path") && require("node:events") === require("events");
        const names = ["path", "path/posix", "path/win32", "url", "events", "assert", "assert/strict", "util", "util/types"];
        const all = names.every((n) => require("node:" + n) === require(n));
        [same, all, require("assert").strict === require("node:assert/strict"), require("util").format("%s", "ok")].join(",");
        "#,
    );
    assert_eq!(out, "true,true,true,ok");
}

#[test]
fn facade_export_lists_match_the_modules() {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .unwrap();
    install_compat_node(&mut iso).unwrap();
    for builtin in BUILTINS {
        let actual = eval_string(
            &mut iso,
            &format!(
                "Object.keys(globalThis[Symbol.for('velyr.node')].require({:?})).sort().join(',')",
                builtin.name
            ),
        );
        let mut listed: Vec<&str> = builtin.exports.to_vec();
        listed.sort_unstable();
        assert_eq!(actual, listed.join(","), "export list of {}", builtin.name);
    }
}
