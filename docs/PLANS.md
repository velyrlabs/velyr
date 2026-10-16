# PLANS — Velyr Core Execution System

This document converts the roadmap into an operating model.

---

## 1. Work breakdown

Every implementation item belongs to one epic:

- `ENG` — ECMAScript engine
- `RUN` — runtime kernel
- `SEC` — security
- `MOD` — module/package system
- `WEB` — Web APIs
- `NET` — networking
- `WRK` — workers/multicore
- `NOD` — Node compatibility
- `NPM` — package ecosystem
- `TOOL` — TS/JSX/dev tooling
- `PERF` — performance
- `OBS` — observability
- `QA` — conformance/testing
- `DX` — CLI/developer experience

Issue example:

```text
RUN-021: Host completion queue cannot mutate engine off-isolate
```

---

## 2. Definition of Ready

An issue is ready only when it has:
- motivation;
- scope/non-scope;
- owning crate;
- security impact;
- compatibility impact;
- benchmark impact;
- acceptance tests;
- rollback/feature-gate strategy for high-risk changes.

---

## 3. Definition of Done

Code is done only if:
- unit/integration tests pass;
- conformance tests updated where relevant;
- fuzz target exists for parser/protocol/security boundary where relevant;
- metrics/benchmark added for performance-sensitive code;
- no unreviewed unsafe code;
- docs/spec updated;
- feature remains optional where architecture requires;
- security threat model updated for new authority.

---

## 4. Merge gates

### Mandatory
```text
cargo fmt --check
cargo clippy
cargo test
cargo audit
cargo deny check
minimal feature build
full feature build
platform matrix
```

### Conditional
```text
Test262 subset/full
WPT subset
fuzz smoke
Miri
ASan/TSan
performance suite
compatibility corpus
```

---

## 5. Branch strategy

Recommended:
- trunk-based development;
- short-lived feature branches;
- protected `main`;
- release branches only when stabilizing;
- conventional commits or scoped commits by epic.

---

## 6. Release channels

### nightly
Every green main build.

### preview
Feature testing. Compatibility may change.

### beta
API freeze candidate. Security model cannot silently weaken.

### stable
SemVer guarantees defined per crate and CLI.

---

## 7. Phase planning

Detailed plans:
- [Phase 00 — Foundation](plans/phase-00-foundation.md)
- [Phase 01 — Engine bridge](plans/phase-01-engine-bridge.md)
- [Phase 02 — Async kernel](plans/phase-02-async-kernel.md)
- [Phase 03 — Security and modules](plans/phase-03-security-modules.md)
- [Phase 04 — Web runtime](plans/phase-04-web-runtime.md)
- [Phase 05 — Multicore](plans/phase-05-multicore.md)
- [Phase 06 — npm](plans/phase-06-npm.md)
- [Phase 07 — Node compatibility](plans/phase-07-node-compat.md)
- [Phase 08 — Frontend](plans/phase-08-frontend.md)
- [Phase 09 — N-API](plans/phase-09-napi.md)
- [Phase 10 — Native engine](plans/phase-10-native-engine.md)

---

## 8. Benchmark governance

Performance results are stored as machine-readable data:

```json
{
  "benchmark": "http-json",
  "commit": "<sha>",
  "profile": "release",
  "platform": "...",
  "cpu": "...",
  "memory": "...",
  "metrics": {
    "requests_per_sec": 0,
    "p99_ms": 0,
    "rss_mb": 0,
    "cpu_seconds": 0
  }
}
```

A dashboard can be added later without changing source data format.

---

## 9. Compatibility governance

Every compatibility target has:
- owner;
- test source;
- supported version range;
- status: unsupported/experimental/partial/supported;
- known deviations;
- regression history.

No compatibility status is assigned manually without executable evidence.

---

## 10. Risk review cadence

Before each beta/stable release review:
- new unsafe code;
- new permissions;
- new native dependencies;
- dependency advisories;
- lifecycle scripts;
- parser/resolver changes;
- quota bypasses;
- cross-isolate shared memory;
- inspector exposure;
- secrets/logging behavior.
