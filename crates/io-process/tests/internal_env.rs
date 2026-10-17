#![cfg(unix)] // velyr-io-process spawns children only on unix; Windows returns Unsupported.
//! Variables the runtime reserves for itself (`VELYR_WATCH_REPORT`) never reach a child, whatever
//! the caller put in its env.

use velyr_io_process::{INTERNAL_ENV, parse_options};

#[test]
fn spawn_options_cannot_pass_a_reserved_variable() {
    for name in INTERNAL_ENV {
        let json = format!(
            r#"{{"program":"x","args":[],"env":[["{name}","/etc/hosts"],["A","b"]],
                "stdin":"null","stdout":"null","stderr":"null"}}"#
        );
        let (_, spec) = parse_options(&json).unwrap();
        assert!(spec.env.iter().all(|(key, _)| key != name), "{name}");
        assert!(spec.env.iter().any(|(key, _)| key == "A"));
    }
}
