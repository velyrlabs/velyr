# Runtime Core

The Runtime Core orchestrates execution while remaining independent of Node compatibility.

## Components

### Runtime
Owns process-level configuration and supervisor state.

### Isolate
Owns one mutable engine instance/realm set and its event-loop state.

### Op registry
Maps host operation identifiers to Rust implementations.

### Permission context
Immutable/effectively immutable capability grants.

### Quota context
Accounts resource budgets.

### Resource table
Owns native resources visible to JS.

### Completion queue
Returns async native results to the isolate.

### Module loader
Resolves, loads, transforms, caches and compiles modules.

## Core lifecycle

```text
create runtime
  ↓
parse config/policy
  ↓
create isolate
  ↓
bootstrap builtins
  ↓
load entry module
  ↓
evaluate
  ↓
event loop
  ↓
graceful/forced shutdown
  ↓
close resources
```

## Shutdown

Graceful shutdown:
1. stop accepting new work;
2. notify workers;
3. cancel cancellable operations;
4. drain within deadline;
5. close resources;
6. flush telemetry;
7. destroy engine state.

Hard shutdown MUST have bounded completion.
