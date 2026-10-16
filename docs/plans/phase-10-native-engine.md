# Phase 10 — Native engine maturity

## Objective
Make Velyr's Rust engine production-capable.

## Workstreams
1. parser/conformance;
2. value model;
3. object model;
4. bytecode;
5. VM;
6. promises/jobs;
7. modules;
8. builtins;
9. typed arrays;
10. RegExp;
11. GC;
12. Intl;
13. performance;
14. JIT research.

## Continuous gate
Test262 results stored per commit/release.

## JIT rule
No optimizing JIT work is allowed to block semantic milestones unless profiling demonstrates VM execution as the current product bottleneck.
