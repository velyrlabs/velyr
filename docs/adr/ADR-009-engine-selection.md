# ADR-009 — Initial production engine: V8 behind `engine-api`

**Status:** Accepted (2026-10-15)
**Date:** 2026-10-15
**Relates to:** ADR-004 (engine abstraction), ADR-005 (Tokio host runtime), ADR-007 (Node optional), ADR-008 (Wasm plugins)

## Context

Milestone 1 requires an engine that executes JavaScript through `engine-api`. ROADMAP and Phase 01 leave the choice open ("reference engine adapter or native bootstrap engine"). Every later milestone depends on this choice:

- M2 needs a solid Promise and microtask model.
- M4 needs `WebAssembly` from the Minimum Common Web API baseline.
- M7 and M8 need full modern ECMAScript, and enough throughput to run Vite, React SSR and Vue SFC compilation at competitive speed.
- M9 needs a host for Node-API.

The Velyr native engine (Track C, Phase 10) is a multi-year effort. Test262 drives its progress, and a JIT is explicitly deferred. It cannot be on the critical path of M1–M8.

The constraints from the existing documents are:

| Source | Constraint |
|---|---|
| SPEC §1 | One mutable engine state per isolate, never mutated across threads |
| SPEC §2 / ADR-004 | `runtime-core` depends only on `engine-api`; adapter types stay inside adapter crates |
| SPEC §11 | Unsafe code isolated in named crates, with fuzz/sanitizer coverage |
| PRD §10 | Optimize `useful_throughput / (cpu_time × rss)`, not microbenchmarks |
| PRD §12 | v1 on Linux/macOS/Windows, meaningful Test262 results, React/Vue/Vite suites |
| ROADMAP v1 gate | v1 needs a *documented* native engine status, not a production native engine |
| TDD §4 | The engine contract must not force the native engine to mimic V8 handles |

## Options considered

| | V8 (`v8` crate / rusty_v8) | QuickJS-NG (`rquickjs`) | Boa | SpiderMonkey (`mozjs`) | Native-only |
|---|---|---|---|---|---|
| ECMAScript conformance | Reference-grade | High, interpreter | Good, still maturing | Reference-grade | Starts at zero |
| CPU-bound throughput | JIT tiers | Interpreter only | Interpreter only, slower | JIT tiers | Interpreter, later |
| Startup / idle RSS | Heavier; snapshots help | Very small | Small | Heavier | Unknown |
| Binary size | Tens of MB | ~1 MB | Moderate | Tens of MB | Small |
| `WebAssembly` JS API | Built in | No | No | Built in | No |
| Rust embedding maturity | High (production use by Deno) | Good | Native Rust | Low outside Servo | n/a |
| Build | Prebuilt static libs per target | C via `cc` | Pure Cargo | Heavy C++ toolchain | Pure Cargo |
| Hostile-input hardening | Heavily fuzzed; JIT CVE history | Smaller surface, less scrutiny | Memory-safe Rust, less fuzzed | Heavily fuzzed | Unproven |
| Node-API precedent | Yes (Deno) | Possible | Possible | Possible | n/a |
| License | BSD-3 / MIT | MIT | MIT / Unlicense | MPL-2.0 | Own |

JavaScriptCore and Hermes were discarded early. JSC has no maintained Rust embedding. Hermes targets mobile ahead-of-time bytecode and its Rust embedding is immature.

**Native-only** would block M1–M8 on Track C. That contradicts the three-track model in ROADMAP. Rejected.

**QuickJS-NG** best fits the "small minimum surface" principle, but it fails the workloads the PRD prioritizes. An interpreter-only engine running Vite, Rollup, React SSR or `vue/compiler-sfc` would lose the Phase 4 comparative benchmark by a wide margin. It also lacks `WebAssembly`, which is a Minimum Common Web API target.

**Boa** is memory-safe Rust and philosophically aligned. It has the same interpreter-throughput gap as QuickJS-NG, and it is in practice a peer of Track C, not a reference for it.

**SpiderMonkey** matches V8 technically, but its standalone Rust embedding has a much smaller maintenance base, which makes it a higher build and maintenance risk with no offsetting advantage.

## Decision

1. **V8 is Velyr's initial production engine and its reference engine.** It is integrated through the `v8` crate (denoland/rusty_v8) in a new adapter crate, `engine-v8`. All profiles (`minimal`, `web`, `node`, `full`) use `engine-v8` until a profile is promoted to `engine-native` under the gate below.
2. **`engine-v8` is a named unsafe-boundary crate** (SPEC §11). No V8 type, lifetime or handle scope appears outside it (ADR-004, `specs/engine-api.md` forbidden pattern).
3. **`engine-api` must have two implementations from Phase 01 onward:** `engine-v8` and the `engine-native` bootstrap. A shared engine-api conformance suite runs against both. `engine-native` must pass the subset its implemented features cover. This keeps the trait engine-neutral and enforces TDD §4. No third engine is added.
4. **Velyr owns microtask timing.** `engine-v8` runs V8 with the explicit microtasks policy. Microtasks drain only through `Engine::drain_microtasks`, called by the Velyr event loop (`architecture/event-loop.md`).
5. **Each isolate is pinned to its owning thread** (SPEC §1). The process-wide V8 platform is initialized once by `runtime-core` startup, never by user code.
6. **A hardened `--jitless` mode is supported** as a deployment option for untrusted-code profiles, trading throughput for a smaller attack surface. It does not change the non-sandbox guarantee in `security/threat-model.md`.

