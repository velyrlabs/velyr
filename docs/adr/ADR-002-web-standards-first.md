# ADR-002 — Web standards first

**Status:** Accepted

## Decision
Portable Web APIs are implemented before Node-specific equivalents where a standardized server-appropriate API exists.

## Consequences
Node remains an optional compatibility surface; React/Vue SSR can target portable streams/request/response primitives.
