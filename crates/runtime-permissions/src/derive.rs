//! Authority intersection: `child = parent ∩ requested ∩ policy` (ADR-014).
//!
//! The child keeps the three sets as bounds and allows a request only when all three allow it.
//! Each bound applies its own deny-over-allow rule, so denies are united and nothing is ever
//! widened. Exact by construction: no pattern intersection is computed, so no representability
//! gap exists. Monotonic: `child == Allow` implies each bound `== Allow`.

use std::sync::Arc;

use crate::CapabilitySet;

impl CapabilitySet {
    pub fn derive_child(&self, requested: &Self, policy: &Self) -> Arc<Self> {
        Arc::new(Self {
            bounds: [self, requested, policy]
                .map(|s| Arc::new(s.clone()))
                .into(),
            ..Self::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AddrPattern, Capability, HostPattern, NamePattern, PathPattern, PortRange, ProgramPattern,
        Request,
    };
    use proptest::prelude::*;
    use std::net::SocketAddr;

    fn set(allow: Vec<Capability>, deny: Vec<Capability>) -> Arc<CapabilitySet> {
        let mut b = CapabilitySet::builder();
        for c in allow {
            b = b.allow(c);
        }
        for c in deny {
            b = b.deny(c);
        }
        b.build()
    }

    fn fs_read(p: &str) -> Capability {
        Capability::FsRead(PathPattern::new(p))
    }

    fn env_read(p: &str) -> Capability {
        Capability::EnvRead(NamePattern::new(p))
    }

    fn can_read(s: &CapabilitySet, p: &str) -> bool {
        s.check(&Request::FsRead(p)).is_allow()
    }

    #[test]
    fn narrower_parent_bounds_broader_request() {
        let parent = set(vec![fs_read(abs!("/app"))], vec![]);
        let requested = set(vec![fs_read(abs!("/"))], vec![]);
        let policy = set(vec![fs_read(abs!("/"))], vec![]);
        let child = parent.derive_child(&requested, &policy);
        assert!(can_read(&child, abs!("/app/x")));
        assert!(!can_read(&child, abs!("/etc/passwd")));
        assert!(!can_read(&child, abs!("/application")));
    }

    #[test]
    fn narrower_request_and_policy_bound_the_child() {
        let all = set(vec![fs_read(abs!("/"))], vec![]);
        let narrow = set(vec![fs_read(abs!("/app/a"))], vec![]);
        let by_request = all.derive_child(&narrow, &all);
        let by_policy = all.derive_child(&all, &narrow);
        for child in [by_request, by_policy] {
            assert!(can_read(&child, abs!("/app/a/f")));
            assert!(!can_read(&child, abs!("/app/b")));
        }
    }

    #[test]
    fn never_a_union() {
        let a = set(vec![env_read("A")], vec![]);
        let b = set(vec![env_read("B")], vec![]);
        let all = set(vec![env_read("*")], vec![]);
        for child in [a.derive_child(&b, &all), a.derive_child(&all, &b)] {
            assert!(!child.check(&Request::EnvRead("A")).is_allow());
            assert!(!child.check(&Request::EnvRead("B")).is_allow());
        }
    }

    #[test]
    fn deny_in_any_of_the_three_applies() {
        let open = set(vec![env_read("*")], vec![]);
        let deny_secret = set(vec![env_read("*")], vec![env_read("SECRET")]);
        let children = [
            deny_secret.derive_child(&open, &open),
            open.derive_child(&deny_secret, &open),
            open.derive_child(&open, &deny_secret),
        ];
        for child in children {
            assert!(child.check(&Request::EnvRead("HOME")).is_allow());
            assert!(!child.check(&Request::EnvRead("SECRET")).is_allow());
        }
    }

    #[test]
    fn identical_sets_keep_the_grant_and_chains_stay_bounded() {
        let s = set(vec![fs_read(abs!("/app"))], vec![]);
        let child = s.derive_child(&s, &s);
        assert!(can_read(&child, abs!("/app/x")));
        let grandchild = child.derive_child(&set(vec![fs_read(abs!("/"))], vec![]), &s);
        assert!(can_read(&grandchild, abs!("/app/x")));
        assert!(!can_read(&grandchild, abs!("/etc")));
    }

    #[test]
    fn network_two_phase_checks_require_all_three() {
        let ports = PortRange { start: 1, end: 1 };
        let allow = |h: &str| vec![Capability::NetConnect(HostPattern::new(h), ports)];
        let wide = set(allow("*.a.com"), vec![]);
        let narrow = set(allow("x.a.com"), vec![]);
        let child = wide.derive_child(&narrow, &wide);
        assert!(child.check_connect_host("x.a.com", 1).is_allow());
        assert!(!child.check_connect_host("y.a.com", 1).is_allow());
        let addr: SocketAddr = "8.8.8.8:1".parse().unwrap();
        assert!(child.check_connect_addr("x.a.com", addr).is_allow());
        assert!(!child.check_connect_addr("y.a.com", addr).is_allow());
    }

    fn seg() -> impl Strategy<Value = &'static str> {
        prop_oneof![Just("a"), Just("b"), Just("c"), Just("..")]
    }

    fn path() -> impl Strategy<Value = String> {
        prop::collection::vec(seg(), 0..4)
            .prop_map(|v| format!("{}/velyr-prop/{}", abs!(""), v.join("/")))
    }

    fn name() -> impl Strategy<Value = &'static str> {
        prop_oneof![
            Just("A"),
            Just("AB"),
            Just("B"),
            Just("A*"),
            Just("*"),
            Just("")
        ]
    }

