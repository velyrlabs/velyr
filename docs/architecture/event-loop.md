# Event loop

## Goals
- preserve ECMAScript semantics;
- integrate Rust async without cross-thread VM mutation;
- maintain fairness;
- support deterministic testing;
- surface event-loop lag.

## Queues

Conceptual queues:
- engine microtasks;
- timer-ready tasks;
- completed host operations;
- worker messages;
- control messages.

## Scheduling

A loop iteration:

```text
1. run synchronous continuation
2. drain required microtasks
3. process bounded completion batch
4. process timers/messages
5. drain microtasks created by callbacks
6. evaluate liveness
7. await native readiness if idle
```

Exact ordering is finalized by semantic and compatibility tests.

## Fairness

Completion processing MUST be bounded to prevent a high-volume native source from permanently starving timers/messages.

## Cancellation

Every async op SHOULD carry a cancellation token. Isolate termination MUST make outstanding completion delivery safe even when the native task finishes afterward.
