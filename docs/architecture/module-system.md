# Module system

## Native format
ESM.

## Resolver pipeline

```text
specifier
→ classify
→ normalize parent
→ package scope
→ exports/imports conditions
→ permission
→ locate
→ read/fetch
→ transform
→ compile
→ cache
```

## Resolver strategies

- relative/absolute file
- file URL
- package
- `node:` when Node compatibility exists
- future explicit remote schemes
- embedder-defined virtual modules

## Cache layers

1. source cache
2. transform cache
3. parse/compile cache
4. instantiated module state (per isolate)

Only immutable layers are candidates for cross-isolate sharing.

## Security

Resolution is not authorization. A successfully resolved path still requires read/import authority before content is loaded.
