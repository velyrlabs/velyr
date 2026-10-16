# ADR-008 — WebAssembly Components for first-class plugins

**Status:** Accepted as preferred extension direction

## Decision
New Velyr plugin interfaces should prefer WebAssembly Components/WIT over native dynamic libraries.

## Consequences
Capabilities and memory/execution budgets can be explicit. Node native addons remain a compatibility exception.
