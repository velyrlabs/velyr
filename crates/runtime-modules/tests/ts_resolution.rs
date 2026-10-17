use std::fs;
use std::path::Path;

use velyr_runtime_errors::ErrorCode;
use velyr_runtime_modules::{FsResolver, ModuleId, ModuleResolver, ResolveContext};

fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::create_dir_all(root.join("lib2")).unwrap();
    for f in [
        "main.ts",
        "m.js",
        "a.ts",
        "b.mts",
        "c.tsx",
        "d.js",
        "d.ts",
        "lib/index.ts",
        "lib2.ts",
        "lib2/index.ts",
        "e.js",
    ] {
        fs::write(root.join(f), "").unwrap();
    }
    (dir, root)
}

fn resolve_from(
    root: &Path,
    referrer: &str,
    spec: &str,
) -> velyr_runtime_modules::Result<velyr_runtime_modules::ResolvedModule> {
    let referrer = ModuleId::from_path(&root.join(referrer)).unwrap();
    FsResolver.resolve(spec, &referrer, &ResolveContext::default())
}

#[test]
fn typescript_importer_probes_extensions_and_index() {
    let (_d, root) = project();
    let cases = [
        ("./a", "a.ts"),
        ("./a.ts", "a.ts"),
        ("./b.mjs", "b.mts"),
        ("./c", "c.tsx"),
        ("./d.js", "d.js"),
        ("./a.js", "a.ts"),
        ("./lib", "lib/index.ts"),
        ("./lib2", "lib2.ts"),
        ("./e.js", "e.js"),
    ];
    for (spec, want) in cases {
        let got = resolve_from(&root, "main.ts", spec).unwrap();
        assert_eq!(got.path, root.join(want), "{spec}");
    }
    let bare = resolve_from(&root, "main.ts", "./a").unwrap();
    let spelled = resolve_from(&root, "main.ts", "./a.ts").unwrap();
    assert_eq!(bare.id, spelled.id);
    let err = resolve_from(&root, "main.ts", "./nope").unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[test]
fn javascript_importer_stays_strict() {
    let (_d, root) = project();
    for spec in ["./a", "./a.js"] {
        let err = resolve_from(&root, "m.js", spec).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound, "{spec}");
    }
}
