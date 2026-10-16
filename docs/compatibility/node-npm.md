# Node and npm compatibility

## Philosophy

Node compatibility is an adoption layer, not the runtime's architectural center.

## Module priorities

### Tier 1
- `node:path`
- `node:url`
- `node:events`
- `node:assert`
- `node:util`
- `Buffer`

### Tier 2
- `node:fs`
- `node:stream`
- `node:crypto`
- `node:os`

### Tier 3
- `node:http`
- `node:https`
- `node:net`
- `node:tls`
- `node:dns`

### Tier 4
- `node:child_process`
- `node:worker_threads`
- advanced diagnostics

## ESM/CommonJS

ESM remains canonical. CommonJS loader is an optional compatibility module.

Interop requires explicit tests for:
- default/named import behavior;
- cyclic modules;
- `require` cache;
- JSON;
- package type boundaries;
- dynamic import from CommonJS;
- ESM import of CommonJS.

## package.json

Resolver support includes:
- `type`;
- `main`;
- `exports`;
- `imports`;
- subpath patterns;
- conditional exports;
- package self-reference.

## npm installation

Velyr may use npm registry protocols without reproducing the npm CLI implementation.

## Native packages

Node-API is targeted because it is designed to be independent of the underlying JS engine and ABI-stable across Node versions. Direct V8 C++ addon compatibility is not a core target.

## Compatibility reporting

Per module/package:
```text
unsupported
experimental
partial
supported
```

Each status links to executable evidence.
