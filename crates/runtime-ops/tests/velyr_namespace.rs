//! `Velyr.env` and `Velyr.permissions.query` on V8, with capability sets built directly.

mod common;

use std::sync::Arc;

use common::{events, run};
use velyr_runtime_permissions::{Capability, CapabilitySet, NamePattern, PathPattern};

const GRANTED: &str = "CARGO_MANIFEST_DIR";
const UNGRANTED: &str = "CARGO_PKG_NAME";

fn env_read(names: &[&str]) -> Arc<CapabilitySet> {
    let builder = names.iter().fold(CapabilitySet::builder(), |b, n| {
        b.allow(Capability::EnvRead(NamePattern::new(*n)))
    });
    builder.build()
}

fn denied(script: &str, permissions: Arc<CapabilitySet>) -> String {
    run(
        &format!(
            r#"try {{ {script} }} catch (e) {{ throw new Error(e.code + "|" + e.message); }}
            throw new Error("not-denied");"#
        ),
        permissions,
    )
}

#[test]
fn env_get_returns_granted_variable() {
    let value = std::env::var(GRANTED).unwrap();
    let message = run(
        &format!(r#"throw new Error("got:" + Velyr.env.get("{GRANTED}"));"#),
        env_read(&[GRANTED]),
    );
    assert_eq!(message, format!("Error: got:{value}"));
}

#[test]
fn env_get_of_ungranted_variable_is_permission_denied_without_leaking_it() {
    let secret = std::env::var(UNGRANTED).unwrap();
    for name in [UNGRANTED, "HOME"] {
        let message = denied(
            &format!(r#"Velyr.env.get("{name}");"#),
            env_read(&[GRANTED]),
        );
        assert!(message.contains("VELYR_PERMISSION_DENIED"), "{message}");
        assert!(!message.contains(&secret), "{message}");
    }
    assert!(!events().contains(&secret));
}

#[test]
fn env_has_reports_granted_variables_and_denies_the_rest() {
    let message = run(
        &format!(
            r#"throw new Error([Velyr.env.has("{GRANTED}"), Velyr.env.has("VELYR_ABSENT_VAR")].join());"#
        ),
        env_read(&[GRANTED, "VELYR_ABSENT_VAR"]),
    );
    assert_eq!(message, "Error: true,false");
    let message = denied(
        &format!(r#"Velyr.env.has("{UNGRANTED}");"#),
        env_read(&[GRANTED]),
    );
    assert!(message.contains("VELYR_PERMISSION_DENIED"), "{message}");
}

#[test]
fn env_to_object_needs_the_wildcard_grant() {
    let message = denied("Velyr.env.toObject();", env_read(&[GRANTED]));
    assert!(message.contains("VELYR_PERMISSION_DENIED"), "{message}");
}

#[test]
fn env_to_object_omits_explicitly_denied_variables() {
    let permissions = CapabilitySet::builder()
        .allow(Capability::EnvRead(NamePattern::new("*")))
        .deny(Capability::EnvRead(NamePattern::new("CARGO_PKG_*")))
        .build();
    let message = run(
        &format!(
            r#"const o = Velyr.env.toObject();
            throw new Error([typeof o["{GRANTED}"], "{UNGRANTED}" in o].join());"#
        ),
        permissions,
    );
    assert_eq!(message, "Error: string,false");
}

#[test]
fn env_set_needs_env_write_and_never_touches_the_process_environment() {
    let message = denied(
        r#"Velyr.env.set("VELYR_SET_ME", "1");"#,
        env_read(&["VELYR_SET_ME"]),
    );
    assert!(message.contains("VELYR_PERMISSION_DENIED"), "{message}");

    let permissions = CapabilitySet::builder()
        .allow(Capability::EnvRead(NamePattern::new("VELYR_SET_ME")))
        .allow(Capability::EnvWrite(NamePattern::new("VELYR_SET_ME")))
        .build();
    let message = run(
        r#"Velyr.env.set("VELYR_SET_ME", "1"); throw new Error("v=" + Velyr.env.get("VELYR_SET_ME"));"#,
        permissions,
    );
    assert_eq!(message, "Error: v=1");
    assert!(std::env::var("VELYR_SET_ME").is_err());
}

#[test]
fn env_set_rejects_invalid_names_and_values() {
    let permissions = CapabilitySet::builder()
        .allow(Capability::EnvWrite(NamePattern::new("*")))
        .build();
    let message = run(
        r#"const out = [];
        for (const [k, v] of [["", "x"], ["A=B", "x"], ["A", "x\0y"]]) {
          try { Velyr.env.set(k, v); out.push("ok"); } catch (e) { out.push(e.name); }
        }
        throw new Error(out.join());"#,
        permissions,
    );
    assert_eq!(message, "Error: TypeError,TypeError,TypeError");
}

fn query(descriptor: &str, permissions: Arc<CapabilitySet>) -> String {
    run(
        &format!(
            r#"Velyr.permissions.query({descriptor}).then(
              (r) => {{ throw "state=" + r.state; }},
              (e) => {{ throw "rejected=" + e.name; }});"#
        ),
        permissions,
    )
}

fn fs_read(path: &std::path::Path) -> Arc<CapabilitySet> {
    CapabilitySet::builder()
        .allow(Capability::FsRead(PathPattern::new(path.to_str().unwrap())))
        .build()
}

#[test]
fn query_read_is_granted_inside_the_grant_and_denied_outside() {
    let dir = tempfile::tempdir().unwrap();
    let inside = dir.path().join("data");
    let permissions = fs_read(dir.path());
    let granted = query(
        &format!(
            r#"{{ name: "read", path: {:?} }}"#,
            inside.to_str().unwrap()
        ),
        permissions.clone(),
    );
    assert!(granted.contains("state=granted"), "{granted}");
    let outside = query(
        r#"{ name: "read", path: "/definitely/not/granted" }"#,
        permissions.clone(),
    );
    assert!(outside.contains("state=denied"), "{outside}");
    let write = query(
        &format!(
            r#"{{ name: "write", path: {:?} }}"#,
            inside.to_str().unwrap()
        ),
        permissions,
    );
    assert!(write.contains("state=denied"), "{write}");
}

#[test]
fn query_covers_env_net_listen_run_native_and_import() {
    use velyr_runtime_permissions::{AddrPattern, HostPattern, PortRange, ProgramPattern};
    let ports = |p| PortRange { start: p, end: p };
    // Unix absolute paths are drive-relative on Windows.
    let root = if cfg!(windows) { "C:" } else { "" };
    let permissions = CapabilitySet::builder()
        .allow(Capability::EnvRead(NamePattern::new("FOO")))
        .allow(Capability::NetConnect(
            HostPattern::new("example.com"),
            ports(443),
        ))
        .allow(Capability::NetListen(
            AddrPattern::new("127.0.0.1"),
            ports(8080),
        ))
        .allow(Capability::ProcessSpawn(ProgramPattern::new("git")))
        .allow(Capability::NativeLoad(PathPattern::new(format!(
            "{root}/opt/lib"
        ))))
        .allow(Capability::ImportLocal(PathPattern::new(format!(
            "{root}/opt/mods"
        ))))
        .allow(Capability::ImportRemote(HostPattern::new("esm.sh")))
        .build();
    let native = format!(r#"{{ name: "native", path: "{root}/opt/lib/x.so" }}"#);
    let import = format!(r#"{{ name: "import", path: "{root}/opt/mods/a.js" }}"#);
    let cases = [
        (r#"{ name: "env", variable: "FOO" }"#, "granted"),
        (r#"{ name: "env", variable: "BAR" }"#, "denied"),
        (r#"{ name: "env" }"#, "denied"),
        (r#"{ name: "net", host: "example.com:443" }"#, "granted"),
        (r#"{ name: "net", host: "example.com:80" }"#, "denied"),
        (r#"{ name: "listen", host: "127.0.0.1:8080" }"#, "granted"),
        (r#"{ name: "run", command: "git" }"#, "granted"),
        (r#"{ name: "run", command: "rm" }"#, "denied"),
        (native.as_str(), "granted"),
        (import.as_str(), "granted"),
        (r#"{ name: "import", host: "esm.sh" }"#, "granted"),
        (r#"{ name: "import", host: "evil.test" }"#, "denied"),
    ];
    for (descriptor, expected) in cases {
        let message = query(descriptor, permissions.clone());
        assert!(
            message.contains(&format!("state={expected}")),
            "{descriptor}: {message}"
        );
    }
}

#[test]
fn query_rejects_malformed_descriptors() {
    for descriptor in [
        r#"{ name: "nope" }"#,
        r#"{ name: "read" }"#,
        r#"{ name: "net", host: "example.com" }"#,
        r#"{ name: "import" }"#,
        "null",
    ] {
        let message = query(descriptor, CapabilitySet::builder().build());
        assert!(
            message.contains("rejected=TypeError"),
            "{descriptor}: {message}"
        );
    }
}

#[test]
fn query_never_prompts_and_emits_no_denial_event() {
    let marker = "/velyr/query/marker/path";
    let message = query(
        &format!(r#"{{ name: "read", path: "{marker}" }}"#),
        CapabilitySet::builder().build(),
    );
    assert!(message.contains("state=denied"), "{message}");
    assert!(!events().contains(marker));
}
