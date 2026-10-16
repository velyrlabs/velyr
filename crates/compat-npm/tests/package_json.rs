use serde_json::{Value, json};
use velyr_compat_npm::PackageJson;

fn parse(raw: &str) -> PackageJson {
    PackageJson::parse(raw).expect("fixture should parse")
}

#[test]
fn parses_react_fixture() {
    let pkg = parse(include_str!("fixtures/react.json"));
    assert_eq!(pkg.name.as_deref(), Some("react"));
    assert!(pkg.version.is_some());
    assert!(pkg.exports.as_ref().is_some_and(Value::is_object));
}

#[test]
fn parses_vite_fixture() {
    let pkg = parse(include_str!("fixtures/vite.json"));
    assert_eq!(pkg.name.as_deref(), Some("vite"));
    assert_eq!(pkg.r#type.as_deref(), Some("module"));
    assert!(pkg.dependencies.contains_key("postcss"));
    assert!(pkg.imports.is_some());
    assert!(pkg.bin.as_ref().is_some_and(Value::is_object));
    assert!(pkg.engines.contains_key("node"));
    assert!(
        pkg.peer_dependencies_meta
            .values()
            .any(|m| m.optional == Some(true))
    );
}

#[test]
fn parses_lodash_fixture() {
    let pkg = parse(include_str!("fixtures/lodash.json"));
    assert_eq!(pkg.name.as_deref(), Some("lodash"));
    assert_eq!(pkg.main.as_deref(), Some("lodash.js"));
}

#[test]
fn unknown_fields_survive_round_trip() {
    let raw = r#"{"name":"x","version":"1.0.0","funding":"https://f","custom":{"a":[1,2]}}"#;
    let pkg = parse(raw);
    assert_eq!(pkg.extra["custom"], json!({"a": [1, 2]}));

    let reparsed: Value = serde_json::from_str(&serde_json::to_string(&pkg).unwrap()).unwrap();
    let original: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(reparsed, original);
}

#[test]
fn polymorphic_fields_accept_string_object_and_array() {
    let pkg = parse(r#"{"exports":"./i.js","bin":"cli.js","workspaces":["packages/*"]}"#);
    assert_eq!(pkg.exports, Some(json!("./i.js")));
    assert_eq!(pkg.bin, Some(json!("cli.js")));
    assert_eq!(pkg.workspaces, Some(json!(["packages/*"])));

    let pkg = parse(r#"{"exports":["./a.js","./b.js"],"workspaces":{"packages":["p/*"]}}"#);
    assert!(pkg.exports.as_ref().is_some_and(Value::is_array));
    assert!(pkg.workspaces.as_ref().is_some_and(Value::is_object));
}

#[test]
fn empty_object_and_invalid_json() {
    assert_eq!(parse("{}").name, None);
    assert!(PackageJson::parse("not json").is_err());
    assert!(PackageJson::parse("[]").is_err());
}
