# Contributing to Velyr

Thanks for helping. This project is in alpha with one maintainer, so reviews can be slow. Small,
focused pull requests move fastest.

## Setup

Install [rustup](https://rustup.rs). The toolchain version is pinned in `rust-toolchain.toml` and
installed automatically on the first `cargo` call.

```sh
git clone https://github.com/velyrlabs/velyr
cd velyr
cargo build -p velyr-cli
```

The binary is `velyr`, built from `crates/cli`.

## Running tests

Run the tests of the crate you changed, not the whole workspace:

```sh
cargo test -p velyr-<crate-dir>     # crates/engine-api -> velyr-engine-api
```

Test262 (ECMAScript conformance) uses its own runner. CI does this, in
`.github/workflows/test262-native.yml`:

```sh
bash scripts/fetch-test262.sh
cargo build --release -p velyr-qa-test262
target/release/velyr-test262 --engine native --threads 4 test
```

See `docs/compatibility/ecmascript-test262.md` for details and `scripts/update-expectations.sh`
for refreshing expectations.

## What CI runs on a pull request

From `.github/workflows/ci.yml`, on Linux, macOS and Windows unless noted:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `velyr-cli` builds with `profile-minimal` and `profile-full`
- Linux only: `scripts/ci/check-deps.sh`, `scripts/ci/check-unsafe.sh`, `scripts/ci/tests/run.sh`
- a Linux job for the `SO_REUSEPORT` multicore tests

Run `cargo fmt` and clippy on your crate before pushing.

## Code rules

- `unsafe` is forbidden workspace-wide. Only the crates in `unsafe-allowlist.txt` opt out, and every
  `unsafe` block needs a `// SAFETY:` comment. Do not add a crate to the list without an ADR.
- `runtime-core` must not depend on `web-*`, `compat-*`, `tooling-*` or `io-*` (`check-deps.sh`).
- Privileged host operations go through permission evaluation. Default deny is a design
  invariant (see `docs/adr/ADR-003-default-deny.md`).
- Errors use the stable `VELYR_*` codes from `runtime-errors`. Do not rename an existing code.
- New dependencies must pass `cargo deny` (`deny.toml`).
- Conventions in full: `docs/adr/ADR-011-workspace-conventions.md`.

## Commit messages

Conventional Commits, as in the history: `type(scope): summary`, imperative, lowercase.

```
feat(engine-native): Intl.DurationFormat
fix(runtime): close the lost wakeup between platform tasks and loop park
perf(workers): run each worker isolate on its own single thread
```

Common types: `feat`, `fix`, `perf`, `test`, `docs`, `chore`, `ci`, `style`. Scope is a crate
name without the `velyr-` prefix, or an area. No DCO sign-off is required.

## How a change moves

1. Discuss. Ask in Discussions or open an issue from a template. A change to the public surface
   (CLI flags, permissions model, JS-visible APIs) needs an RFC. There is no RFC repository yet: open
   a Discussion in the "Ideas" category and label it `needs-rfc`.
2. Triage. A maintainer adds one `area/*` and one `kind/*` label.
3. Pull request. Fill in the template, keep CI green.
4. Review. One approval from the area owner (see CODEOWNERS and `GOVERNANCE.md`). A change that
   shifts the architecture adds an ADR in `docs/adr`.
5. Release. Nightly builds come from the release workflow schedule. Pre-releases are tagged
   `vX.Y.Z-alpha.N`. There is no stable cadence yet.

## Labels

- Area: `area/engine`, `area/runtime`, `area/security`, `area/web`, `area/io`, `area/compat`, `area/cli`
- Kind: `kind/bug`, `kind/feature`, `kind/perf`, `kind/docs`, `kind/security`
- Other: `good first issue`, `help wanted`, `needs-rfc`, `needs-repro`

The crate-to-area map is in `GOVERNANCE.md`.

## Security issues

Do not file them as issues. Use the private process in [SECURITY.md](SECURITY.md).

## License

Velyr is dual licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). Unless you state
otherwise, any contribution you submit is licensed under those same terms, with no additional
conditions (inbound = outbound). Code adapted from third parties must be attributed in
`THIRD_PARTY_NOTICES.md`.

Participation is covered by the [Code of Conduct](CODE_OF_CONDUCT.md).
