#![cfg(unix)] // velyr-io-process spawns children only on unix; Windows returns Unsupported.
//! A child never gets more shim authority than its parent: `VELYR_NODE_SHIM_FLAGS` is overwritten
//! (or dropped) on every spawn, whatever the caller put in its env.

use std::ffi::OsString;

use velyr_io_process::{SHIM_FLAGS_ENV, parse_options, pin_shim_flags};

fn pair(name: &str, value: &str) -> (OsString, OsString) {
    (name.into(), value.into())
}

fn shim_values(env: &[(OsString, OsString)]) -> Vec<&OsString> {
    env.iter()
        .filter(|(name, _)| name == SHIM_FLAGS_ENV)
        .map(|(_, value)| value)
        .collect()
}

#[test]
fn the_callers_value_is_replaced_by_the_parents() {
    let mut env = vec![pair(SHIM_FLAGS_ENV, "-A"), pair("KEEP", "1")];
    pin_shim_flags(&mut env, Some("--allow-read=."));
    assert_eq!(shim_values(&env), [&OsString::from("--allow-read=.")]);
    assert!(env.contains(&pair("KEEP", "1")));
}

#[test]
fn without_parent_flags_the_variable_is_removed() {
    let mut env = vec![pair(SHIM_FLAGS_ENV, "-A")];
    pin_shim_flags(&mut env, None);
    assert!(shim_values(&env).is_empty());
}

#[test]
fn spawn_options_cannot_smuggle_the_variable_in() {
    // No parent flags were recorded in this process, so the explicit value must vanish.
    let json = format!(
        r#"{{"program":"x","args":[],"env":[["{SHIM_FLAGS_ENV}","-A"],["A","b"]],
            "stdin":"null","stdout":"null","stderr":"null"}}"#
    );
    let (_, spec) = parse_options(&json).unwrap();
    assert!(shim_values(&spec.env).is_empty());
    assert!(spec.env.contains(&pair("A", "b")));
}
