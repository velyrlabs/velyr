use assert_cmd::Command;

#[test]
fn abort_controller_is_available_in_every_profile() {
    let path = format!(
        "{}/../../tests/fixtures/events/abort.js",
        env!("CARGO_MANIFEST_DIR")
    );
    Command::cargo_bin("velyr")
        .unwrap()
        .args(["run", &path])
        .assert()
        .success()
        .stdout("true\n");
}
