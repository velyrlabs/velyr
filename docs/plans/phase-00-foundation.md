# Phase 00 — Foundation

## Objective
Make future architecture hard to accidentally collapse into a monolith.

## Tasks
- create Cargo workspace;
- define crate naming;
- establish `engine-api` and `runtime-core`;
- prohibit unsafe in high-level crates;
- CI Linux/macOS/Windows;
- clippy/fmt/test;
- cargo-audit/cargo-deny;
- benchmark result schema;
- Criterion or selected microbenchmark harness;
- integration benchmark harness;
- fuzz workspace;
- error code conventions;
- tracing conventions;
- SECURITY.md and disclosure channel;
- feature/profile build tests.

## Acceptance
- minimal CLI prints version and exits;
- workspace builds with minimal and full feature sets;
- CI can detect dependency advisories;
- no Node/Web features are required by `runtime-core`.
