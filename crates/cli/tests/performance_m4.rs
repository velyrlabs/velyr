use std::time::Duration;

use assert_cmd::Command;

#[test]
fn performance_and_navigator_are_available() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.js");
    std::fs::write(
        &path,
        "console.log(typeof performance.now());\nconsole.log(navigator.userAgent);\n",
    )
    .unwrap();
    Command::cargo_bin("velyr")
        .unwrap()
        .timeout(Duration::from_secs(30))
        .arg("run")
        .arg(&path)
        .assert()
        .success()
        .stdout(format!("number\nVelyr/{}\n", env!("CARGO_PKG_VERSION")));
}
