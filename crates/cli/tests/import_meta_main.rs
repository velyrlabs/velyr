use assert_cmd::Command;

fn run(engine: Option<&str>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.mjs"),
        "import './dep.mjs'; console.log('main', import.meta.main);",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("dep.mjs"),
        "console.log('dep', import.meta.main);",
    )
    .unwrap();
    let mut args = vec!["run", "-A"];
    args.extend(engine.into_iter().flat_map(|name| ["--engine", name]));
    args.push("main.mjs");
    Command::cargo_bin("velyr")
        .unwrap()
        .current_dir(dir.path())
        .args(args)
        .assert()
        .success()
        .stdout("dep false\nmain true\n");
}

#[test]
fn only_the_entry_is_main() {
    run(None);
}

#[cfg(feature = "engine-native")]
#[test]
fn only_the_entry_is_main_on_native() {
    run(Some("native"));
}
