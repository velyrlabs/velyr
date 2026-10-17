# Large-app footprint: Velyr vs Node (VELYR-183)

Date: 2026-10-16. Machine: Apple M-series, 12 cores, 24 GB, macOS (darwin-aarch64). Node v26.3.1.
Velyr: `target/release/velyr` (profile full, V8) and `target/native-bench/release/velyr` (built with
`--features engine-native`). Results: `benches/results/footprint-large-1792164637.json` (every round)
and `benches/results/footprint-large-latest.json` (medians, the shape `velyr-bench` reads).

## What was measured

The fixture (`benches/large-app`, `python3 -I benches/large-app/gen.py`, hash checked twice) has
2,000 modules (80% ESM, 20% CommonJS, 3,958 import edges), 50,000 live cache objects and three routes
with real work: `/json` (about 100 KB), `/transform` (24 chained string transforms) and `/template`
(150 HTML rows, about 28 KB). The server loads the whole graph before answering.

- Velyr V8 and Node over HTTP, 1 and 4 workers (Node: one process, or cluster with 4 workers).
- In-process mode (no HTTP, handlers called in a loop) for Velyr V8, Node and the native engine.
- 3 rounds per configuration, medians below. `oha` with 64 connections, 10 s per route, 2 s warm-up.
  RSS is the whole process tree (cluster workers included; Velyr workers are threads). "Idle" is after
  the graph load and one request per route; "peak" is the maximum sampled under load.
- Everything ran inside `rss-guard.sh 3000`. The guard printed `child setpgid ... Operation not
  permitted` in the background runs, so its process-group kill may not have been armed there; the
  largest tree seen was 1.6 GB (Node, 4 workers).
- The machine was not idle: load average was 5 to 7 at the start (other work on the Mac). `oha` shares
  the cores. Treat the absolute numbers as indicative; the ratios are more stable (rounds differ by
  less than 3% in memory and less than 8% in req/s).

### Native engine

`velyr serve` has no engine switch, so the native engine only runs in process (`velyr run --engine
native inproc.js`). Two further limits, recorded as found, nothing was fixed in the engine:

1. The fixture as generated does not load: `Uncaught Cannot read properties of undefined (reading
   'importCjs')` at `src/g00/m004.cjs:1:13` (the native engine has no CommonJS host).
2. With `gen.py --cjs-every 0` (same 2,000 modules, all ESM; the copy lives outside the repo) it loads,
   but `URL` is not defined (`ReferenceError: URL is not defined` in `app.js`). The template's
   `handle()` now parses the path by hand, so native and V8 run the same code.

The native row therefore runs a different fixture (ESM only) in a different mode. It is a data point,
not an apples-to-apples comparison.

## Memory

| Config (median of 3) | Idle RSS MB | RSS under load MB | Peak RSS MB |
|---|---:|---:|---:|
| Velyr V8, 1 worker | 131.9 | 390.5 | 400.5 |
| Node, 1 process | 181.2 | 372.3 | 375.6 |
| Velyr V8, 4 workers | 451.3 | 1217.0 | 1280.1 |
| Node, cluster 4 workers | 787.9 | 1554.7 | 1568.8 |
| Velyr V8 in process | 127.6 | n/a | 517.0 |
| Node in process | 169.6 | n/a | 570.8 |
| Velyr native in process (ESM only) | 311.2 | n/a | 385.7 |

## Performance

| Config | First response ms | Graph load ms | req/s (mean of 3 routes) | p99 ms (worst route) | CPU ms/request | req/s per peak MB |
|---|---:|---:|---:|---:|---:|---:|
| Velyr V8, 1 worker | 1648 | 1531 | 3873 | 27.2 | 0.32 | 9.70 |
| Node, 1 process | 523 | 362 | 4360 | 58.7 | 0.28 | 11.61 |
| Velyr V8, 4 workers | 2061 | 1963 | 12650 | 12.7 | 0.38 | 10.09 |
| Node, cluster 4 workers | 711 | 423 | 15754 | 16.1 | 0.31 | 10.03 |
| Velyr V8 in process | 1762 | 1648 | 3899 | n/a | n/a | 7.54 |
| Node in process | 426 | 289 | 3537 | n/a | n/a | 6.21 |
| Velyr native in process (ESM only) | 1472 | 920 | 43 | n/a | n/a | 0.11 |

Native state build (50k objects) took 523 ms against 46 ms on Node and 31 ms on V8.

## Velyr / Node ratio

| Metric | 1 worker | 4 workers |
|---|---:|---:|
| Idle RSS | 0.73 | 0.57 |
| RSS under load | 1.05 | 0.78 |
| Peak RSS | 1.07 | 0.82 |
| First response | 3.15 | 2.90 |
| Graph load | 4.23 | 4.64 |
| req/s | 0.89 | 0.80 |
| p99 | 0.46 | 0.79 |
| CPU per request | 1.14 | 1.23 |
| req/s per peak MB | 0.84 | 1.01 |

