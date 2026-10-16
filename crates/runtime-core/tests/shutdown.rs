use std::thread;
use std::time::Duration;

use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RunError, Runtime, RuntimeConfig};

#[test]
fn terminate_from_another_thread_stops_an_infinite_loop() {
    let script = std::env::temp_dir().join(format!("velyr-shutdown-{}.js", std::process::id()));
    std::fs::write(&script, "while (true) {}").unwrap();
    let mut isolate = Runtime::<V8Engine>::new(RuntimeConfig::default())
        .create_isolate()
        .unwrap();
    let handle = isolate.termination_handle();
    let killer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        handle.terminate();
    });
    let result = isolate.run_script(&script);
    killer.join().unwrap();
    std::fs::remove_file(&script).unwrap();
    assert!(matches!(result, Err(RunError::Terminated)), "{result:?}");
}