## Guardrails

- **engine-api boundary.** Values crossing `engine-api` are runtime-owned opaque handles, or are accessed inside a callback-scoped region. A V8 `HandleScope` lifetime must never become an `engine-api` lifetime. A review rejects any trait change that the `engine-native` bootstrap cannot implement.
- **No V8-only semantics.** Observable behavior that is specific to V8 and not covered by ECMA-262 must not leak into Web or kernel behavior: stack trace format, `Error.captureStackTrace`, property enumeration quirks. If Node compatibility needs one of these, it lives in `compat-node` behind the adapter (SPEC §2.5).
- **Supply chain.**
  - Prebuilt V8 static libraries are pinned by exact crate version.
  - Their checksums are verified in CI.
  - The libraries are covered by `cargo deny` / `cargo audit`.
  - A from-source build of V8 is documented and run on at least one CI target per release, so that releases do not depend solely on third-party prebuilt artifacts.
- **Patch cadence.** V8 security fixes ship on Chrome's schedule. The release process must allow a V8 bump as a patch release, without waiting for feature work.
- **Measure, then budget.** The following are recorded in the benchmark schema as the baseline that later budgets and the native-engine gate are measured against:
  - Phase 01 records binary size, cold startup and idle RSS per isolate for `engine-v8`, in `minimal` and `full` builds.
  - Phase 05 records RSS per worker.
- **Two Wasm runtimes, two roles.** V8 provides the JS `WebAssembly` API. Velyr plugins use Wasmtime and the Component Model (ADR-008). These roles stay separate. Plugin authority never flows through the JS `WebAssembly` API.

## Native engine promotion gate

`engine-native` becomes the default engine for a profile only through a new ADR showing all of the following for that profile:

1. **Test262.** The known-failure manifest is published, with no unexpected failures in features the profile requires and no critical semantic blockers (`compatibility/ecmascript-test262.md`).
2. **Acceptance.** The profile's full Velyr suites pass on `engine-native`: runtime, WPT subset, and compatibility corpus where applicable.
3. **Efficiency.** `useful_throughput / (cpu_time × rss)` is at least equal to `engine-v8` on the profile's representative benchmarks. An explicit, accepted trade-off also qualifies: for example, `minimal` may accept lower throughput in exchange for lower RSS and faster startup.
4. **Security.** Fuzzing of parser, bytecode verifier and VM runs with no open critical findings, for a duration set by security review.
5. **Differential window.** `engine-v8` remains buildable as the reference engine for at least one release cycle after promotion.

`minimal` is the expected first candidate, because it has the most to gain from a small engine and the least ecosystem surface.

## Consequences

**Positive**

- M1–M8 are unblocked with a conformant, JIT-backed engine.
- Phase 4 comparisons against Node, Deno and Bun are meaningful, because the engine is not an obvious handicap.
- `WebAssembly`, startup snapshots (M11 edge snapshots) and DevTools inspector support come built in. The inspector stays deny-by-default.
- Deno proves that V8 works with permission-gated host ops, Web APIs and Node-API. This lowers execution risk for M3, M4 and M9.
- Track C proceeds independently, and V8 serves as its differential reference.

**Negative**

- The `minimal` profile is larger and heavier than the "small minimum surface" principle suggests, until `engine-native` is promoted. This conflicts with P1 edge users and is accepted as a temporary cost.
- Velyr inherits a large C++ dependency, its unsafe FFI surface, its build complexity and its JIT CVE history.
- The project depends on rusty_v8 maintenance cadence, which is driven by Deno. Mitigation: the `engine-api` boundary limits the blast radius, and the crate can be forked.
- Velyr shares an engine with Deno, so differentiation must come from architecture: modular profiles, the multicore supervisor, quotas and the native engine track.

## Not decided here

- **A second production adapter** (for example, QuickJS-NG for `minimal`/edge). Reconsider only if the measured footprint of `minimal` blocks P1 users before `engine-native` is promoted.
- **Pointer compression.** V8 pointer compression lowers RSS but caps the heap per isolate. Decide in Phase 05 from measured data.
- **Whether `engine-native` reuses an existing Rust engine codebase.** That belongs in a Track C ADR.

## Revisit triggers

- rusty_v8 is no longer maintained or falls behind V8 security releases.
- Measured `engine-v8` footprint makes the `minimal` profile unviable for target edge platforms.
- The engine-api conformance suite cannot stay engine-neutral without V8-shaped types.
- `engine-native` satisfies the promotion gate for any profile.

## Documents to update on acceptance

- `ROADMAP.md` M1: replace "reference engine adapter or native bootstrap engine" with `engine-v8` plus the `engine-native` bootstrap.
- `plans/phase-01-engine-bridge.md`: add `engine-v8`, the engine-api conformance suite, and the Phase 01 footprint baseline.
- `TDD.md` §3 and `README.md` topology: add the `engine-v8` crate.
- `performance/benchmark-plan.md`: "Velyr reference engine build" means `engine-v8`.
