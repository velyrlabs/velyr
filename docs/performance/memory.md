# Memory strategy

## Memory classes

Track separately:
- executable/code pages;
- Rust heap;
- JS heap;
- module source;
- bytecode/JIT code;
- network buffers;
- package caches;
- worker stacks;
- OS/TLS state.

## Per-isolate reduction

Do not duplicate immutable artifacts unnecessarily.

Candidate shared read-only assets:
- source blobs;
- parse metadata;
- bytecode if engine semantics permit;
- standard library compiled artifacts;
- package metadata.

## JS values

Representation research:
- tagged values / NaN boxing where safe and beneficial;
- compact small integers;
- string representation tiers;
- object shapes;
- inline small storage.

These are engine decisions and require benchmarks.

## GC

First objective: predictable correctness.

Later optimize:
- nursery/generational behavior;
- incremental marking;
- compaction;
- allocation arenas.

## OOM behavior

Hard memory budget must produce controlled isolate termination/error where possible rather than uncontrolled host exhaustion.

Multi-tenant/serverless deployments should still apply OS/cgroup limits.
