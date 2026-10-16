# ADR-005 — Tokio as initial host async runtime

**Status:** Accepted for initial implementation

## Decision
Use Tokio to execute host async operations.

## Clarification
Tokio is not defined as the JavaScript event loop. The Velyr scheduler owns JS-visible ordering.

## Revisit
If profiling identifies Tokio as a material bottleneck or deployment blocker.
