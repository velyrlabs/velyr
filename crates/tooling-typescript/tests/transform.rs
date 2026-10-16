//! TypeScript stripping through the real `ModuleHost`, plus the on-disk transform cache.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use velyr_runtime_errors::ErrorCode;
use velyr_runtime_modules::{
    FsLoader, FsResolver, LoadContext, MediaType, ModuleHost, ModuleId, ModuleResolver,
    ResolveContext, SourceTransform,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_typescript::TypeScriptTransform;

fn transform(source: &str, media_type: MediaType) -> String {
    TypeScriptTransform::new(None)
        .transform("/virtual/file.ts", source, media_type)
        .unwrap()
        .code
}

#[test]
fn strips_annotations_interfaces_and_type_aliases() {
    let code = transform(
        "interface A { x: number }\ntype B = A | null;\nexport const f = (a: A, b?: B): number => a.x;\n",
        MediaType::TypeScript,
    );
    assert!(
        !code.contains("interface") && !code.contains("type B"),
        "{code}"
    );
    assert!(
        !code.contains(": number") && !code.contains("a: A"),
        "{code}"
    );
    assert!(code.contains("export const f = (a, b) => a.x"), "{code}");
}

#[test]
fn compiles_enums() {
    let code = transform(
        "export enum Color { Red, Green = 5 }\nexport const enum Flag { On = 1 }\n",
        MediaType::Mts,
    );
    assert!(!code.contains("enum "), "{code}");
    assert!(
        code.contains("Color[Color[\"Red\"] = 0] = \"Red\""),
        "{code}"
    );
    assert!(
        code.contains("Color[Color[\"Green\"] = 5] = \"Green\""),
        "{code}"
    );
}

#[test]
fn compiles_namespaces_and_parameter_properties() {
    let code = transform(
        "export namespace N { export const a = 1; }\nexport class P { constructor(public x: number, private y = 2) {} }\n",
        MediaType::Cts,
    );
    assert!(!code.contains("namespace "), "{code}");
    assert!(code.contains("N.a = 1") || code.contains("N.a=1"), "{code}");
    assert!(
        code.contains("this.x = x") && code.contains("this.y = y"),
        "{code}"
    );
    assert!(!code.contains("public"), "{code}");
}

#[test]
fn tsx_is_stripped_and_jsx_is_lowered() {
    let code = transform(
        "const el = <div id=\"a\">hi</div>;\nexport const n: number = 1;\n",
        MediaType::Tsx,
    );
    assert!(
        !code.contains(": number") && !code.contains("<div"),
        "{code}"
    );
}

#[test]
fn javascript_passes_through_untouched() {
    let source = "export const a = 1;\n";
    assert_eq!(transform(source, MediaType::JavaScript), source);
}

#[test]
fn produces_a_source_map() {
    let out = TypeScriptTransform::new(None)
        .transform(
            "/virtual/a.ts",
            "export const a: number = 1;\n",
            MediaType::TypeScript,
        )
        .unwrap();
    let map: serde_json::Value = serde_json::from_str(&out.source_map.unwrap()).unwrap();
    assert!(map["mappings"].as_str().is_some_and(|m| !m.is_empty()));
    assert!(map["sources"].to_string().contains("a.ts"));
}

#[test]
fn syntax_error_reports_file_line_and_column() {
    let err = TypeScriptTransform::new(None)
        .transform(
            "/virtual/bad.ts",
            "const a = 1;\nconst b: = 2;\n",
            MediaType::TypeScript,
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    assert!(
        err.message().contains("/virtual/bad.ts:2:"),
        "{}",
        err.message()
    );
}

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, LoadContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for (name, text) in files {
        fs::write(root.join(name), text).unwrap();
    }
    let permissions = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
        )))
        .build();
    (dir, root, LoadContext { permissions })
}

