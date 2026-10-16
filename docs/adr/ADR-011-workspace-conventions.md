# ADR-011 — Workspace, crate, error and tooling conventions

**Status:** Accepted (2026-10-15)
**Relates to:** ADR-001, SPEC §11, PLANS §4

## Decision

### Workspace
- A virtual workspace at the repository root: `members = ["crates/*"]`, `resolver = "3"`, `edition = "2024"`. Adding a crate never requires editing the root `Cargo.toml`.
- `rust-toolchain.toml` pins the exact stable toolchain installed at Phase 00. MSRV is that version. Toolchain bumps get their own ticket.
- Shared dependencies live in `[workspace.dependencies]`, declared in Phase 00: `tokio`, `tokio-util`, `thiserror`, `tracing`, `tracing-subscriber`, `serde`, `serde_json`, `clap`, `criterion`, `v8`. Any other dependency is declared in the consuming crate with an explicit version. Promotion to a workspace dependency is a separate ticket, to avoid contention on the root manifest.
- `Cargo.lock` is committed.
- License: `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`). Adapted third-party code is attributed in `THIRD_PARTY_NOTICES.md`.

### Crates
- Directory names follow TDD §3 plus `engine-v8`. The package name is `velyr-<dir>` (for example `crates/engine-api` becomes `velyr-engine-api`). The CLI binary is `velyr`, from `crates/cli`.
- `[workspace.lints.rust] unsafe_code = "forbid"` applies to every crate. Only the crates listed in `unsafe-allowlist.txt` (`engine-v8`, `engine-gc`, `engine-vm`, `compat-napi`) opt out. They opt out with a crate-level `[lints]` override, and every `unsafe` block carries a `// SAFETY:` comment.
- `runtime-core` must never depend on `web-*`, `compat-*`, `tooling-*` or `io-*`. CI enforces this with `scripts/ci/check-deps.sh`.
- Build profiles are Cargo features of `crates/cli`: `profile-minimal`, `profile-web`, `profile-node`, `profile-full`. The default is `profile-full`.

### Errors
- The leaf crate `runtime-errors` defines `ErrorCode` and `VelyrError { code, message, cause, context, span_id }`. Fields in `context` are redacted by default.
- Stable codes: `VELYR_PERMISSION_DENIED`, `VELYR_QUOTA_EXCEEDED`, `VELYR_NOT_FOUND`, `VELYR_ALREADY_EXISTS`, `VELYR_INVALID_DATA`, `VELYR_NETWORK`, `VELYR_TLS`, `VELYR_TIMEOUT`, `VELYR_CANCELLED`, `VELYR_UNSUPPORTED`, `VELYR_BAD_RESOURCE`, `VELYR_INTERNAL`. Adding a code is fine; renaming one is a breaking change.

### Telemetry
- Crates use `tracing` macros. Span names follow `velyr.<crate>.<operation>`.
- The subscriber is set up only in `crates/cli`, filtered by `VELYR_LOG`, with `pretty` or `json` output.
- Permission denials emit `velyr.permission.denied`; quota denials emit `velyr.quota.denied`. Secrets and env values are never recorded as fields.

### Tests, benchmarks, fuzzing, CI
- Unit tests live in-crate. Integration tests go in `crates/<c>/tests/`. End-to-end tests that run the `velyr` binary go in `crates/cli/tests/`. JS fixtures go in `tests/fixtures/<area>/`.
- `crates/bench-harness` (binary `velyr-bench`) runs the suites defined in `benches/suites/*.toml`. It writes PLANS §8 JSON, validated against `benches/schema/result.schema.json`, into `benches/results/` (gitignored). Committed baselines live in `benches/baselines/`. Criterion microbenchmarks live in each crate's `benches/`.
- `fuzz/` is a `cargo-fuzz` workspace (nightly), excluded from the main workspace.
- CI runs on GitHub Actions (`.github/workflows/`) with an ubuntu/macos/windows matrix and the merge gates from PLANS §4.
