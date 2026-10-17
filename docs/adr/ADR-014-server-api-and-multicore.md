# ADR-014 — Server API and multicore distribution

**Status:** Accepted (2026-10-15)
**Relates to:** `architecture/scheduler-workers.md`, SPEC §9, ADR-010

## Decision

### Server API
- Single isolate: `Velyr.serve({ port, hostname, signal, tls }, handler)`. The handler has the shape `(request: Request, info) => Response | Promise<Response>`. It returns a `Server` with `finished: Promise<void>` and `shutdown()`.
- Multicore entry: a module that does `export default { fetch(request, info) }`, run with `velyr serve [--workers=N|auto] main.ts`. The supervisor creates N isolates, and each one loads the module. The same module also works with `velyr serve` and a single worker.
- WebSocket server upgrade: `Velyr.upgradeWebSocket(request)` returns `{ socket, response }`.

### Distribution
- Linux and FreeBSD: each worker isolate binds its own listener with `SO_REUSEPORT`.
- macOS and Windows: a supervisor acceptor thread accepts connections and hands each stream off round-robin to the workers through a channel.
- Phase 05 benchmarks both strategies. A change of default needs a follow-up ADR.

### Worker count
- The default is 1 worker. Multicore is opt-in through `--workers` or the `workers` key in `velyr.json`.
- `auto` computes `min(available_parallelism honoring cgroup CPU quota, floor(memory_budget / per_worker_estimate))`. The per-worker estimate is 128 MiB, measured in ADR-016 (which supersedes the earlier 64 MiB placeholder).

### Workers API
- `new Worker(url, { type: "module", velyr: { permissions } })`. Only module workers are supported.
- Effective authority is `parent ∩ requested ∩ deployment_policy`.
- Structured clone goes through `engine-api` `serialize`/`deserialize`, which `engine-v8` implements with V8's `ValueSerializer`.
