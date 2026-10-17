//! Module kind of TypeScript sources: ESM for `.ts`/`.mts`/`.tsx`, CommonJS for `.cts`.

use velyr_runtime_modules::{MediaType, SourceTransform};
use velyr_tooling_typescript::TypeScriptTransform;

fn transform(source: &str, media_type: MediaType) -> Result<String, String> {
    TypeScriptTransform::new(None)
        .transform("/virtual/file.ts", source, media_type)
        .map(|done| done.code)
        .map_err(|error| error.to_string())
}

#[test]
fn ts_without_module_syntax_is_a_module() {
    let code = transform("const a: number = 1;\n", MediaType::TypeScript).unwrap();
    assert!(!code.contains("require("), "{code}");
    assert!(code.contains("const a = 1"), "{code}");
}

#[test]
fn export_equals_in_ts_is_a_diagnostic() {
    let message = transform("export = 1;\n", MediaType::TypeScript).unwrap_err();
    assert!(message.contains("/virtual/file.ts:1:1"), "{message}");
    assert!(message.contains("Export assignment"), "{message}");

    let message = transform(
        "import fs = require(\"fs\");\nfs.readFileSync;\n",
        MediaType::Mts,
    )
    .unwrap_err();
    assert!(message.contains("/virtual/file.ts:1:1"), "{message}");
}

#[test]
fn cts_keeps_export_equals() {
    let code = transform("const a: number = 1;\nexport = a;\n", MediaType::Cts).unwrap();
    assert!(code.contains("module.exports = a"), "{code}");
    assert!(code.contains("\"use strict\""), "{code}");
}

#[test]
fn cts_erased_type_import_leaves_no_empty_export() {
    let source =
        "import type { X } from './x';\nconst v: number = 1;\nexport = { v };\n// export {};\n";
    let code = transform(source, MediaType::Cts).unwrap();
    assert!(
        !code.lines().any(|line| line.trim() == "export {};"),
        "{code}"
    );
    assert!(code.contains("// export {};"), "{code}");
}

#[test]
fn cts_keeps_an_empty_export_the_author_wrote() {
    let code = transform("const v: number = 1;\nexport {};\n", MediaType::Cts).unwrap();
    assert!(code.contains("export {};"), "{code}");
}
