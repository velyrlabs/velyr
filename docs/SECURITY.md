# Velyr Security Program

## Security objective

A normal Velyr application should receive only the host authority it explicitly needs.

## Required repository controls

- `SECURITY.md` at repository root before public beta;
- private disclosure mechanism;
- security advisories process;
- dependency advisory automation;
- fuzzing of parser, bytecode, resolver and protocol boundaries;
- explicit unsafe-code inventory;
- release threat review;
- no secrets in CI logs;
- signed/provenance release plan before stable.

## Severity examples

### Critical
- arbitrary native code execution from ordinary JS without granted native/process authority;
- runtime permission bypass exposing arbitrary filesystem/environment/network;
- cross-isolate memory corruption.

### High
- scoped permission escape;
- crafted package/source causing reliable memory corruption;
- inspector remotely exposed without authorization.

### Medium/Low
Assessed by exploitability, authority gained, confidentiality/integrity impact and deployment prerequisites.
