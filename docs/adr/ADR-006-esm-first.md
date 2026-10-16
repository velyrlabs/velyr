# ADR-006 — ESM first

**Status:** Accepted

## Decision
ES modules are canonical. CommonJS exists only in a compatibility layer.

## Consequences
New runtime APIs and examples use ESM. Resolver design is not constrained by `require` internals.
