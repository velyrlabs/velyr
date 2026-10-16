# ADR-004 — Engine abstraction

**Status:** Accepted

## Decision
Runtime Kernel consumes `engine-api` and does not expose concrete engine handles in core interfaces.

## Consequences
Native engine development can proceed independently and differential/reference engines remain possible.
