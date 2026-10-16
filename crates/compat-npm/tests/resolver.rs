//! Resolver tests against an in-memory registry (no network, no disk).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use velyr_compat_npm::PackageJson;
use velyr_compat_npm::registry::Packument;
use velyr_compat_npm::resolver::{
    InstallPlan, PackumentSource, PlanWarning, Platform, ResolveOptions, resolve,
};
use velyr_runtime_errors::{ErrorCode, VelyrError};

#[derive(Default)]
struct FakeRegistry(BTreeMap<String, Arc<Packument>>);

impl FakeRegistry {
    /// `versions`: `(version, manifest extras)`; `tags`: `(tag, version)`.
    fn with(mut self, name: &str, tags: &[(&str, &str)], versions: &[(&str, Value)]) -> Self {
        let mut manifests = serde_json::Map::new();
        for (version, extra) in versions {
            let mut manifest = json!({
                "name": name,
                "version": version,
                "dist": { "tarball": format!("https://r/{name}-{version}.tgz") },
            });
            manifest
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            manifests.insert((*version).to_owned(), manifest);
        }
        let tags: BTreeMap<_, _> = tags.iter().map(|(t, v)| (*t, *v)).collect();
        let packument = json!({ "name": name, "dist-tags": tags, "versions": manifests });
        self.0.insert(
            name.to_owned(),
            Arc::new(serde_json::from_value(packument).unwrap()),
        );
        self
    }
}

impl PackumentSource for FakeRegistry {
    fn packument(&self, name: &str) -> Result<Arc<Packument>, VelyrError> {
        self.0
            .get(name)
            .cloned()
            .ok_or_else(|| VelyrError::not_found(format!("{name} not in registry")))
    }
}

fn root(deps: Value) -> PackageJson {
    serde_json::from_value(json!({ "name": "app", "version": "1.0.0", "dependencies": deps }))
        .unwrap()
}

fn options() -> ResolveOptions {
    ResolveOptions {
        platform: Platform::new("linux", "x64"),
        strict_peer_deps: false,
    }
}

fn plan(registry: &FakeRegistry, root: &PackageJson) -> InstallPlan {
    resolve(root, registry, &options()).unwrap()
}

fn versions(plan: &InstallPlan) -> Vec<(String, String)> {
    plan.packages
        .iter()
        .map(|(path, p)| (path.clone(), format!("{}@{}", p.name, p.version)))
        .collect()
}

fn at(plan: &InstallPlan, path: &str) -> String {
    let p = plan
        .packages
        .get(path)
        .unwrap_or_else(|| panic!("{path} not planned: {:?}", versions(plan)));
    format!("{}@{}", p.name, p.version)
}

#[test]
fn conflicting_versions_hoist_first_and_nest_second() {
    let registry = FakeRegistry::default()
        .with("a", &[], &[("1.0.0", json!({"dependencies": {"c": "1"}}))])
        .with("b", &[], &[("1.0.0", json!({"dependencies": {"c": "2"}}))])
        .with("c", &[], &[("1.0.0", json!({})), ("2.0.0", json!({}))]);
    let plan = plan(&registry, &root(json!({"a": "1", "b": "1"})));
    assert_eq!(plan.packages.len(), 4);
    assert_eq!(at(&plan, "node_modules/c"), "c@1.0.0");
    assert_eq!(at(&plan, "node_modules/b/node_modules/c"), "c@2.0.0");
}

#[test]
fn plan_is_byte_identical_regardless_of_registry_insertion_order() {
    let deps = json!({"a": "^1", "b": "^1", "c": "*"});
    let build = |order: &[&str]| {
        let mut registry = FakeRegistry::default();
        for name in order {
            registry = match *name {
                "a" => registry.with(
                    "a",
                    &[("latest", "1.1.0")],
                    &[
                        ("1.0.0", json!({})),
                        ("1.1.0", json!({"dependencies": {"c": "1"}})),
                    ],
                ),
                "b" => registry.with("b", &[], &[("1.0.0", json!({"dependencies": {"c": "2"}}))]),
                _ => registry.with("c", &[], &[("1.0.0", json!({})), ("2.0.0", json!({}))]),
            };
        }
        serde_json::to_string(&plan(&registry, &root(deps.clone()))).unwrap()
    };
    assert_eq!(build(&["a", "b", "c"]), build(&["c", "b", "a"]));
}

#[test]
fn shared_dependency_is_deduplicated_to_highest_satisfying() {
    let registry = FakeRegistry::default()
        .with(
            "a",
            &[],
            &[("1.0.0", json!({"dependencies": {"c": "^1.0.0"}}))],
        )
        .with(
            "b",
            &[],
            &[("1.0.0", json!({"dependencies": {"c": "^1.1.0"}}))],
        )
        .with(
            "c",
            &[],
            &[
                ("1.0.0", json!({})),
                ("1.1.0", json!({})),
                ("1.2.0", json!({})),
                ("2.0.0", json!({})),
            ],
        );
    let plan = plan(&registry, &root(json!({"a": "1", "b": "1"})));
    assert_eq!(plan.packages.len(), 3);
    assert_eq!(at(&plan, "node_modules/c"), "c@1.2.0");
}

#[test]
fn dist_tags_resolve_to_tagged_version() {
    let registry = FakeRegistry::default().with(
        "a",
        &[("latest", "1.0.0"), ("next", "2.0.0-beta.1")],
        &[("1.0.0", json!({})), ("2.0.0-beta.1", json!({}))],
    );
    let plan = plan(&registry, &root(json!({"a": "next"})));
    assert_eq!(at(&plan, "node_modules/a"), "a@2.0.0-beta.1");
}

