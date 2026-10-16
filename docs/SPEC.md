# SPEC — Velyr Runtime v0.1

This document uses **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** normatively.

---

## 1. Execution model

1. Each JavaScript isolate MUST own its mutable engine state.
2. Engine state MUST NOT be mutated concurrently from host worker threads.
3. Native async completions MUST be delivered to the owning isolate through the runtime event-loop boundary.
4. ECMAScript microtasks MUST follow the selected ECMAScript specification semantics.
5. Runtime-specific tasks MUST NOT starve microtasks indefinitely.
6. An isolate MAY be terminated when exceeding a configured hard resource budget.

---

## 2. Engine independence

1. `runtime-core` MUST depend on the engine abstraction, not a concrete engine.
2. A concrete engine MUST implement all mandatory engine lifecycle operations.
3. Engine adapters MUST NOT bypass runtime permissions to access host resources.
4. A reference-engine adapter MAY exist for differential testing.
5. Production features MUST NOT depend on undocumented V8-specific behavior unless explicitly isolated in a compatibility adapter.

---

## 3. Security

1. Arbitrary filesystem access MUST be denied by default.
2. Arbitrary outbound network access MUST be denied by default.
3. Network listeners MUST require explicit listen authority.
4. Environment access MUST be denied by default.
5. Process spawning MUST be denied by default.
6. FFI/native dynamic library loading MUST be denied by default.
7. Package lifecycle scripts MUST be denied by default.
8. Inspector/debug ports MUST be disabled by default.
9. Deny policy MUST take precedence over allow policy.
10. Worker authority MUST be equal to or narrower than its creator's authority.
11. Permissions MUST be checked at the host boundary, not only in JavaScript wrappers.

---

## 4. Resource ownership

1. Native resources MUST have an owning runtime context.
2. JavaScript-visible resource objects MUST NOT expose raw native pointers.
3. Runtime shutdown MUST close or cancel all owned resources.
4. Resource allocation MUST participate in configured quotas.
5. Resource-table entries MUST support diagnostics sufficient to identify leaks during testing.

---

## 5. Modules

1. ES modules MUST be the canonical module format.
2. CommonJS MUST be an optional compatibility module.
3. Module identity MUST use normalized canonical URLs/identifiers.
4. Package `exports` and `imports` semantics MUST be implemented in the Node compatibility/package resolver path where supported.
5. Loading local or remote modules MUST obey permission policy.
6. A module loader MUST be injectable for embedders.
7. Transformations MUST occur before engine compilation and MUST be cache-keyed.

---

## 6. Web APIs

1. Web APIs SHOULD follow corresponding WHATWG/W3C behavior.
2. The Minimum Common Web API SHALL define the primary interoperability baseline for server Web APIs.
3. Velyr extensions MUST NOT alter normative behavior of standardized interfaces.
4. Runtime-specific APIs SHOULD live under a Velyr namespace rather than modifying standardized prototypes.
5. Conformance SHOULD be tested through applicable Web Platform Tests.

---

## 7. Node compatibility

1. Node compatibility MUST be optional at build/product profile level.
2. `node:` specifiers MUST only be accepted when the relevant compatibility module exists.
3. N-API/native addons MUST require elevated native permission.
4. Direct V8 addon APIs are NOT a compatibility requirement.
5. Compatibility support MUST be versioned and reported per API/package, not represented as a single unsupported "100%" claim.

---

## 8. Package installation

1. Package artifacts MUST be integrity-checked when integrity metadata is available.
2. Lifecycle scripts MUST NOT execute implicitly.
3. Enabling lifecycle scripts SHOULD allow package-scoped authorization.
4. Installation SHOULD use a content-addressed global cache.
5. Resolver behavior MUST be deterministic for a lockfile.
6. Lockfile changes MUST be explicit and reviewable.

---

## 9. Workers and multicore

1. Workers MUST have independent mutable JS heaps.
2. Worker creation MUST be bounded by quotas.
3. Permission inheritance MUST use intersection semantics.
4. Message passing MUST support structured cloning where specified.
5. Shared memory MAY be supported only when language/WebAssembly semantics and security constraints are satisfied.
6. A server supervisor MAY distribute accepted connections or requests to worker isolates.

---

## 10. Performance

1. Security checks MUST NOT be removed for production benchmarks.
2. Benchmark configurations MUST be version-controlled.
3. Benchmark results MUST identify CPU, memory, OS, compiler profile and dependency versions.
4. Performance regressions beyond the repository budget MUST block merge unless waived with rationale.
5. Memory performance MUST include RSS and engine heap, not heap alone.
6. Claims against Node/Deno/Bun MUST use equivalent application semantics and security settings as far as practical.

---

## 11. Unsafe Rust

1. Crates that do not require unsafe code SHOULD use `#![forbid(unsafe_code)]`.
2. Unsafe code MUST be isolated to named low-level crates/modules.
3. Every unsafe block MUST document its safety invariant.
4. Unsafe boundary crates MUST receive targeted tests and fuzz/sanitizer coverage.

---

## 12. Observability

1. Runtime health metrics MUST remain available when application JS is CPU-saturated when technically possible.
2. Permission denials and quota denials MUST produce structured events.
3. Sensitive values such as secrets and environment contents MUST NOT be emitted by default.
4. Profiling/inspector features that expose memory/content MUST be explicitly enabled.

---

## 13. Compatibility gates

### Engine gate
A release MUST publish its Test262 result and known blockers.

### Web gate
A release MUST publish implemented Minimum Common Web API interfaces and WPT result scope.

### Node gate
A release that advertises Node compatibility MUST publish per-module status.

### Frontend gate
React/Vue/Vite compatibility is considered supported only when automated acceptance projects pass in CI.
