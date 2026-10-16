# Phase 01 — Engine bridge

## Objective
Execute JS and call Rust without committing runtime architecture to one engine.

## Tasks
- finalize Engine trait;
- implement `engine-v8` adapter (ADR-009);
- implement `engine-native` bootstrap (minimal subset) as second implementation;
- engine-api conformance suite run against both adapters;
- record footprint baseline (binary size, cold startup, idle RSS per isolate) for `minimal` and `full`;
- value conversion;
- host function registration;
- console;
- exception/stack transport;
- isolate lifecycle;
- engine metrics interface;
- differential fixture runner.

## Acceptance program

```js
console.log(nativeAdd(20, 22));
```

Expected output: `42`.

## Security
Engine has zero direct host filesystem/network authority.
