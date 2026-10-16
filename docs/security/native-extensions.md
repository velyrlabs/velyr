# Native extensions security

## Native code classification

### Safe/normal
Rust runtime crates compiled as part of Velyr and governed by repository review.

### Restricted
Node-API addons, FFI dynamic libraries, arbitrary native plugins.

Restricted native code can:
- access process memory;
- bypass runtime permissions;
- crash the process;
- call OS APIs directly.

Therefore N-API compatibility does NOT imply sandbox safety.

## Policy

Native addon loading:
- off by default;
- requires explicit permission;
- audited in telemetry;
- MAY be disabled entirely by deployment profile.

## Future containment

Investigate an out-of-process native addon host:

```text
Velyr
  │ IPC
  ▼
Native Addon Host
```

Benefits:
- crash isolation;
- OS sandbox policy;
- narrower host API.

Costs:
- serialization;
- incompatibility with addons expecting in-process engine pointers;
- latency.

## Velyr plugin recommendation

New plugins should prefer WebAssembly Components with WIT-defined interfaces and injected capabilities.
