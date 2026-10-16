# TDD — Velyr Core Technical Design

**Status:** Draft v0.1  
**Scope:** Runtime architecture and implementation boundaries

---

## 1. Architecture summary

Velyr separates language semantics, runtime semantics and ecosystem compatibility.

```text
JS / TS source
     │
     ▼
┌───────────────┐
│ Engine API    │
└──────┬────────┘
       │
┌──────▼────────┐
│ JS Engine     │
│ native/ref    │
└──────┬────────┘
       │
┌──────▼────────────────────────────────┐
│ Runtime Kernel                        │
│ event loop | ops | modules | workers │
│ permissions | resources | quotas     │
└──────┬────────────────────────────────┘
       │
┌──────▼────────────────────────────────┐
│ Host Services                         │
│ fs | net | dns | http | tls | process│
└──────┬────────────────────────────────┘
       │
     Tokio / OS
```

Optional layers:

```text
Web APIs
Node compatibility
CommonJS
npm resolver/package manager
Node-API
Wasm plugins
TS/JSX tooling
Frontend tooling
```

---

## 2. Architectural invariants

1. The engine MUST NOT call OS services directly.
2. Every privileged host operation MUST pass through permission/capability evaluation.
3. Runtime resource ownership MUST be represented through handles/resource IDs.
4. Worker authority MUST be a subset of parent authority.
5. ESM is the canonical module graph.
6. CommonJS is an adapter.
7. Node compatibility must not be required for Web-only applications.
8. Runtime behavior and engine behavior must be independently testable.
9. Unsafe Rust must not leak across crate boundaries as a normal programming model.
10. Performance optimizations cannot weaken language semantics or security checks.

---

## 3. Workspace boundaries

### Engine plane
- `engine-api`
- `engine-parser`
- `engine-bytecode`
- `engine-vm`
- `engine-gc`
- `engine-native`
- `engine-v8` (initial production and reference engine, ADR-009)

### Runtime plane
- `runtime-core`
- `runtime-ops`
- `runtime-event-loop`
- `runtime-permissions`
- `runtime-resources`
- `runtime-modules`
- `runtime-workers`

### Host plane
- `io-fs`
- `io-net`
- `io-dns`
- `io-http`
- `io-tls`
- `io-process`

### Standards plane
- `web-core`
- `web-fetch`
- `web-streams`
- `web-crypto`
- `web-websocket`

### Compatibility plane
- `compat-node`
- `compat-commonjs`
- `compat-npm`
- `compat-napi`

### Tooling plane
- `tooling-typescript`
- `tooling-jsx`
- `tooling-bundler`
- `tooling-dev-server`

### Extension plane
- `plugins-wasm`

---

## 4. Engine boundary

The runtime interacts with a small engine contract.

Conceptual surface:

```rust
pub trait Engine {
    type Value;
    type Error;
    type ModuleId;

    fn eval(&mut self, source: Source) -> Result<Self::Value, Self::Error>;
    fn compile_module(&mut self, source: Source) -> Result<Self::ModuleId, Self::Error>;
    fn instantiate_module(&mut self, id: Self::ModuleId) -> Result<(), Self::Error>;
    fn evaluate_module(&mut self, id: Self::ModuleId) -> Result<PromiseHandle, Self::Error>;

    fn register_host_function(&mut self, def: HostFunctionDef) -> Result<(), Self::Error>;
    fn resolve_promise(&mut self, promise: PromiseHandle, value: Self::Value);
    fn reject_promise(&mut self, promise: PromiseHandle, error: Self::Value);

    fn drain_microtasks(&mut self) -> MicrotaskProgress;
    fn request_gc(&mut self, reason: GcReason);
    fn heap_stats(&self) -> HeapStats;
}
```

This is conceptual. Concrete API design should avoid forcing the native engine to mimic V8 handles.

---

## 5. Event loop

The JS event loop is not Tokio.

Tokio schedules native async tasks. Velyr schedules JavaScript-visible completion.

```text
┌──────── JS Realm ────────┐
│ synchronous execution    │
│ microtask queue          │
└────────────┬─────────────┘
             │
      Velyr event loop
             │
   ┌─────────┼───────────┐
   │         │           │
 timers   host-op CQ   messages
   │         │           │
   └─────────┼───────────┘
             │
           Tokio
```

### Loop invariant
Before sleeping for I/O, Velyr drains all required ECMAScript microtasks according to language semantics.

### Completion queue
Native futures MUST NOT directly mutate engine state from arbitrary worker threads. Completions are returned to the owning isolate via a completion channel/queue.

---

## 6. Host operations

Host operations have four mandatory stages:

```text
decode request
   ↓
authorize capability
   ↓
reserve quota/resource budget
   ↓
execute host service
   ↓
encode completion
```

Every host op receives:
- isolate identity;
- request identity;
- capability context;
- cancellation token;
- quota context;
- tracing span.

---

## 7. Resource table

JS receives opaque IDs/objects whose native state is managed by the runtime.

```text
ResourceTable
├── 12 -> File
├── 13 -> TcpStream
├── 14 -> Listener
├── 15 -> ChildProcess
└── 16 -> WebSocket
```

A resource:
- has an owner isolate;
- has a capability provenance;
- has close semantics;
- is counted against quotas;
- is visible to leak diagnostics;
- can be cancelled when an isolate terminates.