Velyr uses 27% (1 worker) to 43% (4 workers) less idle memory than Node, but only 18% less peak with
4 workers and 7% more peak with 1 worker. It starts about 3 times slower, because of the graph load,
and spends 14 to 23% more CPU per request.

## Distance to the goal (memory 90% below Node)

| | Node | Goal (10% of Node) | Velyr V8 today | Over the goal |
|---|---:|---:|---:|---:|
| Idle, 1 worker | 181.2 | 18.1 | 131.9 | 7.3x |
| Peak, 1 worker | 375.6 | 37.6 | 400.5 | 10.7x |
| Idle, 4 workers | 787.9 | 78.8 | 451.3 | 5.7x |
| Peak, 4 workers | 1568.8 | 156.9 | 1280.1 | 8.2x |

The goal is far: today's best case is 43% below Node (idle, 4 workers). The app's own data is a floor
for any engine on this fixture: V8 `heap_used` is 67.6 MB at idle, mostly the 50,000 cache objects and
the 2,000-module graph.

## Where Velyr's memory goes (V8, 1 worker)

Sources: `/stats`, the runtime metrics (`--metrics-addr`: `velyr_heap_used_bytes`,
`velyr_heap_total_bytes`) and `vmmap -summary` (the dirty column) taken idle and after 3 x 8 s of load.

| State | RSS (ps) | V8 heap used / total | Dirty total |
|---|---:|---:|---:|
| Idle | 131 MB | 67.6 / 80.8 MB | 111.6 MB |
| After load | 412 MB | 108.1 / 143.1 MB (81.8 / 145.2 after GC) | 332.7 MB |

Dirty memory by region, idle then after load:

| Region | Idle MB | After load MB |
|---|---:|---:|
| V8 heap (`App-Specific Tag 16`) | 77.0 | 138.5 |
| malloc, small (live) | 22.3 | 98.9 |
| malloc, small (freed, not returned) | 0.1 | 76.4 |
| Kernel page tables | 9.3 | 14.9 |

The three largest sources of Velyr's memory under load:

1. **malloc retention and fragmentation: about 175 MB.** The Rust side allocates and frees request
   and response buffers per request. vmmap reports only 21.3 MB allocated in the malloc zones, with
   155.5 MB fragmentation (88%) and 76 MB sitting in "empty" pages that stay resident. It grows 152 MB
   during load and never shrinks (the "settled" reading 3 s later is unchanged). This is the biggest
   difference from Node's profile and the main reason the 1-worker peak is above Node's.
2. **V8 heap: 138.5 MB.** 77 MB at idle (app state and graph) and 61 MB of growth under load
   (garbage of the large `/json` and `/template` strings and new-space growth). Node has the same
   engine and a similar heap, so this part is not a differentiator.
3. **Fixed floor at idle: about 32 MB outside the V8 heap** (malloc 22.8 MB, page tables 9.3 MB), plus
   the binary's text (356 MB mapped, 166 MB resident, clean pages).

Not separable: the clean, file-backed pages of the binary (they count in RSS but not as dirty), and
how much of the 22 MB idle malloc is the module loader's source and path tables versus the HTTP stack:
nothing exposed attributes it. `GcStats` (with `external_bytes`, VELYR-185) belongs to the native
engine's Rust API and is not surfaced by the CLI or the metrics. For the native engine the runtime
metric reports `heap_used` = 0.54 MB while the process RSS is 324 MB, so 99.8% of the native engine's
memory is not attributed by what is exposed today. That is a gap to fix before the native engine can
be optimised for memory.

## Where Velyr's CPU goes

Method: `sample <pid> 5` during 10 s of load on one route (V8, 1 worker), classifying the top-of-stack
frames of the busy threads; idle waits (`__psynch_cvwait`, `__ulock_wait`, `kevent`) are excluded.
Busy samples were about 5% of all thread samples (most threads wait). GC cannot be isolated well: V8
collects on helper threads and the visible GC frames were under 1% of busy samples (5 of 2,800 in
`/transform`, 6 of 2,844 in `/template`); no GC time counter is exposed. So the GC share is "small
where visible", not measured.

Steady state, share of busy samples:

| Source | /json | /transform | /template |
|---|---:|---:|---:|
| JS execution: string, RegExp, Array builtins and JIT code | 19% | 94% | 70% |
| `JSON.stringify` (V8) | 39% | 0% | 2% |
| V8 string to Rust string copy for the response body (`JsString::to_string_lossy`) | 25% | 0% | 11% |
| malloc, memmove and memory syscalls | 8% | 2% | 11% |
| Socket syscalls (`sendto`, `recvfrom`) | 8% | 3% | 6% |

Start-up (graph load, 1.5 s of wall time, one sample of the first second, 446 busy samples):

| Source | Share |
|---|---:|
| File-system syscalls (`read`, `open`, `getattrlist`) | 35% |
| malloc, free, memmove | 34% |
| Hash maps (SipHash and hashbrown in `velyr_engine_v8`) | 15% |
| String conversion V8 <-> Rust | 7% |
| V8 parse and compile | 3% |