    fn host() -> impl Strategy<Value = &'static str> {
        prop_oneof![
            Just("a.com"),
            Just("x.a.com"),
            Just("b.com"),
            Just("*.a.com"),
            Just("*"),
            Just("127.0.0.1"),
            Just("localhost"),
            Just("10.1.2.3"),
            Just("10.0.0.0/8"),
            Just("10.1.0.0/16"),
            Just("8.8.8.8"),
            Just("bad host!"),
        ]
    }

    fn ports() -> impl Strategy<Value = PortRange> {
        (1u16..4, 1u16..4).prop_map(|(start, end)| PortRange { start, end })
    }

    fn capability() -> impl Strategy<Value = Capability> {
        let pp = || path().prop_map(PathPattern::new);
        prop_oneof![
            pp().prop_map(Capability::FsRead),
            pp().prop_map(Capability::FsWrite),
            pp().prop_map(Capability::ImportLocal),
            pp().prop_map(Capability::NativeLoad),
            (host(), ports()).prop_map(|(h, p)| Capability::NetConnect(HostPattern::new(h), p)),
            (host(), ports()).prop_map(|(h, p)| Capability::NetListen(AddrPattern::new(h), p)),
            host().prop_map(|h| Capability::ImportRemote(HostPattern::new(h))),
            host().prop_map(|h| Capability::InspectorBind(AddrPattern::new(h))),
            name().prop_map(|n| Capability::EnvRead(NamePattern::new(n))),
            name().prop_map(|n| Capability::EnvWrite(NamePattern::new(n))),
            name().prop_map(|n| Capability::ProcessSpawn(ProgramPattern::new(n))),
            name().prop_map(|n| Capability::SysInfo(NamePattern::new(n))),
        ]
    }

    fn capability_set() -> impl Strategy<Value = Arc<CapabilitySet>> {
        (
            prop::collection::vec(capability(), 0..5),
            prop::collection::vec(capability(), 0..2),
        )
            .prop_map(|(allow, deny)| set(allow, deny))
    }

    /// `(kind selector, text, port)` turned into a `Request` by the test body.
    fn request_parts() -> impl Strategy<Value = (u8, String, u16)> {
        let text = prop_oneof![
            path(),
            host().prop_map(str::to_owned),
            name().prop_map(str::to_owned)
        ];
        (0u8..12, text, 0u16..5)
    }

    fn build_request(kind: u8, text: &str, port: u16) -> Request<'_> {
        match kind {
            0 => Request::FsRead(text),
            1 => Request::FsWrite(text),
            2 => Request::ImportLocal(text),
            3 => Request::NativeLoad(text),
            4 => Request::NetConnect(text, port),
            5 => Request::NetListen(text, port),
            6 => Request::ImportRemote(text),
            7 => Request::InspectorBind(text),
            8 => Request::EnvRead(text),
            9 => Request::EnvWrite(text),
            10 => Request::SysInfo(text),
            _ => Request::ProcessSpawn(text),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10_000))]

        #[test]
        fn child_allow_implies_all_three_allow(
            parent in capability_set(),
            requested in capability_set(),
            policy in capability_set(),
            (kind, text, port) in request_parts(),
        ) {
            let child = parent.derive_child(&requested, &policy);
            let request = build_request(kind, &text, port);
            if child.check(&request).is_allow() {
                prop_assert!(parent.check(&request).is_allow());
                prop_assert!(requested.check(&request).is_allow());
                prop_assert!(policy.check(&request).is_allow());
            }
        }

        #[test]
        fn child_connect_phases_imply_all_three(
            parent in capability_set(),
            requested in capability_set(),
            policy in capability_set(),
            host in host(),
            port in 0u16..5,
            ip in prop_oneof![Just("127.0.0.1"), Just("10.1.2.3"), Just("8.8.8.8")],
        ) {
            let child = parent.derive_child(&requested, &policy);
            let addr: SocketAddr = format!("{ip}:{port}").parse().unwrap();
            let all = [&*parent, &*requested, &*policy];
            if child.check_connect_host(host, port).is_allow() {
                prop_assert!(all.iter().all(|s| s.check_connect_host(host, port).is_allow()));
            }
            if child.check_connect_addr(host, addr).is_allow() {
                prop_assert!(all.iter().all(|s| s.check_connect_addr(host, addr).is_allow()));
            }
        }
    }
}