---

## 8. Permissions and capabilities

Permission parsing produces immutable capability grants.

Example:

```text
ReadPath("/app/data/**")
WritePath("/app/cache/**")
Connect("api.example.com", 443)
Listen("0.0.0.0", 3000)
ReadEnv("DATABASE_URL")
```

Host services consume capabilities, not CLI flags.

This makes the kernel usable from:
- CLI;
- embedder;
- serverless platform;
- worker supervisor;
- test harness.

---

## 9. Multicore model

Velyr does not execute one mutable JS heap concurrently across cores.

```text
Supervisor
├── Isolate 1 -> core/runtime task
├── Isolate 2 -> core/runtime task
├── Isolate 3 -> core/runtime task
└── Isolate N
```

### Shared candidates
Only immutable or explicitly synchronized structures:
- source cache;
- parsed metadata;
- compiled bytecode where safe;
- interned immutable constants;
- package metadata;
- TLS config;
- HTTP listener ownership/distribution.

### Per-isolate
- JS heap;
- call stack;
- realm/global state;
- microtask queue;
- module instance state;
- local handles.

---

## 10. Networking

Initial transport:
- Tokio TCP/UDP;
- Hyper HTTP/1 and HTTP/2;
- rustls TLS.

HTTP/3 is a later optional module and MUST NOT be linked into minimal profiles.

The JavaScript-facing API should prefer:
- `fetch`
- `Request`
- `Response`
- Web Streams

rather than exposing Hyper types.

---

## 11. Package/module design

### Canonical forms
- file URL
- relative/absolute file specifiers
- package specifiers
- `node:` compatibility specifiers
- future `npm:` / `jsr:` / `https:` strategies

Module loading uses pluggable resolvers but produces one normalized module graph.

Resolution phases:
1. parse specifier;
2. select scheme/strategy;
3. resolve package scope;
4. enforce exports/imports;
5. permission check;
6. load source;
7. transform if configured;
8. compile;
9. cache;
10. instantiate/evaluate.

---

## 12. Native engine architecture

Initial design:

```text
Source
  ↓
Lexer
  ↓
Parser
  ↓
AST
  ↓
Bytecode compiler
  ↓
Bytecode VM
  ↓
GC-managed values
```

Later:

```text
profiling → baseline JIT → optimizing tiers
```

Correctness and conformance precede JIT.

---

## 13. JavaScript string representation

Rust `String` MUST NOT be treated as the normative JavaScript string representation.

The engine requires a `JsString` abstraction that preserves ECMAScript string semantics including lone surrogates.

Possible implementation:
- Latin-1 compact representation where legal;
- UTF-16 backing when required;
- explicit UTF-8 conversion at host boundaries;
- immutable shared storage where profitable.

---

## 14. Garbage collection

The initial collector may be tracing mark/sweep.

The rest of the VM must access GC-managed objects through stable abstractions so the collector can evolve toward:
- generational;
- incremental;
- compacting;
- concurrent marking.

GC metrics are mandatory:
- heap used;
- heap committed;
- collection count;
- pause time;
- reclaimed bytes;
- allocation rate.

---

## 15. TypeScript and JSX

TypeScript is a tooling concern, not an ECMAScript semantic extension.

Pipeline:

```text
.ts/.tsx
   ↓
transform/parser layer
   ↓
JavaScript / engine AST integration
   ↓
engine
```

Oxc is an evaluation candidate for parsing/transforms/tooling because it is Rust-native and designed for high-performance JS/TS toolchains.

---

## 16. Plugin model

Preferred Velyr-native plugin model:
- WebAssembly Component Model;
- typed WIT interfaces;
- explicit capability injection;
- memory/fuel limits;
- no ambient host authority.

Node native addons remain a compatibility feature and are treated as elevated trust.

---

## 17. Observability

Runtime telemetry must be produced from Rust and include:

```text
process RSS
engine heap
GC pauses
event loop lag
pending host ops
active resources
open sockets/files
worker utilization
HTTP metrics
module resolution/load
permission denials
quota denials
```

OpenTelemetry integration is optional and feature-gated.

---

## 18. Error model

Errors must retain:
- stable code;
- human message;
- cause;
- resource/permission context where safe;
- JS stack;
- host span correlation ID.

Compatibility layers may map Velyr errors to Node-compatible error codes without changing kernel error identity.

---

## 19. Build modularity

Features are additive.

Avoid a single crate with hundreds of mutually-exclusive flags. Prefer crates/adapters.

Profiles are dependency selections, not runtime booleans.

The minimal executable must not pull:
- npm;
- Node;
- N-API;
- TLS;
- HTTP/2;
- WebSocket;
- bundler;
unless selected.

---

## 20. Testing pyramid

### Engine
- unit;
- parser snapshots;
- semantic tests;
- Test262;
- differential tests against reference engines;
- fuzzing.

### Runtime
- host-op unit tests;
- event-loop deterministic tests;
- permissions tests;
- quota tests;
- resource leak tests.

### Web
- adapted WPT suites;
- fetch/streams protocol tests.

### Compatibility
- Node behavior tests;
- package corpus;
- CITGM-like package test execution;
- React/Vue/Vite acceptance repos.

### Security
- fuzzing;
- Miri where feasible;
- sanitizers on unsafe/native boundaries;
- advisory scans;
- malicious package fixtures.
