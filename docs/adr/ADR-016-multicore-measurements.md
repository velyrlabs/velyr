# ADR-016 — Multicore measurements: per-worker memory, pointer compression, shared code cache

**Status:** Proposed (2026-10-16). The decision belongs to the project owner; nothing here is Accepted.
**Date:** 2026-10-16
**Relates to:** ADR-009 (V8), ADR-010 (isolate threading), ADR-014 (server API and multicore); VELYR-56/58 (`--workers=auto`), VELYR-60 (shared code cache), VELYR-61 (this benchmark), VELYR-156 (SO_REUSEPORT CI job)

## Context

ADR-014 fixed `--workers=auto = min(cpus, floor(memory_budget / per_worker_estimate))` with a placeholder estimate of 64 MiB "until Phase 05 measures it". VELYR-60 added an experimental shared code cache behind `--experimental-shared-code-cache` and left its default to this ADR. This ADR also records what is known about V8 pointer compression for a process that runs many isolates.

## Measurements

`benches/suites/multicore-http.toml` runs `benches/multicore/run.py`: `velyr serve --workers N` (N = 1, 2, 4, 8) with and without the shared code cache, 64 connections from `oha`, 6 s per configuration after a 2 s warm-up, fixture `tests/fixtures/m4-acceptance/server.js`. Raw data: `benches/baselines/m5-multicore-{plaintext,stream}-darwin-aarch64.json`. Release build, profile-full.

Machine: Apple M4 Pro, 12 cores, 24 GiB, macOS. **Load average during the runs: 8 to 12 (up to 19 in the repeat runs); `oha` ran on the same cores.** Treat every number as indicative. 16 workers were not run: the machine has 12 cores. Distribution: macOS has only the acceptor (handoff) path. **SO_REUSEPORT was not measured: the Linux comparison is pending and belongs to the reuseport CI job (VELYR-156).**

### Scaling (shared code cache off)

| scenario | workers | req/s | p99 ms | process CPU (100 = 1 core) | peak RSS MiB |
|---|---|---|---|---|---|
| plaintext | 1 | 36010 | 4.0 | 95 | 105 |
| plaintext | 2 | 45396 | 3.5 | 181 | 175 |
| plaintext | 4 | 53567 | 3.4 | 310 | 182 |
| plaintext | 8 | 65853 | 7.4 | 457 | 328 |
| stream | 1 | 18873 | 13.5 | 88 | 158 |
| stream | 2 | 30516 | 9.7 | 169 | 287 |
| stream | 4 | 36051 | 7.8 | 289 | 486 |
| stream | 8 | 46346 | 11.6 | 429 | 812 |

Throughput scales 1.8x at 8 workers for plaintext and 2.5x for stream, far from linear. The load generator and a single acceptor thread compete with the workers on 12 cores, and the machine was busy, so the curve is a lower bound on what the runtime can do; it does show that the acceptor path plus oha, not the number of isolates, is what flattens it past 4 workers. A fair read of the acceptor versus SO_REUSEPORT difference needs Linux.

### Memory per worker

Workers are threads of one process, so RSS is the process total; the per-worker figure is the slope against the 1-worker run.

| | idle (listening, no load) | under plaintext load | under stream load |
|---|---|---|---|
| RSS per extra worker | 5.0 to 6.5 MiB | 26 to 70 MiB (26 to 32 at 4 and 8 workers) | 85 to 130 MiB |

Idle memory is small; the cost is garbage that has not been collected yet under load, and it depends on the handler (a streamed body allocates a promise and a chunk per read). The VELYR-16 figure of 21.4 MiB idle per isolate is a standalone `velyr run`, which includes the process base.

## Decision (proposed)

### 1. per_worker estimate: 128 MiB

`PER_WORKER_ESTIMATE_BYTES` in `crates/runtime-workers/src/auto.rs` goes from 64 MiB to 128 MiB, which covers the 85 to 130 MiB seen under a streaming load. The estimate protects against going over a memory budget, so it follows the worst measured handler, not the idle figure. Cost: `--workers=auto` picks fewer workers in small containers (a 512 MiB budget gives 4, not 8). The real bound on a worker is a heap limit per isolate (`heap_limit_bytes`); setting one makes the estimate conservative instead of load-bearing. The change is applied in this ticket (constant, its tests, the `--workers` help text and the `auto_workers_are_capped_by_the_memory_budget` CLI test); ADR-014 still says 64 MiB and should point here if this ADR is accepted.

### 2. Pointer compression: recommendation only, do not enable now

The V8 build is not changed by this ADR.

- It cuts heap size for pointer-heavy heaps (V8 reports roughly 40%), which would shrink the loaded per-worker figure above.
- It needs a custom V8 build: the prebuilt `v8` crate artifacts do not enable it, and a custom build means giving up the prebuilt binaries ADR-009 chose.
- It caps the managed heap at 4 GiB. As far as I recall, V8's default pointer-compression cage is shared by every isolate of the process, so the sum of all isolates' heaps would have to fit in 4 GiB, which a `--workers=16` server with generous heap limits can exceed; newer V8 has isolate groups that lift this per group. This must be verified against the V8 version in use before relying on it.
- Recommendation: stay without it. Revisit when a real workload shows managed-heap RSS dominating the per-worker cost and a `heap_limit_bytes` budget (sum over workers) below 4 GiB is acceptable. If enabled, add a startup check that `workers x heap_limit_bytes` fits the cage.

### 3. Shared code cache: off by default

Numbers (VELYR-60, `benches/results/worker-startup-summary.md`): with a 100-module graph, workers after the first start 23% faster (27.5 to 21.3 ms) and the first one pays 16% (28.2 to 32.6 ms); peak RSS is 1.5 to 2 MiB higher and there is no per-worker RSS gain. In this benchmark's single-module server it has nothing to share: throughput and p99 were equal within the noise at 1 to 4 workers, and at 8 workers three repeats on a loaded machine gave 62.0k, 45.4k and 47.5k req/s with the cache against 61.1k, 62.1k and 56.6k without it, with a lower peak RSS in two of them (195 and 192 MiB against 326 and 335 in the first two repeats); the cause (noise or lock contention in the shared layer) was not isolated.

- Recommendation: keep `--experimental-shared-code-cache` off by default. The gain only exists for large module graphs and workers that start after the first (restarts, late scale-out); `serve --workers N` starts all workers together and they all miss. The shared source layer never invalidates, which a restarted worker can turn into serving stale files. Revisit with an mtime check on hit and a Linux run.

## Consequences

- `--workers=auto` is more conservative in small memory budgets.
- Pointer compression and the cache default stay as they are until measured on Linux and on a real application.
- Open: SO_REUSEPORT versus acceptor on Linux (VELYR-156 job), 16 workers on a machine with the cores, and a run on an idle machine.
