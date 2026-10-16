use assert_cmd::Command;

#[test]
fn url_is_available_in_every_profile() {
    let path = format!(
        "{}/../../tests/fixtures/web-url/basic.js",
        env!("CARGO_MANIFEST_DIR")
    );
    Command::cargo_bin("velyr")
        .unwrap()
        .args(["run", &path])
        .assert()
        .success()
        .stdout("http://x.com/a?b=1&k=v+w#c\nfalse\n");
}
