use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use velyr_runtime_modules::{MediaType, SourceTransform};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_tooling_tsconfig::TsconfigLoader;
use velyr_tooling_typescript::TypeScriptTransform;

const DECORATED: &str = r#"function Inject(token: string) { return (t: any, k: string | symbol | undefined, i: number) => {}; }
function Log(): MethodDecorator { return () => {}; }
class Svc {}
export class Ctl {
  name!: string;
  constructor(@Inject("svc") private readonly svc: Svc, public id: number) {}
  @Log() run(a: number): string { return "x"; }
}
"#;

const LEGACY: &str = r#"{ // decorators
  "compilerOptions": { "experimentalDecorators": true, "emitDecoratorMetadata": true, "target": "ES2021", } }"#;

const FIELDS: &str = "export class A { x: number; y = 2; }\n";

fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

fn project(tsconfig: &str, cache: bool) -> (tempfile::TempDir, PathBuf, TypeScriptTransform) {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    write(&root, "tsconfig.json", tsconfig);
    let permissions: Arc<CapabilitySet> = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            root.to_str().unwrap(),
        )))
        .build();
    let cache_dir = cache.then(|| root.join(".cache"));
    let transform = TypeScriptTransform::new(cache_dir)
        .with_tsconfig(Arc::new(TsconfigLoader::new(permissions)));
    (dir, root, transform)
}

fn run(transform: &TypeScriptTransform, file: &Path, source: &str) -> String {
    transform
        .transform(file.to_str().unwrap(), source, MediaType::TypeScript)
        .unwrap()
        .code
}

fn compile(tsconfig: &str, source: &str) -> String {
    let (_dir, root, transform) = project(tsconfig, false);
    run(&transform, &root.join("src/a.ts"), source)
}

#[test]
fn legacy_decorators_and_metadata() {
    let code = compile(LEGACY, DECORATED);
    for needle in [
        "from \"velyr-oxc-runtime/helpers/decorate\"",
        "velyr-oxc-runtime/helpers/decorateParam",
        "velyr-oxc-runtime/helpers/decorateMetadata",
        "\"design:paramtypes\"",
        "\"design:returntype\"",
    ] {
        assert!(code.contains(needle), "{needle}\n{code}");
    }
    assert!(
        !code.contains("@Log") && !code.contains("@Inject"),
        "{code}"
    );
    assert!(!code.contains("require("), "{code}");
}

#[test]
fn decorators_in_a_file_without_module_syntax_use_import() {
    let source = "class A { @D() m() {} } function D(): any { return () => {}; }\n";
    let code = compile(LEGACY, source);
    assert!(!code.contains("require("), "{code}");
    assert!(
        code.contains("velyr-oxc-runtime/helpers/decorate"),
        "{code}"
    );
}

#[test]
fn define_for_class_fields_false_or_old_target() {
    for config in [
        r#"{ "compilerOptions": { "useDefineForClassFields": false } }"#,
        r#"{ "compilerOptions": { "target": "ES2021" } }"#,
    ] {
        let code = compile(config, FIELDS);
        assert!(code.contains("this.y = 2"), "{config}\n{code}");
        assert!(!code.contains("x;"), "{config}\n{code}");
    }
}

#[test]
fn define_for_class_fields_true_cases_keep_fields() {
    for config in [
        r#"{ "compilerOptions": { "target": "ES2022" } }"#,
        r#"{ "compilerOptions": { } }"#,
    ] {
        let code = compile(config, FIELDS);
        assert!(
            code.contains("y = 2;") && code.contains("x;"),
            "{config}\n{code}"
        );
    }
}

#[test]
fn verbatim_module_syntax_keeps_type_only_imports() {
    let source = "import { T } from \"./t\";\nexport const a: T = 1 as any;\n";
    let kept = compile(
        r#"{ "compilerOptions": { "verbatimModuleSyntax": true } }"#,
        source,
    );
    assert!(kept.contains("from \"./t\""), "{kept}");
    let dropped = compile(r#"{ "compilerOptions": { } }"#, source);
    assert!(!dropped.contains("./t"), "{dropped}");
}

#[test]
fn node_modules_files_get_defaults() {
    let (_dir, root, transform) = project(LEGACY, false);
    let code = run(&transform, &root.join("node_modules/p/x.ts"), DECORATED);
    assert!(code.contains("applyDecs2311"), "{code}");
}

#[test]
fn broken_tsconfig_falls_back_to_defaults() {
    let (_dir, root, transform) = project(r#"{ "compilerOptions": "#, false);
    for name in ["src/a.ts", "src/b.ts"] {
        let code = run(&transform, &root.join(name), FIELDS);
        assert!(code.contains("x;"), "{code}");
    }
}

#[test]
fn disk_cache_keys_on_derived_options() {
    let (_dir, root, transform) = project(LEGACY, true);
    let file = root.join("src/ctl.ts");
    run(&transform, &file, DECORATED);
    run(&transform, &file, DECORATED);
    assert_eq!(transform.cache_hits(), 1);

    write(
        &root,
        "tsconfig.json",
        r#"{ "compilerOptions": { "target": "ES2021" } }"#,
    );
    let loader = Arc::new(TsconfigLoader::new(
        CapabilitySet::builder()
            .allow(Capability::ImportLocal(PathPattern::new(
                root.to_str().unwrap(),
            )))
            .build(),
    ));
    let edited = TypeScriptTransform::new(Some(root.join(".cache"))).with_tsconfig(loader);
    let code = run(&edited, &file, DECORATED);
    assert_eq!(edited.cache_hits(), 0);
    assert!(code.contains("applyDecs2311"), "{code}");
}
