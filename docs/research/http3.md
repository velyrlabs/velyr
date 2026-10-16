# HTTP/3 (QUIC) for the server and fetch (VELYR-122)

Prototype behind the `http3` feature of `velyr-io-http` (`crates/io-http/src/h3/`), off by default and not
wired into the CLI or web-fetch. Stack: `quinn` 0.11.12 (already in `Cargo.lock` through another dependency),
`h3` 0.0.8, `h3-quinn` 0.0.10, rustls 0.23.46 with aws-lc-rs (the same provider as the TCP path), TLS 1.3 only.

## What was built

- `serve_h3(endpoint, limits, handler, shutdown)`: accept loop that reuses the io-http `Handler`, so
  handlers do not know which protocol served them (`version` is `HTTP/3.0`, `host` is rebuilt from
  `:authority`). Honors `max_header_bytes` (QPACK field-section size), `max_body_bytes` (413 on a larger
  `content-length`, a `QuotaExceeded` chunk when a body outgrows the limit while streaming).
- `H3Connection::connect` / `request`: one QUIC connection, requests multiplex as streams, same
  `HttpRequestParts` / `HttpResponseParts` types as `HttpClient`.
- `server_endpoint` / `client_endpoint`: QUIC TLS setup, ALPN `h3`, 0-RTT disabled.
- `alt_svc_value(port, max_age)`: `h3=":443"; ma=86400`.
- Tests (in-crate): GET and POST round trip on loopback UDP with an rcgen certificate, connection reuse,
  413, handshake failure without the trusted cert, Alt-Svc format. The benchmark is an `#[ignore]` test.

## Numbers

Apple M-series, macOS, release build, loopback.

Footprint (`cargo build -p velyr-io-http --release`, rlib of the crate alone, dependencies excluded):

| | size |
|---|---|
| without `http3` | 1,179,440 B |
| with `http3` | 1,501,104 B (+27%) |

Linked binary (probe binary: `serve_listener` plus, with the feature, `serve_h3`; thin LTO, stripped). The first
pair is misleading because the baseline does not link aws-lc-rs; the second forces both to link it, which is
the real situation since the TCP/TLS path already carries it:

| probe | without | with `http3` | delta |
|---|---|---|---|
| baseline does not link aws-lc-rs | 2,471,408 B | 5,621,856 B | +3.15 MB |
| both link aws-lc-rs | 4,557,840 B | 5,757,728 B | +1.20 MB (+26%) |

New crates in the normal dependency tree: 23 entries, of which the ones that matter are `h3`, `h3-quinn`,
`quinn-proto`, `quinn-udp`, `futures` (+ `futures-util`, `futures-executor`, `futures-macro`, `futures-io`),
`lru-slab`, `tinyvec`, `rand` 0.10 / `rand_core` / `rand_pcg`, `getrandom` 0.4, `chacha20`, `rustc-hash`,
`fastrand`. (Counted with `cargo tree`; the Cargo.lock diff also shows unrelated entries from other
in-flight work.) `quinn` itself was already in the lockfile.

Latency (1 KiB body, one persistent connection, N=1000 sequential requests after 50 warm-up, TLS on all three;
`cargo test -p velyr-io-http --features http3 --release --lib -- --ignored --nocapture h3_vs_tcp`), 4 runs:

| | median | p99 |
|---|---|---|
| h1 (TLS) | 46-95 us | 87-6297 us (one outlier run; typically 90-300 us) |
| h2 (TLS) | 38-46 us | 105-225 us |
| h3 | 61-70 us | 120-339 us |
| h3 cold connect + first request | 0.85-1.46 ms | - |

Loopback caveat: there is no RTT, no loss and no congestion, so none of the things QUIC improves (handshake
round trips, head-of-line blocking on loss, connection migration) show up. These numbers only measure stack
overhead, and there h3 is about 1.5x slower than h2 per request, as expected for a userspace UDP stack
(no kernel TCP offload, per-packet encryption). They are not evidence for or against h3 on a real network. A
real-network evaluation needs a lossy or high-RTT link (for example `tc netem` or a remote host).

## Alt-Svc on the TCP server

h3 discovery is through the `Alt-Svc` response header on h1/h2 (RFC 7838): a client that reaches the origin
over TCP learns `h3=":443"` and may use QUIC on the next request. Design:

- The TCP server adds `Alt-Svc: <alt_svc_value(udp_port, ma)>` to responses when an h3 endpoint is running.
  The natural place is the handler wrapper in the CLI serve command (a closure around the user's handler),
  not in `server.rs`, so io-http stays protocol-neutral. Not done here (out of the allowed paths).
- Same port number for TCP and UDP is the norm (`:443`); the helper takes the UDP port explicitly.
- `ma` (max-age) trades discovery latency for blast radius when h3 is turned off. Start at 1 hour, not 24.
- Without HTTPS there is no h3; the header must only be sent on TLS responses.
- Do not send `Alt-Svc: clear` on shutdown unless the server is really going away for h3.

## Fetch with fallback

h3 in `fetch` must never make a request fail that would have worked over h2. Design:

1. Default off; opt in per client (`ClientOptions.http3: bool`) once the runtime flag exists.
2. An in-memory Alt-Svc cache keyed by origin, filled from `Alt-Svc` headers of h1/h2 responses (`h3` token,
   TTL from `ma`). No cache entry means plain h2/h1 as today: no happy-eyeballs race on first contact, which
   keeps the permission check and DNS behavior identical to today.
3. With an entry: attempt h3 with a short handshake timeout (about 250 ms or 2x measured TCP connect). The
   connect authorization (io-net two-phase check) must run for the UDP destination exactly as for TCP, before
   any datagram is sent; permissions are per host and port, so the h3 port is a distinct `net` capability
   check.
4. On handshake failure, timeout, or `H3_*` connection errors before any response byte: mark the origin as
   broken for a backoff period (for example 5 minutes, doubling) and retry on TCP. Idempotent methods only for
   a retry after the request was sent; non-idempotent requests that failed before send can retry safely.
5. UDP blocked by firewalls is the dominant failure mode in practice; step 4 is what makes it invisible.
6. Request bodies are buffered in the prototype; streaming uploads need the h3 `send_data` loop driven by the
   body channel before this can back `fetch` for large uploads.

## Security and operational cost

- 0-RTT is disabled: early data is replayable and the runtime cannot know which handlers are idempotent.
- QUIC amplification and spoofing: quinn applies address validation (retry) only if enabled
  (`ServerConfig::retry` / `Incoming::retry`); production use should answer `Incoming` with a retry when under
  load. Not enabled in the prototype.
- Quotas: the body-bytes quota context (`Limits.quotas`) is not wired into the h3 pump yet; only the
  `max_body_bytes` limit is enforced. Header-size limit is enforced by h3 (QPACK field section size).
- Idle and handshake timeouts rely on quinn defaults (30 s idle); `Limits.idle_timeout` is not mapped yet.
- No graceful drain: shutdown closes the endpoint; the TCP server has `serve_listener_graceful`, h3 needs
  `Connection::shutdown` (GOAWAY) per connection.
- UDP does not go through io-net `Duplex`/listener abstractions, so metrics (`HttpMetrics`) and the permission
  layer need an h3 path of their own.
- Multicore (ADR-014): quinn endpoints are per socket; scaling across cores needs `SO_REUSEPORT` UDP sockets
  with one endpoint each, and QUIC connection IDs make the kernel's 4-tuple hashing safe only while the
  client address does not change (migration breaks affinity). This is the same problem class as TCP
  `SO_REUSEPORT` but with a worse failure mode.
- Maturity: `h3` is 0.0.x (expect API breaks between releases; the prototype uses caret ranges, a real
  adoption should pin exact versions like the rest of this crate). `h3-quinn` 0.0.10 needs quinn >= 0.11.7.
- Supply chain: about 20 new crates in the tree, mostly small (`lru-slab`, `tinyvec`, `quinn-udp`).
- aws-lc-rs is reused (quinn feature `rustls-aws-lc-rs`, default features off), so no second crypto library
  enters this crate's dependency tree (`ring` does not appear in `cargo tree -p velyr-io-http --features http3`).

## Recommendation: defer, keep the prototype

Do not adopt now.

- The measurable gain is nil on loopback and unproven elsewhere; the runtime's target workloads (local dev
  server, scripts, `fetch` to APIs) are mostly low-RTT and short-lived, where QUIC's handshake saving is
  small and its per-request CPU cost is higher.
- Cost: +1.2 MB linked, a 0.0.x dependency family, a second transport with its own timeouts, quotas,
  metrics, permission and shutdown paths, and a multicore story that needs work.
- Fetch support is useful only with the Alt-Svc cache and the fallback above, which is the larger part of
  the work (roughly: cache + backoff in the client, UDP connect authorization, metrics).

Revisit when: (a) a user-facing requirement for h3 appears (serving to browsers on lossy mobile networks is
the main case), (b) `h3` reaches 0.1 or a stable API, or (c) a real-network measurement shows a benefit for a
target workload. If adopted, the minimum ADR is below.

### ADR draft (not filed)

- Decision: ship h3 as an opt-in server listener (`serve --http3`) first; client h3 later behind the Alt-Svc
  cache and fallback.
- Constraints: feature-gated crate dependency; UDP permission check mirrors TCP; 0-RTT off; retry under load;
  `Alt-Svc` set by the serve command, not by io-http.
- Consequences: +1.2 MB, new dependency family to pin and audit, h3 metrics and graceful shutdown to add.
- Out of scope: WebTransport, datagrams, push, h3 over proxies.
