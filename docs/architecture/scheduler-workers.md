# Scheduler and workers

## Model

Velyr uses independent isolates for parallel JS execution.

```text
Supervisor
   │
   ├── Isolate/Worker A
   ├── Isolate/Worker B
   └── Isolate/Worker N
```

## Server mode

Possible distribution strategies:
- shared listener + OS reuse mechanisms;
- supervisor accept + handoff;
- request-level dispatch above transport.

Each strategy must be benchmarked by OS and protocol.

## Auto scaling

A `--workers=auto` mode may choose a count from:
- available CPU;
- configured memory budget;
- expected heap/worker;
- container CPU quota.

It must not blindly equal logical CPU count.

## Shared state

Default: no shared mutable JS state.

Preferred sharing:
- immutable compiled artifacts;
- source/cache blobs;
- read-only package metadata.

Explicit shared memory is a separate feature with strict semantics.
