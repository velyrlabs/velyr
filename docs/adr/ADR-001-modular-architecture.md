# ADR-001 — Modular architecture

**Status:** Accepted

## Decision
Velyr is implemented as a workspace of focused crates. Node, npm, TLS, N-API, frontend tooling and optional protocols are not dependencies of the minimum kernel.

## Consequences
Positive: smaller builds, explicit trust surface, independent evolution.  
Negative: more interface design and cross-crate version discipline.
