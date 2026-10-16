//! Network policy: host/IP/CIDR/port matching, IPv4-mapped normalization, DNS rebinding guard.
//!
//! Two-phase connect: phase 1 (`check_connect_host`) judges the hostname before DNS; phase 2
//! (`check_connect_addr`) revalidates each resolved IP. A grant by hostname never reaches a
//! non-public IP unless an explicit IP/CIDR grant covers it or the origin is exactly `localhost`
//! (loopback only). A bare `*` is a hostname grant, so it does not reach private literals either.
//!
//! Known limits: embedded-IPv4 forms other than `::ffff:` (NAT64 `64:ff9b::/96`, 6to4) are only
//! checked for the non-public test, not normalized for matching; IPv6 zone ids are rejected.
//! Callers must connect to the exact `SocketAddr` that passed phase 2 (no second resolution).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::{Capability, CapabilitySet, Decision, PortRange, Request};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Matcher {
    Any,
    Exact(String),
    /// Domain without the `*.`; covers strict subdomains only.
    Subdomain(String),
    Ip(IpAddr),
    Cidr(IpAddr, u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    Name(String),
    Ip(IpAddr),
}

impl Matcher {
    fn covers_ip(&self, ip: IpAddr) -> bool {
        match self {
            Matcher::Any => true,
            Matcher::Ip(granted) => *granted == ip,
            Matcher::Cidr(net, bits) => in_cidr(*net, *bits, ip),
            Matcher::Exact(_) | Matcher::Subdomain(_) => false,
        }
    }

    fn covers_host(&self, host: &Host) -> bool {
        match (self, host) {
            (_, Host::Ip(ip)) => self.covers_ip(*ip),
            (Matcher::Any, Host::Name(_)) => true,
            (Matcher::Exact(granted), Host::Name(name)) => granted == name,
            (Matcher::Subdomain(domain), Host::Name(name)) => name
                .strip_suffix(domain.as_str())
                .is_some_and(|prefix| prefix.ends_with('.')),
            _ => false,
        }
    }

    fn is_ip_grant(&self) -> bool {
        matches!(self, Matcher::Ip(_) | Matcher::Cidr(..))
    }
}

macro_rules! net_pattern {
    ($name:ident, $parse:ident) => {
        /// Parsed when the set is built; an unparseable pattern covers nothing (allow)
        /// or everything (deny).
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name {
            raw: String,
            resolved: Option<Matcher>,
        }

        impl $name {
            pub fn new(pattern: impl Into<String>) -> Self {
                Self {
                    raw: pattern.into(),
                    resolved: None,
                }
            }

            /// The flag tells whether the pattern was valid.
            fn resolve(self, deny: bool) -> (Self, bool) {
                let parsed = $parse(&self.raw);
                let valid = parsed.is_some();
                let resolved = parsed.or(deny.then_some(Matcher::Any));
                (Self { resolved, ..self }, valid)
            }
        }
    };
}

net_pattern!(HostPattern, parse_host_pattern);
net_pattern!(AddrPattern, parse_addr_pattern);

impl PortRange {
    fn contains(&self, port: u16) -> bool {
        (self.start..=self.end).contains(&port)
    }
}

/// An invalid or inverted deny range covers every port (fail closed).
fn resolve_ports(ports: PortRange, valid: bool, deny: bool) -> PortRange {
    if deny && (!valid || ports.start > ports.end) {
        return PortRange {
            start: 0,
            end: u16::MAX,
        };
    }
    ports
}

impl Capability {
    pub(crate) fn resolve_net(self, deny: bool) -> Self {
        match self {
            Capability::NetConnect(host, ports) => {
                let (host, valid) = host.resolve(deny);
                Capability::NetConnect(host, resolve_ports(ports, valid, deny))
            }
            Capability::NetListen(addr, ports) => {
                let (addr, valid) = addr.resolve(deny);
                Capability::NetListen(addr, resolve_ports(ports, valid, deny))
            }
            Capability::ImportRemote(host) => Capability::ImportRemote(host.resolve(deny).0),
            Capability::InspectorBind(addr) => Capability::InspectorBind(addr.resolve(deny).0),
            other => other,
        }
    }
}

fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// Last label numeric (`127.1`, `2130706433`) or hex (`0x7f.1`): resolvers read these as IPv4.
fn looks_numeric(name: &str) -> bool {
    let last = name.rsplit('.').next().unwrap_or("");
    last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x") || last.starts_with("0X")
}

/// IP literal (IPv4-mapped unwrapped, `[v6]` brackets accepted) or lowercase IDNA ASCII name
/// with one trailing dot removed. `None` means invalid: callers must deny.
fn parse_host(raw: &str) -> Option<Host> {
    if let Some(inner) = raw.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let v6: Ipv6Addr = inner.parse().ok()?;
        return Some(Host::Ip(unmap(v6.into())));
    }
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Some(Host::Ip(unmap(ip)));
    }
    let name = idna::domain_to_ascii_strict(raw.strip_suffix('.').unwrap_or(raw)).ok()?;
    (!looks_numeric(&name)).then_some(Host::Name(name))
}

fn parse_cidr(raw: &str) -> Option<Matcher> {
    let (ip, bits) = raw.split_once('/')?;
    if bits.is_empty() || !bits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let bits: u8 = bits.parse().ok()?;
    match ip.parse::<IpAddr>().ok()? {
        ip @ IpAddr::V4(_) => (bits <= 32).then_some(Matcher::Cidr(ip, bits)),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => (96..=128)
                .contains(&bits)
                .then(|| Matcher::Cidr(IpAddr::V4(v4), bits - 96)),
            None => (bits <= 128).then_some(Matcher::Cidr(IpAddr::V6(v6), bits)),
        },
    }
}

fn parse_addr_pattern(raw: &str) -> Option<Matcher> {
    if raw.contains('/') {
        return parse_cidr(raw);
    }
    match parse_host(raw)? {
        Host::Ip(ip) => Some(Matcher::Ip(ip)),
        Host::Name(_) => None,
    }
}

fn parse_host_pattern(raw: &str) -> Option<Matcher> {
    if raw == "*" {
        return Some(Matcher::Any);
    }
    if let Some(domain) = raw.strip_prefix("*.") {
        return match parse_host(domain)? {
            Host::Name(name) => Some(Matcher::Subdomain(name)),
            Host::Ip(_) => None,
        };
    }
    if raw.contains('/') {
        return parse_cidr(raw);
    }
    Some(match parse_host(raw)? {
        Host::Name(name) => Matcher::Exact(name),
        Host::Ip(ip) => Matcher::Ip(ip),
    })
}

fn in_cidr(net: IpAddr, bits: u8, ip: IpAddr) -> bool {
    match (net, ip) {
        (IpAddr::V4(net), IpAddr::V4(ip)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(bits)).unwrap_or(0);
            (u32::from(net) ^ u32::from(ip)) & mask == 0
        }
        (IpAddr::V6(net), IpAddr::V6(ip)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(bits)).unwrap_or(0);
            (u128::from(net) ^ u128::from(ip)) & mask == 0
        }
        _ => false,
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a >= 240
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_documentation()
        || (a == 100 && b & 0xc0 == 64)
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && b & 0xfe == 18))
}

/// Not loopback/private/link-local/unspecified/CGNAT/ULA/reserved; embedded IPv4 is unwrapped.
fn is_public(ip: IpAddr) -> bool {
    let IpAddr::V6(v6) = ip else {
        return matches!(ip, IpAddr::V4(v4) if is_public_v4(v4));
    };
    let s = v6.segments();
    let embedded =
        |hi: u16, lo: u16| is_public_v4(Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)));
    if s[..6] == [0; 6] || v6.is_multicast() || s[0] & 0xfe00 == 0xfc00 || s[0] & 0xffc0 == 0xfe80 {
        return false;
    }
    match s {
        [0x2001, 0x0db8, ..] => false,
        [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] => embedded(hi, lo),
        [0x2002, hi, lo, ..] => embedded(hi, lo),
        _ => true,
    }
}

fn connect_matchers(caps: &[Capability], port: u16) -> impl Iterator<Item = &Matcher> {
    caps.iter().filter_map(move |cap| match cap {
        Capability::NetConnect(host, ports) if ports.contains(port) => host.resolved.as_ref(),
        _ => None,
    })
}

