# Velyr documentation

This directory is the canonical design source for Velyr Core.

## Document hierarchy

### Product
- [PRD](PRD.md)
- [ROADMAP](ROADMAP.md)
- [PLANS](PLANS.md)

### Technical authority
- [TDD](TDD.md)
- [SPEC](SPEC.md)
- [Architecture overview](architecture/overview.md)

### Security
- [Threat model](security/threat-model.md)
- [Permissions and capabilities](security/permissions-capabilities.md)
- [Supply-chain security](security/supply-chain.md)
- [Native extensions](security/native-extensions.md)

### Compatibility
- [ECMAScript and Test262](compatibility/ecmascript-test262.md)
- [Web standards](compatibility/web-standards.md)
- [Node and npm](compatibility/node-npm.md)
- [Frontend ecosystem](compatibility/frontend.md)

### Performance
- [Performance architecture](performance/performance.md)
- [Benchmark plan](performance/benchmark-plan.md)
- [Memory strategy](performance/memory.md)

### Specifications
- [Engine API](specs/engine-api.md)
- [Host operations](specs/host-ops.md)
- [Module loader](specs/module-loader.md)
- [Permissions API](specs/permissions-api.md)
- [Worker API](specs/worker-api.md)
- [Plugin API](specs/plugin-api.md)

### Architecture Decision Records
See [`adr/`](adr/).

## Authority order

When documentation conflicts, use this precedence:

1. Security invariants in `security/`
2. Normative requirements in `SPEC.md`
3. Approved ADRs
4. TDD
5. PRD
6. Roadmap and phase plans

A change that breaks a security invariant or a normative specification MUST be accompanied by an explicit ADR.
