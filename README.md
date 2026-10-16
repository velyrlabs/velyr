# Velyr Core

> A security-first, modular, high-performance JavaScript/TypeScript runtime written in Rust.

This repository documentation defines the product, architecture, compatibility strategy, security model, performance goals, specifications, and delivery plans for **Velyr Core**.

## North Star

Velyr exists to maximize **useful workload per CPU core and per GB of memory** while preserving a modern JavaScript/TypeScript developer experience.

The runtime is designed around five permanent principles:

1. **Secure by default**
2. **Web standards first**
3. **Node compatibility is optional**
4. **Everything measurable**
5. **Everything modular**

## Documentation map

- [`docs/PRD.md`](docs/PRD.md) — Product Requirements Document
- [`docs/TDD.md`](docs/TDD.md) — Technical Design Document
- [`docs/SPEC.md`](docs/SPEC.md) — Runtime specification
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — Product and engineering roadmap
- [`docs/PLANS.md`](docs/PLANS.md) — Execution plan and delivery mechanics
- [`docs/architecture/`](docs/architecture/) — Architecture details
- [`docs/security/`](docs/security/) — Threat model, permissions and supply-chain controls
- [`docs/compatibility/`](docs/compatibility/) — ECMAScript, Web, Node, npm and frontend compatibility
- [`docs/performance/`](docs/performance/) — Benchmarks, memory and regression policy
- [`docs/specs/`](docs/specs/) — Interface-level specifications
- [`docs/plans/`](docs/plans/) — Phase-by-phase implementation plans
- [`docs/adr/`](docs/adr/) — Architecture Decision Records
- [`docs/REFERENCES.md`](docs/REFERENCES.md) — Primary external references

## Proposed repository topology

```text
velyr-core/
├── crates/
│   ├── engine-api/
│   ├── engine-v8/
│   ├── engine-native/
│   ├── engine-parser/
│   ├── engine-bytecode/
│   ├── engine-vm/
│   ├── engine-gc/
│   ├── runtime-core/
│   ├── runtime-ops/
│   ├── runtime-event-loop/
│   ├── runtime-permissions/
│   ├── runtime-resources/
│   ├── runtime-modules/
│   ├── runtime-workers/
│   ├── web-core/
│   ├── web-fetch/
│   ├── web-streams/
│   ├── web-crypto/
│   ├── io-fs/
│   ├── io-net/
│   ├── io-http/
│   ├── io-tls/
│   ├── compat-node/
│   ├── compat-commonjs/
│   ├── compat-npm/
│   ├── compat-napi/
│   ├── plugins-wasm/
│   ├── tooling-typescript/
│   ├── tooling-jsx/
│   └── cli/
├── benches/
├── fuzz/
├── tests/
├── compat/
├── docs/
└── Cargo.toml
```

## Definition of success

Velyr is successful when it can:

- execute standards-compliant ECMAScript with a measurable conformance score;
- expose a secure Web-standard server runtime;
- run without filesystem, network, environment or process permissions by default;
- scale vertically across cores without requiring users to manually orchestrate Node-style clusters;
- provide optional Node/npm compatibility without making it part of the minimum runtime;
- run React/Vue/Vite workloads through standards and compatibility rather than framework-specific hacks;
- enforce resource limits suitable for serverless, CI, plugin and multi-tenant workloads;
- demonstrate performance and memory gains through reproducible benchmarks rather than claims.
