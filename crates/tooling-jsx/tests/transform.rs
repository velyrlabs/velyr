//! JSX to JavaScript: automatic and classic runtimes, config files and per-file pragmas.

use std::fs;

use velyr_runtime_modules::{MediaType, SourceTransform};
use velyr_tooling_jsx::{JsxConfig, JsxTransform};

fn run(config: JsxConfig, specifier: &str, source: &str, media_type: MediaType) -> String {
    JsxTransform::new(config)
        .transform(specifier, source, media_type)
        .unwrap()
        .code
}

const APP: &str = "export const App = () => <div id=\"a\">hi</div>;\n";

#[test]
fn defaults_to_automatic_runtime_from_react() {
    let code = run(JsxConfig::default(), "/m.jsx", APP, MediaType::JavaScript);
    assert!(code.contains("react/jsx-runtime"), "{code}");
    assert!(!code.contains("createElement"), "{code}");
}

#[test]
fn custom_import_source() {
    let config = JsxConfig {
        import_source: Some("preact".into()),
        ..JsxConfig::default()
    };
    let code = run(config, "/m.jsx", APP, MediaType::JavaScript);
    assert!(code.contains("preact/jsx-runtime"), "{code}");
}

#[test]
fn classic_runtime_uses_factory_and_fragment_options() {
    let config = JsxConfig::from_compiler_options(
        &serde_json::json!({"jsx": "react", "jsxFactory": "h", "jsxFragmentFactory": "Frag"}),
    )
    .unwrap();
    let code = run(config, "/m.jsx", "export const a = <><b/></>;\n", MediaType::JavaScript);
    assert!(code.contains("h(Frag, null, /* @__PURE__ */ h(\"b\", null))"), "{code}");
    assert!(!code.contains("jsx-runtime"), "{code}");
}

#[test]
fn pragmas_override_config_per_file() {
    let classic = "/** @jsx h */\nexport const a = <b/>;\n";
    let code = run(JsxConfig::default(), "/m.jsx", &format!("/** @jsxRuntime classic */\n{classic}"), MediaType::JavaScript);
    assert!(code.contains("h(\"b\", null)"), "{code}");
    let source = "/** @jsxImportSource solid */\nexport const a = <b/>;\n";
    let code = run(JsxConfig::default(), "/m.jsx", source, MediaType::JavaScript);
    assert!(code.contains("solid/jsx-runtime"), "{code}");
}

#[test]
fn tsx_strips_types_and_transforms_jsx_in_one_pass() {
    let source = "type P = { n: number };\nexport const A = (p: P) => <i>{p.n}</i>;\n";
    let code = run(JsxConfig::default(), "/m.tsx", source, MediaType::Tsx);
    assert!(!code.contains("type P") && !code.contains(": P"), "{code}");
    assert!(code.contains("react/jsx-runtime"), "{code}");
}

#[test]
fn plain_javascript_is_left_alone() {
    let source = "export const a = 1;\n";
    assert_eq!(run(JsxConfig::default(), "/m.mjs", source, MediaType::JavaScript), source);
}

#[test]
fn rejects_unknown_jsx_mode_and_syntax_errors() {
    let bad = serde_json::json!({"jsx": "preserve"});
    assert!(JsxConfig::from_compiler_options(&bad).is_err());
    let err = JsxTransform::new(JsxConfig::default())
        .transform("/m.jsx", "const a = <div>;", MediaType::JavaScript)
        .unwrap_err();
    assert!(err.to_string().contains("/m.jsx:1:"), "{err}");
}

#[test]
fn velyr_json_wins_and_tsconfig_fills_the_gaps() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("velyr.json"),
        r#"{"compilerOptions": {"jsxImportSource": "preact"}}"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("tsconfig.json"),
        r#"{"compilerOptions": {"jsx": "react", "jsxImportSource": "ignored", "jsxFactory": "h"}}"#,
    )
    .unwrap();
    let nested = dir.path().join("src");
    fs::create_dir(&nested).unwrap();
    let config = JsxConfig::discover(&nested).unwrap();
    assert_eq!(config.import_source.as_deref(), Some("preact"));
    assert_eq!(config.factory.as_deref(), Some("h"));
    assert!(config.classic);
}

#[test]
fn no_config_files_means_defaults() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(JsxConfig::discover(dir.path()).unwrap(), JsxConfig::default());
}
