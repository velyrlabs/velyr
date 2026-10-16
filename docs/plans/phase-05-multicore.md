# Phase 05 — Multicore

## Objective
Scale across cores while keeping per-isolate heap semantics.

## Tasks
- Worker lifecycle;
- supervisor;
- structured clone;
- message channels;
- permission intersection;
- worker quota;
- shared immutable cache experiments;
- listener distribution experiments;
- auto worker heuristic;
- graceful multicore shutdown.

## Acceptance
1/2/4/8-worker benchmark demonstrates scaling characteristics and records RSS per worker.
