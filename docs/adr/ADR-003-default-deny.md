# ADR-003 — Default-deny authority

**Status:** Accepted

## Decision
Filesystem, network, environment, process, FFI/native and inspector capabilities are denied by default.

## Consequences
Existing Node applications may require explicit grants. Security-sensitive deployments receive a meaningful least-authority model.
