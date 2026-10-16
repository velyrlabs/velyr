# ROADMAP — Velyr Core

Roadmap sequencing optimizes for early architectural proof while preventing compatibility work from locking the kernel into Node internals.

---

## Track model

Three tracks progress independently:

```text
TRACK A — Runtime Kernel
TRACK B — Compatibility & Tooling
TRACK C — Native ECMAScript Engine
```

A release may advance one track without blocking the others.

---

# Milestone 0 — Foundation

## Goal
Create the repository architecture, benchmark harness, security invariants and engine boundary before feature growth.

## Deliverables
- Cargo workspace and crate boundaries
- lint/format/MSRV policy
- CI matrix
- architecture docs and ADRs
- benchmark harness
- cargo-audit/cargo-deny policy
- fuzz workspace
- engine API skeleton
- runtime error types
- structured tracing foundation

## Exit criteria
- clean workspace build on Linux/macOS/Windows;
- minimal binary builds;
- CI rejects dependency advisories according to policy;
- benchmark result format committed.

---

# Milestone 1 — Engine bridge and synchronous execution

## Goal
Execute JavaScript through an engine adapter and prove JS ↔ Rust host calls.

## Deliverables
- engine lifecycle
- realm/context lifecycle
- JS values/handles
- `console`
- host function registration
- exceptions and stack mapping
- `engine-v8` adapter (ADR-009) and `engine-native` bootstrap as second `engine-api` implementation

## Acceptance
```js
console.log(nativeAdd(20, 22)); // 42
```
runs with deterministic error behavior.

---

# Milestone 2 — Async kernel

## Goal
Complete JS → Rust Future → Promise → microtask → JS.

## Deliverables
- completion queue
- cancellation
- timer wheel/timers
- promise handles
- microtask integration
- Tokio host runtime
- deterministic event-loop tests

## Exit criteria
Top-level async host operations can complete without cross-thread engine mutation.

---

# Milestone 3 — Security, resources and ESM

## Goal
Make the runtime usable safely before broad I/O support.

## Deliverables
- capability model
- permission CLI
- resource table
- quotas
- ESM loader
- source cache
- canonical module identity
- worker permission inheritance foundation

## Exit criteria
An unprivileged script cannot read files, access environment, open network connections or listeners.

---

# Milestone 4 — Web server baseline

## Goal
Deliver a useful secure server runtime.

## Deliverables
- URL / URLSearchParams
- TextEncoder/TextDecoder
- AbortController/AbortSignal
- fetch
- Request/Response/Headers
- Web Streams subset required by fetch
- TCP
- HTTP/1
- HTTP/2
- TLS
- basic WebSocket
- applicable WPT runner

## Exit criteria
A Web-standard HTTP service can run without Node compatibility.

---

# Milestone 5 — Multicore workers

## Goal
Exploit multi-core servers without requiring explicit Node cluster orchestration.

## Deliverables
- worker isolate lifecycle
- supervisor
- connection/request distribution strategy
- structured clone
- worker messaging
- quotas
- shared immutable cache experiments
- worker metrics

## Exit criteria
A server benchmark scales across multiple cores while preserving isolate semantics and authority boundaries.

---

# Milestone 6 — npm and package ecosystem

## Goal
Consume mainstream packages while maintaining secure installation defaults.

## Deliverables
- package.json parser
- semver
- npm registry client
- content-addressed cache
- lockfile
- `exports`
- `imports`
- conditional exports
- peer/optional dependency rules
- lifecycle script permission policy

## Exit criteria
A curated package corpus resolves and installs deterministically.

---

# Milestone 7 — Node compatibility

## Goal
Run common backend ecosystem packages.

Priority follows the tiers in [compatibility/node-npm.md](compatibility/node-npm.md) (Tier 1 → Tier 4).

CommonJS is delivered alongside compatibility.

## Exit criteria
Compatibility dashboard exists and high-value package suites run automatically.

---

# Milestone 8 — Frontend developer ecosystem

## Goal
Support mainstream web development workflows.

## Deliverables
- TS transform path
- JSX transform path
- source maps
- Vite compatibility
- React acceptance repo
- Vue acceptance repo
- Vue SFC via official compiler path
- HMR compatibility investigation
- SSR Web Streams tests

## Exit criteria
Representative React+Vite and Vue+Vite projects build and run through Velyr.

---

# Milestone 9 — Native addon boundary

## Goal
Add controlled N-API interoperability.

## Deliverables
- N-API subset
- explicit `--allow-native`/FFI permission
- compatibility matrix
- crash containment research
- optional out-of-process addon host prototype

## Exit criteria
Selected N-API packages pass without granting native authority by default.

---

# Milestone 10 — Native engine maturity

Runs in parallel from Milestone 0 onward.

## Sequence
- lexer
- parser
- AST
- values
- lexical environments
- objects/prototypes
- bytecode
- VM
- exceptions
- functions/closures
- promises
- modules
- generators
- async functions
- typed arrays
- RegExp
- BigInt
- Intl
- Temporal where applicable
- advanced GC
- baseline JIT research
- optimizing JIT research

## Gate
Test262 drives progress. Runtime releases never claim semantic support based only on implemented syntax.

---

# Milestone 11 — Advanced ecosystem

After core stability:
- Next.js investigation
- Nuxt investigation
- test runner compatibility
- package manager acceleration
- bundler
- dev server
- native TS/JSX fast path
- HTTP/3
- edge snapshots
- single executable packaging
- embedded runtime SDK

---

# v1 release gate

v1 requires production readiness of milestones 0–8 and a documented native engine status. Milestone 9 may be partial. Optimizing JIT is not required for v1.
