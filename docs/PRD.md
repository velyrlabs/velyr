# PRD — Velyr Core

**Status:** Draft v0.1  
**Product:** Velyr Core  
**Category:** JavaScript/TypeScript runtime  
**Implementation language:** Rust  
**Primary domain:** Server, edge, CLI, workers and developer tooling

---

## 1. Problem statement

The JavaScript server ecosystem offers excellent developer productivity and package availability, but production deployments often carry costs from runtime footprint, duplicated worker state, process-oriented multicore scaling, broad dependency permissions, and compatibility layers that are always present even when an application does not need them.

Velyr addresses this by building a runtime around:

- modular compilation;
- explicit capabilities;
- small minimum surface;
- Web-standard APIs;
- optional Node/npm compatibility;
- a Rust host runtime;
- measurable performance;
- vertical and horizontal scaling;
- a replaceable JavaScript engine boundary.

Velyr MUST NOT define success as "Rust is faster than Node." Success is demonstrated through reproducible workloads.

---

## 2. Vision

> Deliver a modern JavaScript/TypeScript runtime that uses only the capabilities an application needs, denies privileged operations by default, scales efficiently across cores, and preserves ecosystem compatibility through clean optional layers.

---

## 3. Target users

### P0 — Backend/platform engineers
Need APIs, workers, WebSockets, queues, streaming, predictable memory and infrastructure efficiency.

### P1 — Edge/serverless platform authors
Need fast startup, hard quotas, sandbox boundaries and minimal binaries.

### P1 — JavaScript/TypeScript developers
Need ESM, npm compatibility, Web APIs, TypeScript tooling and familiar workflows.

### P2 — Frontend developers
Need Vite, React, Vue, SSR and build-tool compatibility.

### P2 — Embedders
Need a programmable JS engine/runtime that can be embedded inside a Rust application with a controlled host API.

---

## 4. Non-goals

Velyr v1 does NOT aim to:

- clone Node source code;
- preserve every undocumented Node behavior;
- implement a browser DOM or rendering engine;
- replace npm registry infrastructure;
- allow arbitrary FFI by default;
- share one mutable JavaScript heap concurrently across OS threads;
- write TLS or cryptographic primitives from scratch;
- guarantee faster execution than V8 on all CPU-bound JavaScript;
- make Next.js/Nuxt a v0 requirement;
- implement a production optimizing JIT before semantic correctness.

---

## 5. Product principles

### 5.1 Secure by default
No arbitrary filesystem, network, environment, process or FFI access without authority.

### 5.2 Web standards first
Portable server APIs precede runtime-specific extensions.

### 5.3 Node compatibility optional
Node compatibility is a product layer, not a kernel invariant.

### 5.4 Everything measurable
Performance, conformance, compatibility and security regressions have explicit gates.

### 5.5 Everything modular
No feature should be linked into a minimal build unless required by its selected profile or dependency graph.

---

## 6. Core product requirements

### PR-001 — Runtime execution
Velyr SHALL execute JavaScript files and modules.

### PR-002 — Engine abstraction
The host runtime SHALL depend on an engine interface rather than engine internals.

### PR-003 — Native engine path
The architecture SHALL support a Velyr-native ECMAScript engine implemented in Rust.

### PR-004 — Async host runtime
The runtime SHALL support asynchronous host operations and Promise resolution.

### PR-005 — ESM first
ES modules SHALL be the native module format.

### PR-006 — Capability security
Privileged operations SHALL require capabilities resolved through a central policy layer.

### PR-007 — Resource quotas
The runtime SHALL expose configurable memory, worker, socket, file, body-size and execution budgets.

### PR-008 — Web API baseline
The runtime SHALL progressively implement the Minimum Common Web API / WinterTC surface applicable to server runtimes.

### PR-009 — Node compatibility
Node core modules, CommonJS, npm semantics and Node-API SHALL be independently optional.

### PR-010 — Multicore
Velyr SHALL support a supervisor/worker model that can utilize multiple CPU cores without users manually creating a cluster topology.

### PR-011 — Observability
Runtime-level metrics SHALL remain observable independently of application JavaScript.

### PR-012 — Reproducible benchmarks
Every performance claim SHALL be reproducible from repository benchmark definitions.

---

## 7. Developer experience

Target CLI:

```bash
velyr run app.ts
velyr run --allow-net api.ts
velyr test
velyr bench
velyr add react
velyr dev
velyr build
velyr check
velyr compat ./project
```

Target permission flow:

```bash
velyr run \
  --allow-read=./data \
  --allow-write=./cache \
  --allow-net=api.example.com:443 \
  --allow-env=DATABASE_URL \
  server.ts
```

Target build profiles:

```bash
velyr build --profile=minimal
velyr build --profile=web
velyr build --profile=node
velyr build --profile=full
```

---

## 8. Compatibility targets

Compatibility is a matrix, never a boolean.

### ECMAScript
Measured using Test262 and Velyr-specific semantic tests.

### Web
Measured using selected Web Platform Tests and Minimum Common Web API requirements.

### Node
Measured per public core API behavior and ecosystem suites.

### npm
Measured against package resolution, package.json fields, exports/imports, lifecycle policy and package corpus tests.

### Frontend
Measured by real applications and upstream test suites for:
- React
- Vue
- Vite

Next/Nuxt are later compatibility tracks.

---

## 9. Security requirements

- Deny privileged I/O by default.
- Deny lifecycle scripts by default.
- Deny FFI/native addons by default.
- Deny inspector exposure by default.
- Child workers can never gain broader authority than the parent.
- Host operations must pass capability checks.
- Resource handles cannot expose raw OS pointers to JS.
- Unsafe Rust must be localized and reviewed.
- Dependency advisories and license policy are CI gates.
- Parser, resolver, bytecode verifier and protocol boundaries are fuzz targets.

---

## 10. Performance goals

All goals are hypotheses until benchmarked.

Primary metric:

```text
efficiency = useful_throughput / (cpu_time × rss)
```

Secondary metrics:
- startup time;
- idle RSS;
- peak RSS;
- requests/core;
- requests/GB;
- P50/P95/P99 latency;
- event-loop lag;
- GC pause distributions;
- worker startup;
- package/module load time;
- JS ↔ Rust boundary overhead.

Velyr MUST optimize for server economics rather than microbenchmark leadership alone.

---

## 11. Release profiles

### Minimal
Engine + core + ESM only.

### Web
Minimal + networking + fetch + request/response + streams + HTTP/TLS selected features.

### Node
Web + Node compatibility + CommonJS + npm resolver.

### Full
All stable production capabilities, excluding explicitly experimental engine/JIT features unless enabled.

---

## 12. Acceptance criteria for v1.0

v1.0 requires:

- stable engine/host interface;
- reliable Promise/microtask/event-loop integration;
- ESM and production module graph;
- capability security;
- resource quotas;
- Linux/macOS/Windows support for core runtime;
- meaningful Test262 conformance with no known critical semantic blockers;
- Minimum Common Web API target subset documented and tested;
- HTTP/1, HTTP/2 and TLS production readiness;
- workers/multicore;
- compatibility dashboard;
- npm package installation/resolution for supported package classes;
- React + Vue + Vite acceptance suites;
- performance regression CI;
- threat model and coordinated disclosure process.

Node-API coverage may remain partial if clearly versioned and documented.
