# Web standards compatibility

## Baseline

The Minimum Common Web API standard is Velyr's server Web interoperability target.

Target areas include:
- AbortController/AbortSignal;
- Event/EventTarget;
- MessageChannel/MessagePort where applicable;
- URL/URLSearchParams/URLPattern;
- TextEncoder/TextDecoder;
- Blob/File/FormData;
- Headers/Request/Response;
- fetch;
- ReadableStream/WritableStream/TransformStream;
- crypto/SubtleCrypto;
- performance;
- structuredClone;
- WebAssembly.

## Web Platform Tests

Applicable WPT tests should be run through a Velyr adapter.

Not every browser test is meaningful for a server runtime. The selected test manifest must be explicit and versioned.

## Extension policy

Do not add convenience methods to standard prototypes when a Velyr namespace can expose the feature.

Preferred:

```js
Velyr.permissions.query(...)
```

Avoid:

```js
Request.prototype.velyrMagic = ...
```

## Network security

`fetch` uses runtime connect/import permissions. Standard API shape does not bypass capability checks.
