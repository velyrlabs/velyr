//! Shared fixture runner: a V8 isolate with the web globals the builtins lean on, the Buffer ops
//! and `install_compat_node`. Each fixture registers cases with `test`/`atest` (see
//! `fixtures/_harness.js`) and ends with `finish()`.
#![allow(dead_code)]

use velyr_compat_node::{compat_node_host_functions, install_buffer_global, install_compat_node};
use velyr_engine_api::{Engine, Isolate, IsolateOptions, PromiseState, Source, Value};
use velyr_engine_v8::V8Engine;
use velyr_web_core::{
    encoding_host_functions, install_encoding, install_events, install_url, url_host_functions,
};

pub mod script;

const HARNESS: &str = include_str!("../fixtures/_harness.js");

pub fn new_isolate() -> impl Isolate {
    let mut iso = V8Engine::new()
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let ops = url_host_functions()
        .into_iter()
        .chain(encoding_host_functions())
        .chain(compat_node_host_functions());
    for def in ops {
        iso.register_host_function(def).expect("host fn");
    }
    install_events(&mut iso).expect("events");
    install_url(&mut iso).expect("url");
    install_encoding(&mut iso).expect("encoding");
    install_compat_node(&mut iso).expect("install");
    iso
}

pub fn run_fixture(name: &str, fixture: &str, buffer_global: bool) {
    let mut iso = new_isolate();
    if buffer_global {
        install_buffer_global(&mut iso).expect("buffer global");
    }
    let source = Source {
        name: name.into(),
        text: format!("{HARNESS}\n{fixture}"),
    };
    let promise = iso
        .eval_script(&source)
        .unwrap_or_else(|e| panic!("{name}: {}", e.message));
    iso.perform_microtask_checkpoint();
    match iso.promise_state(&promise) {
        Some(PromiseState::Fulfilled(Value::String(text))) => {
            let text = text.to_string_lossy();
            assert!(text.starts_with("ok "), "{name}:\n{text}");
        }
        other => panic!("{name}: fixture did not finish: {other:?}"),
    }
}
