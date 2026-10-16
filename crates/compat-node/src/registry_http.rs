//! Factories and export lists for `node:http`, `node:https` and `node:http2`. The JS lives in
//! `js/http/`, written for this runtime; the `internal/http/*` modules are shared by the three.

pub(crate) const HTTP_FACTORIES: &[(&str, &str)] = &[
    ("internal/http/common", include_str!("../js/http/common.js")),
    (
        "internal/http/incoming",
        include_str!("../js/http/incoming.js"),
    ),
    (
        "internal/http/response",
        include_str!("../js/http/response.js"),
    ),
    ("internal/http/server", include_str!("../js/http/server.js")),
    (
        "internal/http/client_socket",
        include_str!("../js/http/client_socket.js"),
    ),
    ("internal/http/client", include_str!("../js/http/client.js")),
    ("http", include_str!("../js/http/index.js")),
    ("https", include_str!("../js/http/https.js")),
    ("http2", include_str!("../js/http/http2.js")),
];

pub(crate) const HTTP_EXPORTS: &[&str] = &[
    "Agent",
    "ClientRequest",
    "IncomingMessage",
    "METHODS",
    "OutgoingMessage",
    "STATUS_CODES",
    "Server",
    "ServerResponse",
    "createServer",
    "get",
    "globalAgent",
    "maxHeaderSize",
    "request",
    "setMaxIdleHTTPParsers",
    "validateHeaderName",
    "validateHeaderValue",
];

pub(crate) const HTTPS_EXPORTS: &[&str] = &[
    "Agent",
    "Server",
    "createServer",
    "get",
    "globalAgent",
    "request",
];

pub(crate) const HTTP2_EXPORTS: &[&str] = &[
    "Http2ServerRequest",
    "Http2ServerResponse",
    "connect",
    "constants",
    "createSecureServer",
    "createServer",
    "getDefaultSettings",
    "getPackedSettings",
    "getUnpackedSettings",
    "sensitiveHeaders",
];
