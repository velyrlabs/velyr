# ADR-015 — Conformance suite vendoring and reporting

**Status:** Accepted (2026-10-15)
**Relates to:** `compatibility/*`, PLANS §9, REFERENCES study rules

## Decision

1. Test262 and WPT are not git submodules. `scripts/fetch-test262.sh` and `scripts/fetch-wpt.sh` download the commit SHA pinned in `compat/test262.lock` or `compat/wpt.lock` into `compat/vendor/`, which is gitignored.
2. The runner crates are `crates/qa-test262` (binary `velyr-test262`) and `crates/qa-wpt` (binary `velyr-wpt`). They run a suite against a selected engine or runtime build and write JSON results to `compat/results/`, which is gitignored.
3. Expected-failure manifests are committed: `compat/test262/expectations/<engine>.txt` and `compat/wpt/expectations.txt`. Each entry has a reason. Outcomes are classified as pass, expected fail, unexpected fail, unexpected pass, timeout or crash.
4. The selected WPT tests are listed in `compat/wpt/manifest.txt`, which is versioned.
5. `scripts/gen-compat-matrix` regenerates `docs/COMPATIBILITY-MATRIX.md` from result JSON. A status is never edited by hand.
6. Node and npm compatibility corpora live in `compat/node/` and `compat/npm/`. Frontend acceptance apps live in `compat/acceptance/<name>/`.
