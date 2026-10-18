//! RT-19: a burst of file reads never runs more blocking threads than the cap.

use velyr_engine_v8::V8Engine;
use velyr_io_fs::{fs_host_functions, install_fs};
use velyr_runtime_core::{RuntimeConfig, peak_blocking_threads, run_module_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::QuotaConfig;

const CAP: usize = 4;
const READS: usize = 200;
const FILE_BYTES: usize = 1 << 20;

#[test]
fn read_burst_stays_within_the_blocking_thread_cap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
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
            max_blocking_threads: Some(CAP),
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
    let message = result.unwrap_err().to_string();
    assert!(message.contains(&format!("done:{READS}")), "{message}");
    let peak = peak_blocking_threads();
    assert!(peak > 0, "no read reached the blocking pool");
    assert!(peak <= CAP, "{peak} blocking threads alive, cap is {CAP}");
}
