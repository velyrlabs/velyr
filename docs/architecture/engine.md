# Engine architecture

## Responsibilities of the engine

The engine owns:
- ECMAScript parsing/compilation/execution;
- language values;
- objects/prototypes;
- lexical environments;
- function calls;
- exceptions;
- Promise semantics;
- microtask queue;
- modules at the ECMAScript semantic level;
- garbage collection;
- optional JIT tiers.

The engine does NOT own:
- filesystem;
- sockets;
- DNS;
- subprocesses;
- package installation;
- runtime permission policy;
- HTTP implementation.

## Native engine stages

```text
source → tokens → AST → bytecode → VM
                         │
                         └→ profile → JIT (future)
```

## Differential testing

Until native semantics mature, test programs may be evaluated in:
- Velyr native engine;
- a reference engine adapter;
- other engines in offline test harnesses.

Differences become minimized reproducible fixtures.

## Test262

Test262 is a first-class CI asset. Results should be stored by:
- test path;
- feature;
- expected status;
- actual status;
- runtime version.

Known failures must not be hidden behind a single percentage.