/// Loads `main.ts` and everything it imports, returning each module's transformed text.
async fn load_graph(host: &ModuleHost, root: &Path, ctx: &LoadContext) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut texts = Vec::new();
    let mut queue = vec![ModuleId::from_path(&root.join("main.ts")).unwrap()];
    while let Some(referrer) = queue.pop() {
        let module = FsResolver
            .resolve(
                &format!(
                    "./{}",
                    referrer
                        .to_path()
                        .unwrap()
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                ),
                &referrer,
                &ResolveContext::default(),
            )
            .unwrap();
        if !seen.insert(module.id.as_str().to_owned()) {
            continue;
        }
        let source = host.load(&module, ctx).await.unwrap();
        for line in source.text.lines().filter(|l| l.starts_with("import ")) {
            let spec = line.split('"').nth(1).unwrap();
            let target = FsResolver
                .resolve(spec, &module.id, &ResolveContext::default())
                .unwrap();
            queue.push(target.id);
        }
        texts.push(source.text.clone());
    }
    texts
}

const GRAPH: &[(&str, &str)] = &[
    (
        "main.ts",
        "import { Color } from \"./colors.ts\";\nimport type { Shape } from \"./types.ts\";\nimport { type Opts, run } from \"./util.ts\";\nconst s: Shape = { color: Color.Green };\nconst o: Opts = {};\nconsole.log(run(s, o));\n",
    ),
    ("colors.ts", "export enum Color { Red, Green }\n"),
    (
        "types.ts",
        "import type { Color } from \"./colors.ts\";\nexport interface Shape { color: Color }\n",
    ),
    (
        "util.ts",
        "import type { Shape } from \"./types.ts\";\nexport interface Opts { n?: number }\nexport function run(s: Shape, _o: Opts): number { return s.color; }\n",
    ),
];

#[tokio::test]
async fn ts_graph_loads_through_the_host_and_the_second_run_hits_the_cache() {
    let (_d, root, ctx) = project(GRAPH);
    let cache = tempfile::tempdir().unwrap();
    let first = Arc::new(TypeScriptTransform::new(Some(cache.path().to_path_buf())));
    let host = ModuleHost::new(Arc::new(FsLoader)).with_transform(first.clone());
    let texts = load_graph(&host, &root, &ctx).await;
    assert_eq!(
        texts.len(),
        3,
        "types.ts is type-only and never imported at runtime"
    );
    assert_eq!(first.cache_hits(), 0);
    let all = texts.join("\n");
    assert!(
        !all.contains("enum ") && !all.contains("interface") && !all.contains("Opts"),
        "{all}"
    );
    assert!(!all.contains("types.ts"), "{all}");
    assert!(
        all.contains("./colors.ts") && all.contains("./util.ts"),
        "{all}"
    );

    let second = Arc::new(TypeScriptTransform::new(Some(cache.path().to_path_buf())));
    let host = ModuleHost::new(Arc::new(FsLoader)).with_transform(second.clone());
    assert_eq!(load_graph(&host, &root, &ctx).await, texts);
    assert_eq!(second.cache_hits(), 3);
}

#[test]
fn cache_is_keyed_by_content_and_survives_a_corrupt_entry() {
    let cache = tempfile::tempdir().unwrap();
    let ts = TypeScriptTransform::new(Some(cache.path().to_path_buf()));
    let run = |text: &str| {
        ts.transform("/p/a.ts", text, MediaType::TypeScript)
            .unwrap()
    };
    let a = run("export const a: number = 1;");
    run("export const a: number = 1;");
    assert_eq!(ts.cache_hits(), 1);
    assert_ne!(run("export const a: number = 2;"), a);
    assert_eq!(ts.cache_hits(), 1);
    for entry in fs::read_dir(cache.path()).unwrap() {
        fs::write(entry.unwrap().path(), "not json").unwrap();
    }
    assert_eq!(run("export const a: number = 1;"), a);
    let leftovers = fs::read_dir(cache.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("tmp")
        })
        .count();
    assert_eq!(leftovers, 0);
}
