//! Factories and export lists for `node:net`, `node:tls`, `node:dns` and `node:dns/promises`.
//! The JS lives in `js/` (and `js/dns/`), written for this runtime.

pub(crate) const NET_FACTORIES: &[(&str, &str)] = &[
    ("internal/net/ip", include_str!("../js/dns/ip.js")),
    ("net", include_str!("../js/net.js")),
    ("tls", include_str!("../js/tls.js")),
    ("dns", include_str!("../js/dns/index.js")),
    ("dns/promises", include_str!("../js/dns/promises.js")),
];

pub(crate) const NET_EXPORTS: &[&str] = &[
    "BlockList",
    "Server",
    "Socket",
    "Stream",
    "connect",
    "createConnection",
    "createServer",
    "isIP",
    "isIPv4",
    "isIPv6",
];

pub(crate) const TLS_EXPORTS: &[&str] = &[
    "DEFAULT_MAX_VERSION",
    "DEFAULT_MIN_VERSION",
    "SecureContext",
    "Server",
    "TLSSocket",
    "checkServerIdentity",
    "connect",
    "createSecureContext",
    "createServer",
    "getCiphers",
];

pub(crate) const DNS_EXPORTS: &[&str] = &[
    "ADDRCONFIG",
    "ALL",
    "Resolver",
    "V4MAPPED",
    "getDefaultResultOrder",
    "getServers",
    "lookup",
    "promises",
    "resolve",
    "resolve4",
    "resolve6",
    "resolveAny",
    "resolveCaa",
    "resolveCname",
    "resolveMx",
    "resolveNaptr",
    "resolveNs",
    "resolvePtr",
    "resolveSoa",
    "resolveSrv",
    "resolveTxt",
    "reverse",
    "setDefaultResultOrder",
    "setServers",
];

pub(crate) const DNS_PROMISES_EXPORTS: &[&str] = &[
    "Resolver",
    "getDefaultResultOrder",
    "getServers",
    "lookup",
    "resolve",
    "resolve4",
    "resolve6",
    "resolveAny",
    "resolveCaa",
    "resolveCname",
    "resolveMx",
    "resolveNaptr",
    "resolveNs",
    "resolvePtr",
    "resolveSoa",
    "resolveSrv",
    "resolveTxt",
    "reverse",
    "setDefaultResultOrder",
    "setServers",
];