The three largest CPU sources of Velyr:

1. **Graph load (loader and host work): 1.5 s once, 4.2 to 4.6 times Node's.** V8 parse and compile is
   only 3% of it; the time is file reads and stats, allocation and hashing around 2,000 modules.
2. **Interpretation of the app's JS** (string, RegExp and Array builtins and JIT code): 70 to 94% of
   `/transform` and `/template`. Same engine as Node, so no ratio gap in itself; it explains most of the
   CPU per request.
3. **Copying the response body from a V8 string into a Rust `String`:** 25% of `/json` and 11% of
   `/template`. This is Velyr-only work and is part of why CPU per request is 14 to 23% above Node's.

GC, as above, is not separable. A sample of a 5 s window on 1 worker is a coarse tool: single sample,
no symbols for JIT frames (`???` frames are counted as "JS execution" only because their callers are
builtins; roughly 40% of `/transform` busy samples are such frames).

## Recommendations

1. **Allocator** (`crates/cli/src/main.rs`, global allocator; request buffers in `crates/web-fetch`):
   try mimalloc or jemalloc with purge/decay, or per-worker arenas, in place of the system allocator.
   The 155 MB of fragmentation (88%) is the largest single item; if it is recovered, the 1-worker
   peak drops from 400 MB to roughly 250 MB, about a third below Node's 376 MB (an estimate to confirm:
   it was not tried here).
2. **Response body path** (`JsString::to_string_lossy` in `crates/engine-api/src/value.rs` and the
   serve code in `crates/web-fetch`): write the V8 string straight into the outgoing buffer (one-byte
   strings are already Latin-1/UTF-8 compatible after a check) or reuse one buffer per connection,
   instead of allocating a `String` per response. Targets the 11 to 25% CPU share and the allocation
   churn behind recommendation 1.
3. **Module loader** (`crates/runtime-modules`, `crates/engine-v8` module maps): use a fast hasher in
   place of SipHash, resolve once and cache the result, avoid the `stat`+`open`+`getattrlist` sequence
   per file, and persist the V8 code cache between runs (the experimental shared code cache of VELYR-60
   is in-process only). Target: graph load from 1.5 s to below 0.4 s, which also lifts the 4-worker case
   where every isolate repeats the load (2.0 s).

Reaching 90% below Node needs more than these: the 50,000-object state alone is about 40 MB in V8. The
native engine would have to shrink its 311 MB idle by an order of magnitude and expose its heap
breakdown first, and the generated fixture must load on it (`.cjs` host, `URL`) before that comparison
is meaningful.

## Native

VELYR-196, 2026-10-16. `velyr serve --engine native` on the full fixture (CommonJS included, it loads
now), idle after one `/json`. Binary: `target/native-bench/release/velyr`. Readings: the runtime
metrics (`velyr_process_resident_memory_bytes`, `velyr_memory_bytes{kind}`) and `vmmap -summary` of
the same process; one `malloc_history -allBySize` run (`MallocStackLogging=lite`) to name sources.
Load average 5 to 10 during the runs. MB is 10^6 bytes; vmmap's MiB were converted.

RSS is noisy on this machine: under memory pressure macOS compresses pages, which leaves RSS but
stays in the physical footprint (one run read 172.6 MB RSS with 101.7 MiB compressed). The RSS rows
below are runs with no compressed pages; the physical footprint is the stable reading.

### Before and after

| Native, large-app idle | Before | After | Change |
|---|---:|---:|---:|
| RSS (metric) | 318.0 MB | 276.1 MB | -13.2% |
| Physical footprint (vmmap) | 344.8 MB | 263.6 MB | -23.5% |
| malloc allocated (vmmap) | 323.7 MB | 245.2 MB | -24.3% |
| malloc allocations | 2.26 M | 2.02 M | -10.6% |
| malloc fragmentation | 17.4 MB (6%) | 14.9 MB (6%) | |

An empty `serve` app on the final binary: 47.0 MB RSS, 13.3 MB of it counted (code 5.5 MB of bootstrap
JS, shapes 3.8 MB).

Steps, footprint and malloc allocated in MiB (vmmap units):

| Step | Footprint | Allocated |
|---|---:|---:|
| Before | 328.8 | 308.7 |
| `Date` local zone from `/etc/localtime` (no ICU load), `shrink_to_fit` of finished code, one inline cache per named access site (was one per instruction), one string constant per text per module | 290.7 | 270.9 |
| `JsObject` 144 -> 96 bytes (dictionary and sparse storage boxed), property slot 32 -> 16 bytes (accessor pair boxed), string header 80 -> 64-byte malloc block | 254.7 | 234.6 |
| Attribution counters (no reduction intended) | 251.4 | 233.8 |

`malloc_history` names the larger single gains: the ICU time-zone data that `iana_time_zone` loaded
through CoreFoundation (36.4 MB, gone) and the inline caches (31 MB, now about 6 MB). The two steps
were not measured change by change.