#[test]
fn failing_optional_dependency_becomes_warning() {
    let registry = FakeRegistry::default()
        .with(
            "a",
            &[],
            &[(
                "1.0.0",
                json!({"optionalDependencies": {"ghost": "1", "winonly": "1"}}),
            )],
        )
        .with("winonly", &[], &[("1.0.0", json!({"os": ["win32"]}))]);
    let plan = plan(&registry, &root(json!({"a": "1"})));
    assert_eq!(
        versions(&plan),
        [("node_modules/a".into(), "a@1.0.0".into())]
    );
    let skipped: Vec<_> = plan
        .warnings
        .iter()
        .map(|w| match w {
            PlanWarning::OptionalSkipped {
                package,
                required_by,
                ..
            } => (package.as_str(), required_by.as_str()),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(skipped, [("ghost", "a@1.0.0"), ("winonly", "a@1.0.0")]);
}

#[test]
fn optional_dependency_matching_platform_is_installed() {
    let registry = FakeRegistry::default().with(
        "fsevents",
        &[],
        &[("1.0.0", json!({"os": ["!win32"], "cpu": ["x64"]}))],
    );
    let root: PackageJson =
        serde_json::from_value(json!({"optionalDependencies": {"fsevents": "1"}})).unwrap();
    let plan = plan(&registry, &root);
    assert_eq!(at(&plan, "node_modules/fsevents"), "fsevents@1.0.0");
    assert!(plan.warnings.is_empty());
}

#[test]
fn required_dependency_failures_are_errors() {
    let registry =
        FakeRegistry::default().with("winonly", &[], &[("1.0.0", json!({"os": ["win32"]}))]);
    let err = resolve(&root(json!({"winonly": "1"})), &registry, &options()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Unsupported);
    let err = resolve(&root(json!({"ghost": "1"})), &registry, &options()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[test]
fn missing_peer_is_installed_automatically_but_optional_peer_is_not() {
    let registry = FakeRegistry::default()
        .with(
            "plugin",
            &[],
            &[(
                "1.0.0",
                json!({
                    "peerDependencies": {"host": "^2", "extra": "1"},
                    "peerDependenciesMeta": {"extra": {"optional": true}},
                }),
            )],
        )
        .with("host", &[], &[("2.3.0", json!({})), ("3.0.0", json!({}))])
        .with("extra", &[], &[("1.0.0", json!({}))]);
    let plan = plan(&registry, &root(json!({"plugin": "1"})));
    assert_eq!(at(&plan, "node_modules/host"), "host@2.3.0");
    assert!(!plan.packages.contains_key("node_modules/extra"));
    assert!(plan.warnings.is_empty());
}

fn conflicting_peer_registry() -> FakeRegistry {
    FakeRegistry::default()
        .with(
            "plugin",
            &[],
            &[("1.0.0", json!({"peerDependencies": {"host": "^2"}}))],
        )
        .with("host", &[], &[("2.0.0", json!({})), ("3.0.0", json!({}))])
}

#[test]
fn peer_conflict_is_a_warning_by_default_and_error_when_strict() {
    let registry = conflicting_peer_registry();
    let app = root(json!({"plugin": "1", "host": "3"}));
    let plan = plan(&registry, &app);
    assert_eq!(at(&plan, "node_modules/host"), "host@3.0.0");
    assert_eq!(
        at(&plan, "node_modules/plugin/node_modules/host"),
        "host@2.0.0"
    );
    assert_eq!(
        plan.warnings,
        [PlanWarning::PeerConflict {
            package: "plugin@1.0.0".into(),
            peer: "host".into(),
            range: "^2".into(),
            found: "3.0.0".into(),
            required_by: "root".into(),
        }]
    );
    let strict = ResolveOptions {
        strict_peer_deps: true,
        ..options()
    };
    assert!(resolve(&app, &registry, &strict).is_err());
}

#[test]
fn nesting_never_shadows_a_copy_another_package_relies_on() {
    let registry = FakeRegistry::default()
        .with(
            "a",
            &[],
            &[("1.0.0", json!({"dependencies": {"p": "2", "q": "2"}}))],
        )
        .with(
            "p",
            &[],
            &[
                ("1.0.0", json!({})),
                ("2.0.0", json!({"dependencies": {"c": "1"}})),
            ],
        )
        .with(
            "q",
            &[],
            &[
                ("1.0.0", json!({})),
                ("2.0.0", json!({"dependencies": {"c": "2"}})),
            ],
        )
        .with("c", &[], &[("1.0.0", json!({})), ("2.0.0", json!({}))]);
    let plan = plan(
        &registry,
        &root(json!({"a": "1", "c": "1", "p": "1", "q": "1"})),
    );
    assert_eq!(at(&plan, "node_modules/c"), "c@1.0.0");
    assert_eq!(
        at(&plan, "node_modules/a/node_modules/q/node_modules/c"),
        "c@2.0.0"
    );
    assert!(!plan.packages.contains_key("node_modules/a/node_modules/c"));
}

#[test]
fn dependency_cycles_terminate() {
    let registry = FakeRegistry::default()
        .with("a", &[], &[("1.0.0", json!({"dependencies": {"b": "1"}}))])
        .with("b", &[], &[("1.0.0", json!({"dependencies": {"a": "1"}}))]);
    assert_eq!(plan(&registry, &root(json!({"a": "1"}))).packages.len(), 2);
}