impl CapabilitySet {
    /// Phase 1: judge the hostname (or IP literal) before any DNS lookup.
    pub fn check_connect_host(&self, host: &str, port: u16) -> Decision {
        let allowed = if self.bounds.is_empty() {
            parse_host(host).is_some_and(|h| self.connect_allowed(&h, port, None))
        } else {
            self.bounds
                .iter()
                .all(|b| b.check_connect_host(host, port).is_allow())
        };
        Self::net_decision(allowed)
    }

    /// Phase 2: revalidate one resolved address. `origin` is the hostname phase 1 judged
    /// (or the IP literal itself); an invalid origin, or a literal that differs from `addr`, denies.
    pub fn check_connect_addr(&self, origin: &str, addr: SocketAddr) -> Decision {
        let ip = unmap(addr.ip());
        let allowed = if self.bounds.is_empty() {
            parse_host(origin).is_some_and(|h| self.connect_allowed(&h, addr.port(), Some(ip)))
        } else {
            self.bounds
                .iter()
                .all(|b| b.check_connect_addr(origin, addr).is_allow())
        };
        Self::net_decision(allowed)
    }

    fn net_decision(allowed: bool) -> Decision {
        if allowed {
            Decision::Allow
        } else {
            Decision::denied("net-connect")
        }
    }

    fn connect_allowed(&self, origin: &Host, port: u16, ip: Option<IpAddr>) -> bool {
        let ip = match (origin, ip) {
            (Host::Ip(literal), Some(resolved)) if *literal != resolved => return false,
            (Host::Ip(literal), _) => Some(*literal),
            (Host::Name(_), resolved) => resolved,
        };
        let denied = connect_matchers(&self.deny, port)
            .any(|m| m.covers_host(origin) || ip.is_some_and(|ip| m.covers_ip(ip)));
        if denied || !connect_matchers(&self.allow, port).any(|m| m.covers_host(origin)) {
            return false;
        }
        let Some(ip) = ip else { return true };
        is_public(ip)
            || connect_matchers(&self.allow, port).any(|m| m.is_ip_grant() && m.covers_ip(ip))
            || (matches!(origin, Host::Name(name) if name == "localhost") && ip.is_loopback())
    }

    fn allowed_by<'a>(
        &'a self,
        select: impl Fn(&'a Capability) -> Option<&'a Matcher>,
        covers: impl Fn(&Matcher) -> bool,
    ) -> bool {
        let hit = |caps: &'a [Capability]| caps.iter().filter_map(&select).any(&covers);
        !hit(&self.deny) && hit(&self.allow)
    }

