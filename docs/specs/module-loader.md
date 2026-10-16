# SPEC — Module Loader

## Interfaces

Conceptual:

```rust
trait ModuleResolver {
    fn resolve(&self, specifier: &str, referrer: &ModuleId, ctx: &ResolveContext)
        -> Result<ResolvedModule>;
}

trait ModuleLoader {
    async fn load(&self, module: &ResolvedModule, ctx: &LoadContext)
        -> Result<ModuleSource>;
}
```

## Resolution is pure-ish

Resolver SHOULD avoid performing network I/O. Loading performs I/O.

## Normalization

Every module receives a canonical ID. Two spellings referring to the same file/package entry should converge according to resolver semantics.

## Transforms

Transform cache key includes:
- source content hash;
- transform implementation version;
- configuration;
- target;
- relevant package metadata.

## Permissions

Read/import permission is evaluated before untrusted content is loaded from its source.
