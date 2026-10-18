//! End to end TypeScript project: tsconfig `paths`/`extends`, `velyr.json` imports, legacy and
//! TC39 decorators, JSX, `using` and mapped stacks, on V8 and (with the feature) native.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

fn project() -> PathBuf {
    dunce::canonicalize(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/typescript-project"),
    )
    .unwrap()
}

fn run(entry: &str, engine: Option<&str>) -> (String, String, bool) {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("velyr").unwrap();
    // `velyr.json` is discovered from the cwd, so run from the project root.
    cmd.current_dir(project())
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", home.path().join(".cache"))
        .env("LOCALAPPDATA", home.path().join("cache"))
        .args(["run", "-A"]);
    if let Some(engine) = engine {
        cmd.args(["--engine", engine]);
    }
    let out = cmd.arg(project().join(entry)).output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

fn stdout_of(entry: &str, engine: Option<&str>) -> String {
    let (stdout, stderr, ok) = run(entry, engine);
    assert!(ok, "{entry} ({engine:?}) failed: {stderr}");
    stdout
}

fn assert_both_engines(entry: &str, expected: &str) {
    assert_eq!(stdout_of(entry, None), expected, "v8");
    #[cfg(feature = "engine-native")]
    assert_eq!(stdout_of(entry, Some("native")), expected, "native");
}

const MAIN_OUTPUT: &str =
    "paramtypes Number\nutil 4 4 6\ndir dir\ncolor Green\ndirname true\nmain true\n1\n";

#[test]
fn project_resolution_decorators_and_import_meta() {
    assert_both_engines("src/main.ts", MAIN_OUTPUT);
}

#[test]
fn tsx_entry_without_imports_uses_the_tsconfig_jsx_factory() {
    assert_both_engines("app.tsx", "<i/> High\n");
}

#[test]
fn jsx_entry_uses_the_tsconfig_jsx_factory() {
    assert_both_engines("app.jsx", "<i/> jsx\n");
}

#[test]
fn tc39_accessor_decorator_without_experimental_flag() {
    assert_both_engines("tc39/accessor.ts", "decorated accessor v\n5\n");
}

#[test]
fn using_declaration_disposes_on_both_engines() {
    assert_both_engines("using.ts", "body object\ndisposed\n");
}

fn first_frame(stack: &str) -> String {
    stack
        .lines()
        .find(|l| l.contains("at "))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn v8_stack_points_at_ts_line() {
    let frame = first_frame(&stdout_of("stack.ts", None));
    assert!(frame.contains("stack.ts:3:9"), "{frame}");
}

#[cfg(feature = "engine-native")]
#[test]
fn native_stack_points_at_ts_line() {
    let frame = first_frame(&stdout_of("stack.ts", Some("native")));
    assert!(frame.contains("stack.ts:3:9"), "{frame}");
}
