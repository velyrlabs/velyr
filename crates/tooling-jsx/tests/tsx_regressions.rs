//! Regressions for `.tsx` enums and module source type of tiny JSX files.

use velyr_runtime_modules::{MediaType, SourceTransform};
use velyr_tooling_jsx::{JsxConfig, JsxTransform};

fn run(config: JsxConfig, specifier: &str, source: &str, media_type: MediaType) -> String {
    JsxTransform::new(config)
        .transform(specifier, source, media_type)
        .unwrap()
        .code
}

#[test]
fn tsx_enum_is_compiled() {
    let source = "export enum E { A, B }\nexport const x = <i>{E.A}</i>;\n";
    let code = run(JsxConfig::default(), "/m.tsx", source, MediaType::Tsx);
    assert!(code.contains("E[E[\"A\"] = 0] = \"A\""), "{code}");
}

#[test]
fn tsx_without_module_syntax_uses_import() {
    let source = "const x = <i/>;\n";
    for (specifier, media_type) in [
        ("/m.tsx", MediaType::Tsx),
        ("/m.jsx", MediaType::JavaScript),
    ] {
        let code = run(JsxConfig::default(), specifier, source, media_type);
        assert!(code.contains("import "), "{code}");
        assert!(!code.contains("require("), "{code}");
    }
}
