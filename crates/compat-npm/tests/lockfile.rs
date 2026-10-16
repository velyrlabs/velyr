use velyr_compat_npm::PackageJson;
use velyr_compat_npm::lockfile::Lockfile;
use velyr_runtime_errors::ErrorCode;

const CANONICAL: &str = r#"{
  "importers": {
    ".": {
      "dependencies": {
        "left-pad": "^1.3.0"
      },
      "devDependencies": {
        "vite": "^5.0.0"
      }
    }
  },
  "lockfileVersion": 1,
  "packages": {
    "fsevents@2.3.3": {
      "cpu": [
        "arm64"
      ],
      "hasInstallScript": true,
      "integrity": "sha512-aaa",
      "os": [
        "darwin"
      ],
      "resolved": "https://registry.npmjs.org/fsevents/-/fsevents-2.3.3.tgz"
    },
    "left-pad@1.3.0": {
      "integrity": "sha512-bbb",
      "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz"
    }
  }
}
"#;

fn package_json(raw: &str) -> PackageJson {
    PackageJson::parse(raw).unwrap()
}

#[test]
fn round_trip_is_identical() {
    let lock = Lockfile::parse(CANONICAL).unwrap();
    assert_eq!(lock.render(), CANONICAL);
}

#[test]
fn unordered_input_is_written_sorted_at_every_level() {
    let unordered = r#"{
      "packages": {
        "z@1.0.0": {"resolved": "r", "integrity": "i", "os": ["linux"], "dependencies": {"b": "1", "a": "2"}},
        "a@1.0.0": {"resolved": "r", "integrity": "i"}
      },
      "lockfileVersion": 1,
      "importers": {".": {"devDependencies": {"y": "1", "x": "1"}, "dependencies": {"q": "1"}}}
    }"#;
    let out = Lockfile::parse(unordered).unwrap().render();
    let pos = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("missing {needle}"))
    };
    assert!(pos("\"importers\"") < pos("\"lockfileVersion\""));
    assert!(pos("\"lockfileVersion\"") < pos("\"packages\""));
    assert!(pos("\"dependencies\": {\n        \"q\"") < pos("\"devDependencies\""));
    assert!(pos("\"x\"") < pos("\"y\""));
    assert!(pos("\"a@1.0.0\"") < pos("\"z@1.0.0\""));
    assert!(pos("\"a\": \"2\"") < pos("\"b\": \"1\""));
    assert!(pos("\"dependencies\": {\n        \"a\"") < pos("\"integrity\": \"i\",\n      \"os\""));
    assert_eq!(Lockfile::parse(&out).unwrap().render(), out);
}

#[test]
fn frozen_fails_listing_changed_added_and_removed_packages() {
    let lock = Lockfile::parse(CANONICAL).unwrap();
    let pkg = package_json(
        r#"{"dependencies":{"left-pad":"^2.0.0","chalk":"^5.0.0"},"devDependencies":{}}"#,
    );
    let err = lock.check_frozen(&pkg).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    let msg = err.message();
    assert!(
        msg.contains("~ left-pad: lock has ^1.3.0, package.json has ^2.0.0"),
        "{msg}"
    );
    assert!(msg.contains("+ chalk@^5.0.0 added"), "{msg}");
    assert!(msg.contains("- vite@^5.0.0 removed"), "{msg}");
}

#[test]
fn frozen_passes_when_specs_match() {
    let lock = Lockfile::parse(CANONICAL).unwrap();
    let pkg = package_json(
        r#"{"dependencies":{"left-pad":"^1.3.0"},"devDependencies":{"vite":"^5.0.0"}}"#,
    );
    lock.check_frozen(&pkg).unwrap();
    assert!(lock.is_satisfied_by(&pkg));
    assert!(!lock.is_satisfied_by(&package_json(r#"{"dependencies":{"left-pad":"^1.4.0"}}"#)));
}

#[test]
fn malformed_or_unknown_version_is_invalid_data() {
    for raw in [
        "not json",
        r#"{"importers":{},"packages":{}}"#,
        r#"{"importers":{},"lockfileVersion":2,"packages":{}}"#,
        r#"{"importers":{},"lockfileVersion":1,"packages":{"a@1.0.0":{}}}"#,
    ] {
        let err = Lockfile::parse(raw).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidData, "{raw}");
    }
}

#[test]
fn write_then_read_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("velyr.lock");
    Lockfile::parse(CANONICAL).unwrap().write(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), CANONICAL);
    assert_eq!(Lockfile::read(&path).unwrap().render(), CANONICAL);
    assert_eq!(
        Lockfile::read(&dir.path().join("missing.lock"))
            .unwrap_err()
            .code(),
        ErrorCode::NotFound
    );
}

#[test]
fn a_complete_lock_round_trips_and_rebuilds_packuments_only_when_complete() {
    let raw = r#"{
  "complete": true,
  "importers": {},
  "lockfileVersion": 1,
  "packages": {
    "@s/a@1.0.0": {
      "bin": {
        "a": "cli.js"
      },
      "dependencies": {
        "b": "^2.0.0"
      },
      "integrity": "sha512-aaa",
      "peerDependencies": {
        "c": "*"
      },
      "resolved": "https://r/a.tgz"
    }
  }
}
"#;
    let lock = Lockfile::parse(raw).unwrap();
    assert_eq!(lock.render(), raw);
    let packuments = lock.packuments().unwrap();
    let manifest = &packuments["@s/a"].versions["1.0.0"];
    assert_eq!(manifest.dependencies["b"], "^2.0.0");
    assert_eq!(manifest.dist.tarball, "https://r/a.tgz");
    assert!(lock.without_graph().packuments().is_none());
    assert!(Lockfile::parse(CANONICAL).unwrap().packuments().is_none());
}
