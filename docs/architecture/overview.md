# Architecture overview

## Layers

```text
Application JS/TS
        │
        ▼
Standards / Compatibility APIs
        │
        ▼
Runtime Kernel
        │
        ▼
Capability + Resource Boundary
        │
        ▼
Host Services
        │
        ▼
Tokio / OS
```

The JS engine sits beside the kernel through an adapter. It is not the owner of host authority.

## Dependency direction

```text
compat-* ─────┐
web-* ────────┤
tooling-* ────┤
              ▼
         runtime-core
              │
              ▼
         engine-api
```

`runtime-core` must not depend on compatibility packages.

## Minimum runtime

A minimum runtime should compile without npm, CommonJS, Node APIs, TLS, HTTP/2, WebSocket, N-API or developer tooling.

## Why this architecture

It preserves:
- security boundaries;
- small deployments;
- replaceable engine work;
- testable compatibility;
- performance isolation;
- ability to ship edge and full server distributions from one ecosystem.
