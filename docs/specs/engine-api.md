# SPEC — Engine API

## Goals
- concrete-engine independence;
- no V8-specific public types;
- explicit lifetime/handle semantics;
- isolate ownership;
- efficient host bridging.

## Required capabilities

### Lifecycle
```text
create engine
create isolate/context/realm
bootstrap globals
destroy isolate
```

### Execution
```text
evaluate script
compile module
instantiate module
evaluate module
call function
construct object
```

### Values
Host must be able to represent/inspect:
- undefined/null;
- boolean;
- number;
- bigint;
- string;
- symbol where needed;
- object;
- function;
- ArrayBuffer/typed arrays;
- Promise/error handles.

### Async
- create promise resolver;
- resolve/reject promise;
- drain microtasks;
- enqueue engine job only from owner isolate.

### Diagnostics
- exception;
- stack trace;
- heap stats;
- GC trigger hint.

## Ownership rule

Engine handles are isolate-scoped unless explicitly documented as thread-safe immutable artifacts.

## Forbidden pattern

```rust
pub type Value = v8::Local<'a, v8::Value>;
```

in `runtime-core`.

Concrete adapter types stay inside adapter crates.