### Where the memory goes

`velyr_memory_bytes{kind}` (also `HeapStats::breakdown` and `GcStats::breakdown`) is kept by counters
updated at allocation and release, never by walking the heap. Strings and storage count against the
heap limit (they make up `velyr_heap_external_bytes`); the other kinds are only attributed.

| Kind | MB | Share of RSS | What |
|---|---:|---:|---|
| code | 68.2 | 24.7% | bytecode, constants, line tables, inline caches |
| strings | 43.4 | 15.7% | string headers and payloads |
| storage | 32.9 | 11.9% | object properties and elements, Map and Set tables |
| cells | 28.7 | 10.4% | GC cells (objects, functions, contexts) |
| source | 10.9 | 3.9% | source text kept for `toString` and stack traces |
| tables | 7.3 | 2.7% | function table, closure records, code info map |
| shapes | 7.2 | 2.6% | hidden classes |
| heap_tables | 5.6 | 2.0% | the collector's slot tables |
| interner | 0.1 | 0.0% | atom table |
| **Counted** | **204.3** | **74.0%** | |
| malloc free space (vmmap fragmentation) | 14.9 | 5.4% | |
| Outside malloc (RSS minus malloc resident: binary data, stacks, page tables) | 16.1 | 5.8% | |
| In malloc, named by `malloc_history`, not counted | 10.4 | 3.8% | loader `SourceTransform` text 6.7 (runtime-modules), module records 3.7 |
| **Not attributed** | **30.4** | **11.0%** | malloc rounding, runtime and HTTP stack, loader tables |

Bytecode mix of the fixture (`footprint` test `code_mix_of_the_large_app`): 43,829 functions, 1.80 M
instructions at 20 bytes (35.9 MB), 0.80 M line-table entries at 12 bytes (9.6 MB), 0.31 M constants.

### Performance

`accounting_throughput_probe` (best of 7): 702 to 740 ms before, 710 to 717 ms after. In process
(`velyr run --engine native inproc.js`, 1.5 s per route): `/json` 72.9, `/transform` 11.9 to 12.7,
`/template` 45.0 req/s, mean 43.6, the same as the 43 recorded above; there is no same-day binary from
before the change to compare against. Test262 (native, `--threads 1`): `built-ins/Date`, `String`,
`Object`, `Map`, `Set`, `WeakMap` and `language/expressions/object` have no new failures.

### What is left for -30%

The RSS target (222.6 MB) is 53.5 MB away. Code is the largest kind and most of it belongs to
functions the app never calls:

1. **Lazy compilation** (`crates/engine-native/src/compiler/bytecode/function.rs`): compile a function
   body on its first call. The compiler resolves every binding to a register or context slot from the
   whole-program scope analysis, and the AST only lives during the compile, so this needs the outer
   scope layout kept per function (V8's ScopeInfo) and a re-parse of the function's text. Up to
   most of the 68 MB.
2. **Smaller instructions** (`crates/engine-vm/src/bytecode.rs`): `Reg` and `RegRange` are `u32`; 16-bit
   operands would bring `Instruction` from 20 to 12 or 16 bytes (7 to 14 MB here). The line table could be
   delta-encoded (9.6 MB to about 3 MB).
3. **Source kept twice**: the loader's transformed text (6.7 MB) and the `Script` copy (10.9 MB with
   the line table) could share one buffer; that crosses into `runtime-modules` and the `Source` API.
4. **Shapes of builtins**: 3.8 MB of the 7.2 MB already exist in an empty isolate, because every
   method added to a prototype copies the layout (the shortcut noted in `shape.rs`).

### Lazy function bodies (VELYR-201)

