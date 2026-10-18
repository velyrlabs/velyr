use assert_cmd::Command;

#[test]
fn version_flag_prints_name_and_version() {
    Command::cargo_bin("velyr")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(concat!("velyr ", env!("CARGO_PKG_VERSION"), "\n"));
}
