//! Stack traces point at the original source of transformed TypeScript and of bundles that
//! carry a `//# sourceMappingURL`.

use std::path::Path;

use assert_cmd::Command;

const MAIN_TS: &str = r#"interface Point {
  x: number;
  y: number;
}
type Id = string;
enum Color { Red, Green }
const p: Point = { x: 1, y: 2 };
const id: Id = "a";
function boom(n: number): never {
  throw new Error("boom " + n + id + Color.Red);
}
try { boom(p.x); } catch (e) { console.log((e as Error).stack); }
"#;

fn run(dir: &Path, file: &str) -> String {
    let home = dir.join("home");
    let out = Command::cargo_bin("velyr")
        .unwrap()
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("LOCALAPPDATA", home.join("cache"))
        .arg("run")
        .arg(dir.join(file))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn typescript_stack_points_at_the_original_line() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.ts"), MAIN_TS).unwrap();
    let stack = run(dir.path(), "main.ts");
    assert!(stack.contains("main.ts:10:9"), "{stack}");
    assert!(stack.contains("main.ts:12:7"), "{stack}");
}