    /// `None` for kinds outside the network policy.
    pub(crate) fn check_net(&self, request: &Request<'_>) -> Option<bool> {
        Some(match *request {
            Request::NetConnect(host, port) => {
                parse_host(host).is_some_and(|h| self.connect_allowed(&h, port, None))
            }
            Request::NetListen(addr, port) => match parse_host(addr) {
                Some(Host::Ip(ip)) => self.allowed_by(
                    |cap| match cap {
                        Capability::NetListen(a, ports) if ports.contains(port) => {
                            a.resolved.as_ref()
                        }
                        _ => None,
                    },
                    |m| m.covers_ip(ip),
                ),
                _ => false,
            },
            Request::InspectorBind(addr) => match parse_host(addr) {
                Some(Host::Ip(ip)) => self.allowed_by(
                    |cap| match cap {
                        Capability::InspectorBind(a) => a.resolved.as_ref(),
                        _ => None,
                    },
                    |m| m.covers_ip(ip),
                ),
                _ => false,
            },
            Request::ImportRemote(host) => parse_host(host).is_some_and(|h| {
                self.allowed_by(
                    |cap| match cap {
                        Capability::ImportRemote(p) => p.resolved.as_ref(),
                        _ => None,
                    },
                    |m| m.covers_host(&h),
                )
            }),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use std::net::SocketAddr;
    use std::sync::Arc;

    const ALL_PORTS: PortRange = PortRange {
        start: 0,
        end: 65535,
    };

    fn port(p: u16) -> PortRange {
        PortRange { start: p, end: p }
    }

    fn connect(allow: &[(&str, PortRange)], deny: &[(&str, PortRange)]) -> Arc<CapabilitySet> {
        let mut b = CapabilitySet::builder();
        for (h, r) in allow {
            b = b.allow(Capability::NetConnect(HostPattern::new(*h), *r));
        }
        for (h, r) in deny {
            b = b.deny(Capability::NetConnect(HostPattern::new(*h), *r));
        }
        b.build()
    }

    fn host_ok(set: &CapabilitySet, host: &str, p: u16) -> bool {
        set.check_connect_host(host, p).is_allow()
    }

    fn addr_ok(set: &CapabilitySet, origin: &str, addr: &str) -> bool {
        let addr: SocketAddr = addr.parse().unwrap();
        set.check_connect_addr(origin, addr).is_allow()
    }

    #[test]
    fn exact_host_and_port_case_insensitive() {
        let set = connect(&[("api.example.com", port(443))], &[]);
        assert!(host_ok(&set, "api.example.com", 443));
        assert!(host_ok(&set, "API.EXAMPLE.COM", 443));
        assert!(host_ok(&set, "api.example.com.", 443));
        assert!(!host_ok(&set, "api.example.com", 80));
        assert!(!host_ok(&set, "xapi.example.com", 443));
        assert!(
            set.check(&Request::NetConnect("api.example.com", 443))
                .is_allow()
        );
    }

    #[test]
    fn wildcard_covers_subdomains_not_apex() {
        let set = connect(&[("*.example.com", ALL_PORTS)], &[]);
        assert!(host_ok(&set, "a.example.com", 1));
        assert!(host_ok(&set, "a.b.example.com", 1));
        assert!(!host_ok(&set, "example.com", 1));
        assert!(!host_ok(&set, "evilexample.com", 1));
    }

    #[test]
    fn idna_hosts_compare_as_ascii() {
        let set = connect(&[("bücher.example", ALL_PORTS)], &[]);
        assert!(host_ok(&set, "xn--bcher-kva.example", 1));
        assert!(host_ok(&set, "BÜCHER.example", 1));
    }

    #[test]
    fn invalid_hosts_are_denied() {
        let set = connect(&[("*", ALL_PORTS)], &[]);
        for h in [
            "",
            " ",
            "a b.com",
            "a.com ",
            "a/b",
            "a..com",
            "xn--.com",
            "127.1",
            "0x7f.1",
            "2130706433",
            "fe80::1%eth0",
            ".",
            "a*.com",
        ] {
            assert!(!host_ok(&set, h, 1), "{h:?}");
        }
    }

    #[test]
    fn ip_and_cidr_grants() {
        let set = connect(
            &[
                ("10.0.0.0/8", port(5432)),
                ("2001:db8::/32", ALL_PORTS),
                ("8.8.8.8", ALL_PORTS),
            ],
            &[],
        );
        assert!(host_ok(&set, "10.1.2.3", 5432));
        assert!(!host_ok(&set, "10.1.2.3", 5433));
        assert!(!host_ok(&set, "11.0.0.1", 5432));
        assert!(host_ok(&set, "2001:db8::1", 1));
        assert!(host_ok(&set, "[2001:db8::1]", 1));
        assert!(host_ok(&set, "8.8.8.8", 1));
        assert!(!host_ok(&set, "8.8.4.4", 1));
    }

    #[test]
    fn ipv4_mapped_is_normalized() {
        let set = connect(&[("10.0.0.0/8", ALL_PORTS)], &[("10.9.9.9", ALL_PORTS)]);
        assert!(host_ok(&set, "::ffff:10.1.2.3", 1));
        assert!(!host_ok(&set, "::ffff:10.9.9.9", 1));
        assert!(addr_ok(&set, "10.1.2.3", "[::ffff:10.1.2.3]:80"));
        let mapped = connect(&[("::ffff:10.0.0.0/104", ALL_PORTS)], &[]);
        assert!(host_ok(&mapped, "10.1.2.3", 1));
    }

    #[test]
    fn port_ranges() {
        let set = connect(
            &[(
                "a.com",
                PortRange {
                    start: 8000,
                    end: 8100,
                },
            )],
            &[],
        );
        assert!(host_ok(&set, "a.com", 8000));
        assert!(host_ok(&set, "a.com", 8100));
        assert!(!host_ok(&set, "a.com", 7999));
        assert!(!host_ok(&set, "a.com", 8101));
    }

    #[test]
    fn deny_beats_allow_for_hosts_and_ips() {
        let set = connect(
            &[("*.example.com", ALL_PORTS)],
            &[("bad.example.com", ALL_PORTS)],
        );
        assert!(host_ok(&set, "ok.example.com", 1));
        assert!(!host_ok(&set, "bad.example.com", 1));
        let set = connect(
            &[("*.example.com", ALL_PORTS)],
            &[("203.0.113.0/24", ALL_PORTS)],
        );
        assert!(addr_ok(&set, "a.example.com", "8.8.8.8:1"));
        assert!(!addr_ok(&set, "a.example.com", "203.0.113.7:1"));
    }

    #[test]
    fn invalid_patterns_allow_nothing_and_deny_everything() {
        for bad in [
            "",
            "a b",
            "10.0.0.0/33",
            "1.2.3.4/x",
            "a/8",
            "a*.com",
            "*.",
            "*.1.2.3.4",
            "::ffff:10.0.0.0/64",
        ] {
            let set = connect(&[(bad, ALL_PORTS)], &[]);
            assert!(
                !host_ok(&set, "a.com", 1) && !host_ok(&set, "10.1.2.3", 1),
                "{bad:?}"
            );
            let set = connect(&[("*", ALL_PORTS)], &[(bad, port(1))]);
            assert!(!host_ok(&set, "a.com", 2), "{bad:?}");
        }
        let set = connect(
            &[("*", ALL_PORTS)],
            &[("a.com", PortRange { start: 9, end: 1 })],
        );
        assert!(!host_ok(&set, "a.com", 5));
        let set = connect(&[("a.com", PortRange { start: 9, end: 1 })], &[]);
        assert!(!host_ok(&set, "a.com", 5));
    }

    #[test]
    fn rebinding_to_private_addresses_is_denied_in_phase_two() {
        let set = connect(
            &[("evil.com", ALL_PORTS), ("*.example.com", ALL_PORTS)],
            &[],
        );
        assert!(host_ok(&set, "evil.com", 80));
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "[::1]",
            "[::]",
            "[fc00::1]",
            "[fd12::1]",
            "[fe80::1]",
            "[::ffff:127.0.0.1]",
            "[64:ff9b::7f00:1]",
            "[2002:7f00:1::]",
        ] {
            assert!(!addr_ok(&set, "evil.com", &format!("{ip}:80")), "{ip}");
            assert!(!addr_ok(&set, "a.example.com", &format!("{ip}:80")), "{ip}");
        }
        assert!(addr_ok(&set, "evil.com", "93.184.216.34:80"));
        assert!(addr_ok(&set, "evil.com", "[2606:2800:220:1::1]:80"));
        assert!(!addr_ok(&set, "other.com", "93.184.216.34:80"));
        assert!(!addr_ok(&set, "evil.com", "93.184.216.34:81") || host_ok(&set, "evil.com", 81));
    }

    #[test]
    fn explicit_ip_grant_and_localhost_allow_private_addresses() {
        let set = connect(
            &[
                ("localhost", ALL_PORTS),
                ("10.0.0.0/8", port(5432)),
                ("*", ALL_PORTS),
            ],
            &[],
        );
        assert!(addr_ok(&set, "localhost", "127.0.0.1:80"));
        assert!(addr_ok(&set, "localhost", "[::1]:80"));
        assert!(!addr_ok(&set, "localhost", "10.0.0.1:80"));
        assert!(!addr_ok(&set, "other.com", "127.0.0.1:80"));
        assert!(addr_ok(&set, "10.1.2.3", "10.1.2.3:5432"));
        assert!(!addr_ok(&set, "10.1.2.3", "10.1.2.3:80"));
        assert!(!addr_ok(&set, "10.1.2.3", "10.9.9.9:5432"));
        let no_wildcard = connect(&[("10.0.0.0/8", port(5432))], &[]);
        assert!(!addr_ok(
            &no_wildcard,
            "nonexistent.example",
            "10.1.2.3:5432"
        ));
        let wildcard_only = connect(&[("*", ALL_PORTS)], &[]);
        assert!(!host_ok(&wildcard_only, "10.1.2.3", 1));
        assert!(!host_ok(&wildcard_only, "[::1]", 1));
    }

    #[test]
    fn deny_on_hostname_applies_in_phase_two() {
        let set = connect(
            &[("*.example.com", ALL_PORTS)],
            &[("bad.example.com", ALL_PORTS)],
        );
        assert!(!addr_ok(&set, "bad.example.com", "8.8.8.8:1"));
        assert!(!addr_ok(&set, "bad.example.com.", "8.8.8.8:1"));
    }

    #[test]
    fn invalid_origin_in_phase_two_is_denied() {
        let set = connect(&[("*", ALL_PORTS)], &[]);
        assert!(!addr_ok(&set, "", "8.8.8.8:1"));
        assert!(!addr_ok(&set, "a b", "8.8.8.8:1"));
        assert!(!addr_ok(&set, "8.8.4.4", "8.8.8.8:1"));
        assert!(addr_ok(&set, "8.8.8.8", "8.8.8.8:1"));
    }

    #[test]
    fn listen_uses_addr_patterns() {
        let set = CapabilitySet::builder()
            .allow(Capability::NetListen(
                AddrPattern::new("127.0.0.0/8"),
                port(8080),
            ))
            .allow(Capability::NetListen(AddrPattern::new("::1"), ALL_PORTS))
            .allow(Capability::NetListen(
                AddrPattern::new("0.0.0.0"),
                port(9000),
            ))
            .deny(Capability::NetListen(
                AddrPattern::new("127.0.0.2"),
                ALL_PORTS,
            ))
            .build();
        let ok = |a: &str, p: u16| set.check(&Request::NetListen(a, p)).is_allow();
        assert!(ok("127.0.0.1", 8080));
        assert!(!ok("127.0.0.1", 8081));
        assert!(!ok("127.0.0.2", 8080));
        assert!(ok("::1", 5));
        assert!(ok("0.0.0.0", 9000));
        assert!(!ok("::", 9000));
        assert!(!ok("localhost", 8080));
        assert!(!ok("example.com", 8080));
        assert!(!ok("", 8080));
        assert!(
            !set.check(&Request::NetConnect("127.0.0.1", 8080))
                .is_allow()
        );
    }

    #[test]
    fn invalid_addr_patterns_fail_closed() {
        for bad in ["localhost", "*", "", "10.0.0.0/99", "*.com"] {
            let set = CapabilitySet::builder()
                .allow(Capability::NetListen(AddrPattern::new(bad), ALL_PORTS))
                .allow(Capability::InspectorBind(AddrPattern::new(bad)))
                .build();
            assert!(
                !set.check(&Request::NetListen("127.0.0.1", 1)).is_allow(),
                "{bad:?}"
            );
            assert!(
                !set.check(&Request::InspectorBind("127.0.0.1")).is_allow(),
                "{bad:?}"
            );
            let set = CapabilitySet::builder()
                .allow(Capability::InspectorBind(AddrPattern::new("127.0.0.1")))
                .deny(Capability::InspectorBind(AddrPattern::new(bad)))
                .build();
            assert!(
                !set.check(&Request::InspectorBind("127.0.0.1")).is_allow(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn import_remote_uses_host_patterns() {
        let set = CapabilitySet::builder()
            .allow(Capability::ImportRemote(HostPattern::new("esm.sh")))
            .allow(Capability::ImportRemote(HostPattern::new("*.cdn.example")))
            .build();
        let ok = |h: &str| set.check(&Request::ImportRemote(h)).is_allow();
        assert!(ok("esm.sh"));
        assert!(ok("ESM.SH"));
        assert!(ok("a.cdn.example"));
        assert!(!ok("cdn.example"));
        assert!(!ok("esm.sh.evil.com"));
        assert!(!ok("esm.shx"));
        assert!(!ok(""));
    }

    #[test]
    fn inspector_bind_uses_addr_patterns() {
        let set = CapabilitySet::builder()
            .allow(Capability::InspectorBind(AddrPattern::new("127.0.0.1")))
            .build();
        assert!(set.check(&Request::InspectorBind("127.0.0.1")).is_allow());
        assert!(
            set.check(&Request::InspectorBind("::ffff:127.0.0.1"))
                .is_allow()
        );
        assert!(!set.check(&Request::InspectorBind("0.0.0.0")).is_allow());
        assert!(!set.check(&Request::InspectorBind("127.0.0.1x")).is_allow());
    }

    #[test]
    fn net_deny_is_redacted() {
        let set = connect(&[], &[]);
        let Decision::Deny { description, .. } = set.check_connect_host("secret.internal", 1)
        else {
            panic!("expected deny");
        };
        assert!(description.contains("net-connect") && !description.contains("secret"));
    }
}
