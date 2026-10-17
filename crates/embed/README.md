# velyr

Embed the Velyr JavaScript runtime in a Rust host: permissions, quotas, host ops,
`run_module` and `call(export, args)`.

```rust
use velyr::{Capability, PathPattern, Permissions, Runtime};

let permissions = Permissions::new().allow(Capability::ImportLocal(PathPattern::new("/srv/app")));
let mut runtime = Runtime::builder().permissions(permissions).build()?;
runtime.run_module("/srv/app/main.mjs")?;
let total = runtime.call("sum", &[1.into(), 2.into()])?;
```

## Limits (0.x)

- Each `call` starts a new isolate and re-runs the module's top level; there is no state
  between calls.
- `cpu_budget` covers only the synchronous stretch of execution.
- Only the `Core` and `Minimal` profiles exist.
- Only the V8 engine is supported.
- Host op authorization is manual: the callback must call `authorize` itself.
- The version is 0.x: no semver guarantee.
