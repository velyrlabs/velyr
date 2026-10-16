// Own test binary: the init guard is process-global.
mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::MockEngine;
use velyr_runtime_core::{Runtime, RuntimeConfig};

static CALLS: AtomicUsize = AtomicUsize::new(0);

fn count_call() {
    CALLS.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn platform_init_runs_once_across_runtimes() {
    for _ in 0..2 {
        let cfg = RuntimeConfig {
            platform_init: Some(count_call),
            ..RuntimeConfig::default()
        };
        Runtime::<MockEngine>::new(cfg);
    }
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
}
