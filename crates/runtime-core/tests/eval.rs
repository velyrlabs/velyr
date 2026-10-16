mod common;

use common::MockEngine;
use velyr_engine_api::Value;
use velyr_runtime_core::{Runtime, RuntimeConfig};

#[test]
fn eval_returns_engine_value() {
    let mut isolate = Runtime::<MockEngine>::new(RuntimeConfig::default())
        .create_isolate()
        .unwrap();
    assert_eq!(isolate.eval("1").unwrap(), Value::Number(1.0));
    isolate.shutdown();
}

#[test]
fn eval_surfaces_js_exception() {
    let mut isolate = Runtime::<MockEngine>::new(RuntimeConfig::default())
        .create_isolate()
        .unwrap();
    let err = isolate.eval("not a number").unwrap_err();
    assert!(err.message.starts_with("SyntaxError"));
}

#[test]
fn default_config_matches_adr_010() {
    assert_eq!(RuntimeConfig::default().max_completions_per_tick, 1024);
}

#[test]
fn run_script_reports_missing_file() {
    let mut isolate = Runtime::<MockEngine>::new(RuntimeConfig::default())
        .create_isolate()
        .unwrap();
    let err = isolate
        .run_script(std::path::Path::new("/nonexistent/file.js"))
        .unwrap_err();
    assert!(matches!(err, velyr_runtime_core::RunError::Read(_)));
}

#[test]
fn run_script_surfaces_uncaught_exception() {
    let path = std::env::temp_dir().join("velyr_run_script_syntax_error.js");
    std::fs::write(&path, "not a number").unwrap();
    let mut isolate = Runtime::<MockEngine>::new(RuntimeConfig::default())
        .create_isolate()
        .unwrap();
    let err = isolate.run_script(&path).unwrap_err();
    std::fs::remove_file(&path).ok();
    assert!(matches!(err, velyr_runtime_core::RunError::Uncaught(_)));
}
