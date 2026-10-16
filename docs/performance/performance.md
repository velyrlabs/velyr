# Performance architecture

## Optimization target

Primary:

```text
useful_throughput / (cpu_time × rss)
```

This avoids optimizing raw throughput while ignoring memory/cost.

## Critical paths

1. JS ↔ Rust host calls
2. allocation/GC
3. Promise resolution/microtasks
4. network buffer movement
5. HTTP parse/write
6. module loading
7. worker startup
8. package resolution
9. string conversion
10. structured cloning

## Zero-copy principles

For binary I/O:
- use ArrayBuffer/typed-array backing stores;
- avoid JSON as an internal FFI boundary;
- define ownership/borrowing rules explicitly;
- copy when needed for safety, not reflexively.

## Fast paths

Candidates:
- ASCII/Latin-1 strings;
- small integers;
- object shapes/hidden classes;
- common property lookups;
- cached module resolution;
- pooled network buffers;
- immutable compiled artifact sharing.

Fast paths require a correct slow path and differential tests.

## Performance is gated

A performance optimization that:
- weakens permissions,
- changes semantics,
- creates unbounded memory,
- increases unsafe surface without justification

is rejected.