2026-10-16, same fixture, binary and method as above, load average 7 to 10 (other builds running).
Only item 1 of the list was needed for the target. A nested function now compiles to a stub (its
header: name, length, kind, strictness, `toString` span) and its body compiles on the first call.
Function expressions and arrows met by top-level script or module code still compile at load, since
that code tends to call them right away. The first call parses only the function's own text, wrapped
as a parenthesized expression, and resolves what that text leaves unresolved through the outer chain
the stub keeps (`compiler/bytecode/outer.rs`, like V8's ScopeInfo): one entry per context on the
chain, shared by every stub below it, with its captured bindings, private names, `arguments` slot and
whether it is the module context.

| Native, large-app idle | VELYR-196 baseline | 196 after | 201 after | Change vs baseline |
|---|---:|---:|---:|---:|
| RSS (metric), 3 runs | 318.0 MB | 276.1 MB | 209.0 to 216.6 MB | -31.9% to -34.3% |
| Physical footprint (vmmap) | 344.8 MB | 263.6 MB | 199.0 to 203.9 MB (189.8 to 194.5 MiB) | -40.9% to -42.3% |
| malloc allocated (vmmap) | 323.7 MB | 245.2 MB | 187.7 MB | -42.0% |
| Compressed pages | | | 0 | |
| `code` | | 68.2 MB | 27.1 MB | |
| `strings` | | 43.4 MB | 38.6 MB | |

The other kinds did not move. The 27 MB of code left is the top-level code of 2,000 modules, the
functions `/json` called and the bootstrap.

Performance, same day, the VELYR-196 binary against this one, alternated:

| | 196 after | 201 after | Change |
|---|---:|---:|---:|
| `graphLoadMs` (median of 6) | 992 | 847 | -14.6% |
| `/json` req/s (mean of 6) | 74.18 | 72.27 | -2.6% |
| `/transform` req/s | 12.60 | 12.47 | -1.0% |
| `/template` req/s | 44.65 | 45.19 | +1.2% |
| `accounting_throughput_probe` (best of 7, 3 runs) | 714.5 ms | 717.9 ms | +0.5% |

`/json` pays for the code generation that used to happen at load: in 1.5 s it makes about 110 calls
over 97 seeds, and each first call of a formatter or scorer compiles it. Compiling every top-level
function of the fixture once (`footprint` test `compile_time_split_of_the_large_app`): load 202 ->
88 ms for the 2,040 modules, then 151 ms for the 31,069 bodies (4.9 us each). A first version that
parsed the whole module again on each first call took 901 ms for the same bodies and cost `/json` 21%.

Behavior changes:

- Early errors (and the scope analysis' `unsupported: direct eval` and `with`) are still reported at
  load. Other `unsupported` constructs in a function body throw their SyntaxError on its first call.
- A stub's `uses_super_property` (whether the closure gets a [[HomeObject]]) checks its text for
  `super`; a false positive sets a home object nothing reads.
- Parameter expressions do not see the body's bindings in the outer chain either
  (`scope-paramsbody-var-open.js`).

Test262 (native, `--threads 1`): `language/statements/function`, `language/statements/class`,
`language/expressions` and `built-ins/Function` have no new failures; their 47 new passes were removed
from the expectations.

### Residual items 2 to 4 (VELYR-213)

2026-10-16, same fixture and `velyr serve --engine native`, binary built from the working tree with
`cargo build --release -p velyr-cli --features engine-native --target-dir target/native-bench`.
Each run: boot, one request per route, 1.5 s, then idle RSS (`ps`), physical footprint (`vmmap
-summary`) and `velyr_memory_bytes{kind}`; then 4 s of `oha -c 8` per route with RSS sampled every
100 ms for the peak and a 3000 MB guard (kill above it; never fired). No `rss-guard.sh` exists in this
repo, so the guard was the sampler itself. A and B alternated, 3 runs each, medians. Load average 9
to 40 (other agents building), and macOS compressed pages in some runs (RSS 148 to 167 MB with an
unchanged footprint), so idle RSS swings by 70 MB between identical runs; the footprint and the
counters are the readings to trust. Rule applied at measurement time: an item below -5% is reverted (later relaxed for item 4, see below).

| Item | Idle RSS MB (A -> B) | Footprint MiB (A -> B) | Peak RSS MB (A -> B) | Counter moved | Probe ms (A -> B) | Decision |
|---|---:|---:|---:|---|---:|---|
| 4. Shapes: shared layouts | 219.5 -> 156.9 (noise) | 202.0 -> 195.2 (-3.4%) | 220.6 -> 212.1 | `shapes` 8.3 -> 1.7 MB | 826.1 -> 758.4 | kept (human decision) |
| 2. Compact instructions | not built | bound: -2.1% | | instructions 11.0 MB | | not attempted |
| 3. Single source | not built | bound in scope: -0.5% | | `source` 11.1 MB | | blocked by scope |

Item 4: shapes in a chain of `Add` transitions shared one layout (entries plus slot map) and each
shape kept only its length, the way V8 shares descriptor arrays; the tip of a chain extended the
layout in place, a branch or a reconfiguration copied its prefix. It removed the O(n^2) copy noted in
`shape.rs`: `shapes` went from 4.9 to 1.6 MB in an empty isolate (RSS 44.7 -> 41.9 MB) and from 8.3 to
1.7 MB in the large app, but that is 3.4% of the footprint, below the bar. The probe ran 3 times on
each side at different moments (B first, at load 15 to 20), so its 8% gain is load, not the change; it
shows no regression. It was first reverted by hand under the 5% rule, then reapplied by human decision
(VELYR-213 follow-up): items below 5% are accepted when they are cheap and cause no regression, and this
one is both. `crates/engine-vm/src/object/shape.rs` is the only file changed. After reapplying,
`cargo test -p velyr-engine-vm` and `cargo test -p velyr-engine-native` pass, and
`accounting_throughput_probe` ran at 835 and 833 ms (best of 7, twice), within noise of the 826 ms
baseline (no slowdown above 10%). The 1.7 MB left is per-shape headers and transition maps.

Item 2: a temporary counter (charged in `Emitter::finish`, removed after the run) read 11.0 MB of
`Instruction` at idle, out of 27.9 MB of `code`; the rest is stubs (36,505 functions in the fixture,
about 145 bytes of `FunctionCode` each), constants, line tables and inline caches. 16-bit `Reg` and
`RegRange::count` would bring an instruction from 20 to 12 bytes: at most 4.4 MB, 2.1% of the footprint,
so it cannot pass the bar and was not built (it touches every register use in the compiler and the
interpreter). `instruction_stays_at_twenty_bytes` is unchanged.

Item 3: the engine already keeps one copy per module, an `Rc<Script>` (text and line starts) shared by
the module's code and every lazy stub, which re-parses from it. `source` is 11.1 MB: 7.6 MB of fixture
text, 1.1 MB of line starts (276k lines at 4 bytes), the bootstrap scripts and names. The second copy is
the loader's transformed text (6.7 MB, `runtime-modules`), handed over as `Source { text: String }`
(`engine-api`); sharing one `Arc<str>` needs both crates, outside this ticket's files. What is left in
scope, building the line table on the first stack trace, is about 1.1 MB (0.5%).

Decision: the 180 MB target is closed, for the reasons below, and the single-source change (6.7 MB,
spanning `runtime-modules` and `engine-api`) is left for a future ticket.

Idle RSS stays at about 200 to 220 MB (footprint 202 MiB, 211.8 MB), not below 180 MB. Reaching 180 MB
needs about 30 MB; the three items together are bounded by 6.6 + 4.4 + 6.7 = 17.7 MB even if every one
were kept and the loader change were allowed. Most of the memory is the app's own state: `cells`,
`storage` and `strings` are 101 MB (the 50,000 cache objects and the module graph), which no change to
code, source or shapes reaches. The next candidates by size are the stub header (`FunctionCode` is the
same struct for a stub and a compiled body; a smaller stub type would save part of
its size on each of tens of thousands of functions) and the per-object storage.

## mimalloc as global allocator (VELYR-191)

Experiment: `mimalloc` 0.1.52 (MIT) as `#[global_allocator]` in the CLI binary, behind a default-on cargo
feature. It built under `unsafe_code = "forbid"` and `check-unsafe.sh` passed with no allowlist change.
Decision: **reverted** (criterion: peak RSS -15% or better, req/s loss at most 3%).

Velyr V8 serve, 10 s per round, 3 rounds, medians. Same fixture, both binaries built from the same tree
and run in the same session (before: A rounds first, after: B rounds second, not interleaved).

| Workers | Metric | Before (malloc) | After (mimalloc) | Delta |
|---|---|---|---|---|
| 1 | idle RSS (MB) | 106 | 138 | +30% |
| 1 | peak RSS (MB) | 367 | 362 | -1.2% |
| 1 | req/s | 3466 | 3683 | +6% (includes one 477 outlier) |
| 1 | graph load (ms) | 2532 | 2287 | -10% |
| 4 | idle RSS (MB) | 426 | 443 | +4% |
| 4 | peak RSS (MB) | 1170 | 1254 | +7% |
| 4 | req/s | 12292 | 13915 | +13% |
| 4 | p99 (ms) | 21.3 | 13.2 | -38% |

Per-round 1-min load, then peak MB / req/s:

- before w1: 7.4 (364 / 3466), 9.6 (367 / 3224), 6.9 (389 / 3557)
- after w1: 7.6 (351 / 477), 13.0 (362 / 3700), 8.2 (383 / 3683)
- before w4: 4.7 (1263 / 12292), 3.8 (1170 / 12749), 9.1 (1116 / 6690)
- after w4: 4.3 (1216 / 13406), 3.9 (1254 / 13915), 8.2 (1264 / 14188)

Reading: peak RSS did not move (-1% at 1 worker, +7% at 4), so the fragmentation is not the main source
of the 21 MB used vs 175 MB resident gap, or V8's own heap dominates peak. Throughput and p99 improved,
but the machine load was 4 to 13 during every round (limit was 4, later relaxed to 7 by the coordinator),
so those gains are within noise and are not a basis to keep it. Raw data:
`benches/results/footprint-large-mimalloc-{before,after}-{w1,w4}.json`.

## Idle and peak footprint of Velyr V8 (VELYR-212)

Method: one server run per reading, routes touched `workers * 2` times, 2 s pause, idle reading, then
`oha` on 3 routes (2 s warm-up, 6 s measured each) with RSS sampled every 250 ms, then the after-load
reading. Besides `ps` RSS (noisy on macOS under memory pressure: compressed pages leave RSS), every run
records the physical footprint from `proc_pid_rusage(RUSAGE_INFO_V4)` (current and lifetime max, summed
over the process tree) and the CPU time of the tree per request in the measured windows. Those two are
the stable columns; req/s is reported but the machine load was 5 to 30 during every run (other workers
building), so req/s differences under ~10% are noise. Velyr heap numbers come from `--metrics-addr`
(`velyr_heap_*{worker}`); Node numbers from a `--import` probe that dumps `process.memoryUsage()`,
`v8.getHeapStatistics()` and `v8.getHeapSpaceStatistics()` on SIGUSR2.

### Decomposition (V8 engine, before any change)

| | Velyr w1 | Node w1 | Velyr w4 | Node w4 |
|---|---|---|---|---|
| RSS idle (MB) | 134 | 95 to 182 | 408 | 780 (4 x ~178 + 59 primary) |
| footprint idle (MB) | 122 | 149 | 446 | 539 |
| footprint max (MB) | 317 | 335 | 1126 | 1121 |
| V8 heap used / total idle, per isolate (MB) | 68 / 80 | 80 / 109 | ~70 / ~79 | ~76 / ~110 |
| V8 heap used / total after load, per isolate (MB) | 94 / 139 | 90 / 158 | ~87 / ~139 | ~91 / ~151 |
| new space size idle / after load (MB) | n/a | 43 / 64 | n/a | 33 / 64 |
| dirty "App-Specific Tag 16" (V8 heap pages) idle / after (MB) | 58 / 76 | 48 / 98 | 289 / 311 | per worker |
| malloc allocated / fragmentation after load (MB) | 16 / 43 | 10 / 55 | 63 / 92 | per worker |
| page tables idle / after (MB) | 10 / 15 | 12 / 17 | 31 / 47 | per worker |

Reading: per isolate Velyr already holds about the same heap as Node. Under load the V8 heap total grows
by ~55 MB per isolate, almost all of it new space growing to its 64 MB cap (2 x 32 MB semi-spaces, the
default for this V8), and nothing shrinks it back: Velyr never pumps the V8 platform message loop, so
the foreground tasks the heap posts (memory reducer, idle-time GC) never run. The second item is the
malloc fragmentation after load (43 MB at w1, 92 MB at w4), already analysed in VELYR-191.

Gaps: Velyr's `process.memoryUsage()` reports `heapUsed`/`heapTotal` as 0 and
`v8.getHeapSpaceStatistics()` is not implemented, so the per-space split above exists only for Node;
Velyr workers are threads, so per-worker RSS cannot be separated (only per-worker heap via metrics).

### Candidates (A/B interleaved, 3 rounds unless noted, medians)

Flags were passed through a temporary `VELYR_V8_FLAGS` hook in `V8Engine::init_platform`; A and B use
the same binary. "fp" = physical footprint.

| Candidate | w | fp idle A / B | fp max A / B | peak RSS A / B | req/s A / B | CPU ms/req A / B | Verdict |
|---|---|---|---|---|---|---|---|
| `--max-semi-space-size=4` | 1 | 118.9 / 113.0 | 313.9 / 266.8 (-15%) | 331 / 289 (-13%) | 4396 / 3329 | 0.233 / 0.293 (+26%) | rejected: scavenge cost |
| `--max-semi-space-size=8` | 1 | 126.8 / 118.3 | 316.7 / 265.7 (-16%) | 332 / 276 (-17%) | 4313 / 4065 (-6%) | 0.237 / 0.252 (+6%) | rejected: CPU cost |
| `--max-semi-space-size=16` (n=4) | 1 | 126.8 / 116.6 (-8%) | 315.7 / 284.6 (-10%) | 333 / 298 (-10.5%) | 4230 / 4282 | 0.239 / 0.239 | kept |
| `--max-semi-space-size=16` | 4 | 441.8 / 457.0 | 1126.2 / 935.9 (-17%) | 1055 / 1005 (-5%) | 9961 / 10561 | 0.340 / 0.332 | kept (heap total after load 555 -> 426, -23%) |
| `--single-threaded-gc` (on top of semi 16) | 4 | 447.8 / 455.5 | 806.9 / 932.6 | 831 / 958 | 11932 / 12142 | 0.296 / 0.301 | rejected: no gain |
| `--no-opt` (on top of semi 16) | 1 | 120.2 / 117.9 | 282.6 / 276.7 (-2%) | 307 / 296 (-4%) | 4449 / 4466 | 0.232 / 0.225 | rejected: below 10% |
| `--lite-mode` (on top of semi 16, n=2) | 1 | 119.0 / 102.3 | 281.9 / 156.7 (-44%) | 305 / 151 (-50%) | 4473 / 1870 (-58%) | 0.231 / 0.642 | rejected: throughput |
| drop `SourceCache` + `ModuleHost` cache after entry evaluation | 1 | 117.9 / 117.4 | 316.8 / 316.3 | 339 / 339 | 4417 / 4428 | 0.233 / 0.233 | rejected: no gain |
| same | 4 | 442.6 / 437.8 (-1%) | 1025.6 / 1072.4 | 936 / 1137 | 13787 / 13590 | 0.284 / 0.289 | rejected: no gain |
| `low_memory_notification` after graph load | 1 | 118.0 / 131.5 (+11%) | 317.3 / 322.0 | 339 / 341 | 4439 / 4388 | 0.232 / 0.233 | rejected: worse idle |
| same | 4 | 445.1 / 476.8 (+7%) | 1158.4 / 986.8 | 1127 / 1046 | 13720 / 13325 | 0.287 / 0.291 | rejected: worse idle |
| pump platform message loop on every microtask checkpoint | 1 | 121.1 / 119.0 | 318.6 / 307.4 (-4%) | 333 / 323 | 4278 / 3740 | 0.240 / 0.262 (+9%) | open, see below |
| same | 4 | 439.2 / 448.0 | 1136.7 / 902.6 (-21%) | 1080 / 766 (-29%) | 11267 / 9750 | 0.302 / 0.341 (+13%) | open, see below |

Notes:

- Sources: the large-app graph is 2046 files, 7.2 MB, so even freeing every Rust-side copy caps at ~6%
  of the idle footprint; V8 keeps its own copy (lazy compilation and `Function.prototype.toString` need
  it) and the freed blocks are small malloc allocations that stay resident. Measured: no change.
- A full GC right after the graph loads made idle worse: it runs before the first requests warm the
  heap, and the heap grows back with a larger initial size.
- Pumping the platform loop is the only lever on the after-load heap besides the semi-space cap
  (w4 peak -29%, fp max -21%), but pumping on every checkpoint cost 9 to 13% CPU per request in this
  run. It needs a throttled pump (time-based) and an idle trigger, which belongs to
  `runtime-event-loop` (outside VELYR-212's files); worth its own ticket.

## Idle-time GC in the event loop (VELYR-215)

Same method as VELYR-212, plus a "settled" reading 3 s after the load ends (the server back to idle
after a burst). Baseline already has `--max-semi-space-size=16`. Candidates went through temporary
env-gated hooks in one binary (A and B identical code); the kept one was then measured as the final
build against the previous one. Machine load 4 to 25 (1-min average, logged per run).

| Candidate | w | fp idle A / B | fp settled A / B | fp max A / B | CPU ms/req A / B | req/s A / B | Verdict |
|---|---|---|---|---|---|---|---|
| pump the platform loop (budget 1 ms) each time the loop is about to block | 4 | 421.8 / 457.3 (+8%) | n/a | 942.6 / 876.1 (-7%) | 0.326 / 0.309 | 11463 / 11750 | rejected: below 10% |
| same plus `run_idle_tasks` (platform with idle-task support, 1 ms deadline) | 4 | 440.7 / 451.8 | 733.3 / 788.6 | 809.3 / 924.3 | 0.275 / 0.268 | 14695 / 13899 | rejected: no gain |
| `low_memory_notification` after 1 s of continuous idleness (no growth guard) | 1 | 117.2 / 103.6 (-12%) | 169.6 / 122.4 (-28%) | 285.0 / 280.6 | 0.256 / 0.259 (+1.2%) | 3947 / 3915 | candidate |
| same | 4 | 440.5 / 376.1 (-15%) | 590.7 / 462.5 (-22%) | 990.4 / 829.0 | 0.335 / 0.336 (+0.3%) | 10280 / 9823 | candidate |
| **final build**: idle GC after 1 s, once per idle period, only if used heap grew 4 MB since the last one | 1 | 126.7 / 105.6 (-17%) | 226.2 / 177.7 (-21%) | 281.9 / 281.5 | 0.233 / 0.236 (+1.3%) | 4430 / 4405 | kept |
| same | 4 | 458.9 / 383.3 (-16%) | 579.8 / 459.6 (-21%) | 952.4 / 968.2 (+2%) | 0.266 / 0.269 (+1.1%) | 15324 / 15307 | kept |

What was kept (`crates/runtime-event-loop/src/lib.rs`): while `EventLoop::run` waits for work, a
1 s sleep joins the `select!`; when it wins, the loop calls `request_gc(GcReason::Idle)` (V8:
`low_memory_notification`, a full GC that also shrinks the young generation back) at most once per
idle period. The GC is skipped unless the used heap grew 4 MB since the previous idle GC, so a
server with sparse traffic (health checks) does not pay a full GC per request. Under load the loop
never stays idle for a second, so the timer never fires there.

Notes:

- The pump alone does little once the semi-space is capped: the -21% max footprint VELYR-212 saw
  was mostly the young-generation growth that VELYR-215's baseline no longer has.
- `ps` peak RSS rose ~8% with the final build (w1 307 -> 333 MB, w4 957 -> 1036 MB, consistent over
  the 3 rounds) while the max physical footprint stayed flat. Pages the idle GC returned are faulted
  back in during the next burst and are counted as resident again, while the baseline's idle pages
  had been compressed out of RSS; the footprint, which counts compressed pages, is the reliable peak.
- Gap: `velyr_heap_*` gauges are recorded at the end of JS turns (`LoopIsolate::turn` in
  runtime-core), so after an idle GC `/metrics` keeps the pre-GC heap until the next request. A
  one-line `record_heap` in `LoopIsolate::request_gc` would fix it (outside VELYR-215's files).
- Not tried: `MemoryPressureLevel` notifications (rusty_v8 exposes `memory_pressure_notification`;
  `low_memory_notification` already is the critical-pressure path and met the target).
