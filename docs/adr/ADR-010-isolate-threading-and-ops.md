# ADR-010 — Isolate threading, host-op binding and resource IDs

**Status:** Accepted (2026-10-15)
**Relates to:** ADR-005, ADR-009, SPEC §1, §4

## Decision

### Threading
1. Every isolate runs on a dedicated OS thread that owns a Tokio `current_thread` runtime. The Velyr event loop is a future driven on that runtime. The engine isolate never leaves the thread.
2. Blocking host work (filesystem, `getaddrinfo`, CPU-heavy crypto, compression) runs through `tokio::task::spawn_blocking`. Its results come back as completions; they never touch engine state directly.
3. No work-stealing (multi-thread) Tokio runtime ever touches engine state. The supervisor (M5) runs on its own thread with its own `current_thread` runtime. Threads communicate only through `tokio::sync::mpsc` channels.
4. The process-wide V8 platform is initialized once by `runtime-core` (`std::sync::Once`).

### Host-op binding
1. Ops are Rust functions registered in an `OpRegistry` under a static name (`op_<area>_<verb>`, for example `op_fs_read_file`). Each op gets a numeric id at bootstrap.
2. Op implementations are engine-agnostic. They receive `&mut dyn CallContext` from `engine-api`, plus an `OpContext` (isolate id, capability set, quota context, cancellation token, trace span).
3. Ops are exposed only to bootstrap JavaScript, through a private binding object that bootstrap code captures and then removes from the global scope. User code never sees raw ops. Permission checks still happen inside the op (SPEC §3.11), never in JS wrappers.
4. Arguments and results cross as engine values. Binary data crosses as `ArrayBuffer`/typed arrays. JSON is never an internal boundary format.
5. Async ops return a Promise. Each isolate has a single `tokio::sync::mpsc::UnboundedReceiver<Completion>` completion queue, and each loop iteration processes at most `max_completions_per_tick` completions (default 1024) for fairness.

### Resource IDs
1. `ResourceId { index: u32, generation: u32 }`, with `index < 2^24` and `generation < 2^29`.
2. JavaScript sees one number: `generation * 2^24 + index`, which always fits a safe integer.
3. A stale or foreign id fails with `VELYR_BAD_RESOURCE`. It must never alias a newer resource.

## Consequences

- One isolate per thread matches SPEC §1 and the V8 `!Send` isolate model.
- I/O inside one isolate is single-threaded. Parallelism comes from workers (M5) and the blocking pool.
