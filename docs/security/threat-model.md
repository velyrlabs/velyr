# Threat model

## Protected assets

- host filesystem;
- network identity and connectivity;
- environment/secrets;
- subprocess execution;
- runtime process integrity;
- other isolates/tenants;
- package cache integrity;
- lockfile integrity;
- native memory;
- credentials used by package registries;
- observability data.

## Adversaries

1. malicious npm dependency;
2. compromised transitive dependency;
3. malicious input triggering parser/VM bug;
4. untrusted user script;
5. hostile network peer;
6. malicious native addon;
7. supply-chain package substitution;
8. denial-of-service workload;
9. cross-isolate attacker.

## Primary trust boundaries

```text
JS code
  │
  ▼
Engine
  │
  ▼
Host-op boundary  ← capability enforcement
  │
  ▼
Rust host
  │
  ▼
OS/network
```

Native addons cross a stronger boundary because they execute native code in the process.

## STRIDE-style risks

### Spoofing
- package registry/source confusion;
- module identity confusion.

Controls:
- canonical module IDs;
- lockfile;
- integrity verification.

### Tampering
- package cache modification;
- bytecode/cache poisoning.

Controls:
- content hashes;
- cache metadata versioning;
- atomic writes.

### Repudiation
- privileged actions without audit context.

Controls:
- structured permission/host-op events.

### Information disclosure
- filesystem/env/network access;
- heap/inspector data.

Controls:
- deny by default;
- inspector opt-in;
- secret redaction.

### Denial of service
- unbounded heap;
- infinite CPU;
- decompression bombs;
- sockets/files/workers exhaustion.

Controls:
- resource quotas;
- time budgets;
- request/body limits;
- worker limits.

### Elevation of privilege
- worker expanding permissions;
- FFI bypassing capabilities;
- host-op bug.

Controls:
- permission intersection;
- no FFI by default;
- centralized host boundary.

## Non-sandbox guarantee

Memory-safe Rust and permissions do not automatically make Velyr a hardened arbitrary-code sandbox. Any claim of hostile multi-tenant isolation must require a dedicated security review and likely OS/container/VM boundaries in addition to runtime capabilities.
