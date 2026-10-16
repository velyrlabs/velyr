# Phase 03 — Security and modules

## Objective
Establish authority and ESM before broad system APIs.

## Tasks
- CapabilitySet;
- path/network/env permission types;
- CLI permission parser;
- deny precedence;
- ResourceTable with generation-safe handles;
- quotas;
- ESM resolver/loader;
- canonical module IDs;
- source cache;
- import permission;
- leak diagnostics;
- symlink/path bypass tests.

## Acceptance
Default script cannot read a known local fixture unless explicitly granted.
