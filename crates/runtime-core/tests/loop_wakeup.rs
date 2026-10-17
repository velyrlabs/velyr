//! A burst of large file reads finishes instead of parking the loop forever.

use std::sync::mpsc;
use std::time::Duration;

use velyr_engine_v8::V8Engine;
use velyr_io_fs::{fs_host_functions, install_fs};
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::QuotaConfig;

const READS: usize = 200;
const FILE_BYTES: usize = 1 << 20;
const RUNS: usize = 5;
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

fn read_burst() -> String {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let data = root.join("data.bin");
    std::fs::write(&data, vec![7u8; FILE_BYTES]).unwrap();
    let entry = root.join("main.mjs");
    let script = format!(
        r#"const reads = Array.from({{ length: {READS} }}, () => Velyr.readFile({data:?}));
        const sizes = (await Promise.all(reads)).map((bytes) => bytes.byteLength);
        if (sizes.some((size) => size !== {FILE_BYTES})) throw new Error("short read");
        throw new Error("done:" + sizes.length);"#
    );
    std::fs::write(&entry, script).unwrap();
    let permissions = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
        )))
        .allow(Capability::FsRead(PathPattern::new(data.to_str().unwrap())))
        .build();
    let granted = permissions.clone();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(fs_host_functions());
    let result = run_module_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            capabilities: Some(granted),
            ..RuntimeConfig::default()
        },
        functions,
        BOOTSTRAP_JS,
        move |isolate, sender, _| {
            install_op_context(isolate, 0, sender, permissions, QuotaConfig::default());
            install_fs(isolate).unwrap();
        },
        entry,
    );
    result.unwrap_err().to_string()
}

#[test]
fn large_read_burst_never_parks_the_loop_forever() {
    for run in 0..RUNS {
        let (done, outcome) = mpsc::channel();
        std::thread::spawn(move || done.send(read_burst()));
        let message = outcome
            .recv_timeout(RUN_TIMEOUT)
            .unwrap_or_else(|_| panic!("run {run}: the event loop hung for {RUN_TIMEOUT:?}"));
        assert!(message.contains(&format!("done:{READS}")), "{message}");
    }
}
