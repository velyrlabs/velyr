# Phase 02 — Async kernel

## Objective
Prove the central async path.

## Tasks
- Tokio host runtime;
- Promise resolver handles;
- completion queue;
- isolate wake mechanism;
- timers;
- cancellation token;
- event-loop deterministic tests;
- pending-op liveness;
- late-completion-after-termination tests;
- event-loop lag metric.

## Acceptance

```js
console.log("a");
const value = await nativeAsync();
console.log(value);
```

Completion must re-enter the owning isolate safely and respect microtask order.
