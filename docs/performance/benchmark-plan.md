# Benchmark plan

## Competitors/reference baselines

Where useful:
- Node.js current supported release;
- Deno;
- Bun;
- Velyr reference engine build (`engine-v8`, ADR-009);
- Velyr native engine build.

Comparisons must disclose versions and configuration.

## Suites

### Startup
```text
empty process
hello world
ESM graph 10/100/1000 modules
```

### Memory
```text
idle
simple HTTP
N workers
large module graph
WebSocket connections
```

### HTTP
```text
plaintext
JSON
small body
large streaming body
TLS
HTTP/1
HTTP/2
```

### Async
```text
timers
Promise chains
file read concurrency
TCP echo
fetch fanout
```

### Engine
```text
language microbenchmarks
object/property
arrays
closures
async
JSON
regexp
```

Engine microbenchmarks are diagnostic, not the product score.

### Multicore
1, 2, 4, 8, 16, 32 workers/cores where hardware permits.

## Metrics
- req/s;
- operations/s;
- CPU seconds;
- RSS;
- peak RSS;
- heap;
- P50/P95/P99;
- context switches;
- allocations where measurable;
- GC pauses;
- startup wall time.

## Regression budgets

Initial proposal:
- throughput: no >3% regression on stable benchmark without approval;
- P99: no >5%;
- RSS: no >3%;
- startup: no >5%.

Noise thresholds must be calibrated per runner before enforcing.
