use assert_cmd::Command;

#[test]
fn version_flag_prints_name_and_version() {
    Command::cargo_bin("velyr")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout("velyr 0.1.0\n");
}
