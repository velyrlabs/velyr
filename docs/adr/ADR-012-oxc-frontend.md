# ADR-012 — Oxc for TypeScript/JSX transforms and as the native engine front-end

**Status:** Accepted (2026-10-15)
**Relates to:** TDD §12, §15, ADR-009

## Decision

1. `tooling-typescript` and `tooling-jsx` use the Oxc crates (`oxc_parser`, `oxc_transformer`, `oxc_codegen`, `oxc_sourcemap`). TypeScript is handled by stripping types only. Type checking is not part of the runtime.
2. JSX defaults to the automatic runtime with `jsxImportSource = "react"`. It can be configured in `velyr.json` (`compilerOptions.jsx`, `compilerOptions.jsxImportSource`) or read from `tsconfig.json`.
3. The transform cache key follows `specs/module-loader.md`: content hash, Oxc version, configuration, target and package metadata.
4. `engine-native` uses `oxc_parser` and the Oxc AST as its front-end. Lexer and parser conformance comes from Oxc, which targets the Test262 parser tests. Velyr owns the bytecode compiler, VM, object model and GC.
5. Every Oxc crate is pinned to the same exact version across the workspace.

## Consequences

- Track C starts at the bytecode compiler instead of the lexer. The lexer, parser and AST stages in ROADMAP M10 are satisfied through Oxc and verified by Test262 parse tests.
- Oxc AST changes become an upgrade cost for `engine-native`. Oxc upgrades get their own tickets.
