use std::path::PathBuf;

use assert_cmd::Command;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/fs")
}

fn velyr(cwd: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(cwd)
        .arg("run")
        .args(args)
        .assert()
}

#[test]
fn read_without_allow_read_is_denied() {
    velyr(&fixtures(), &["read-denied.js"])
        .success()
        .stdout("VELYR_PERMISSION_DENIED\n");
}

#[test]
fn read_with_allow_read_returns_the_content() {
    velyr(&fixtures(), &["--allow-read=.", "read.js"])
        .success()
        .stdout("hello fs\n");
}

#[test]
fn write_outside_allow_write_is_denied_and_creates_nothing() {
    velyr(&fixtures(), &["--allow-read=.", "write-denied.js"])
        .success()
        .stdout("VELYR_PERMISSION_DENIED\n");
    assert!(!fixtures().join("should-not-exist.txt").exists());
}

#[test]
fn write_inside_allow_write_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(
        cwd.join("main.js"),
        r#"await Velyr.writeTextFile("out.txt", "written");
console.log(await Velyr.readTextFile("out.txt"));"#,
    )
    .unwrap();
    velyr(&cwd, &["--allow-read=.", "--allow-write=.", "main.js"])
        .success()
        .stdout("written\n");
    assert_eq!(
        std::fs::read_to_string(cwd.join("out.txt")).unwrap(),
        "written"
    );
}

#[test]
fn text_encoder_is_available() {
    velyr(&fixtures(), &["encoder.js"]).success().stdout("97\n");
}
