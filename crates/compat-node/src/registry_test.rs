//! `node:test` and `velyr:test`: one runner (velyr-tooling-test's `test.js`) behind both names.
//! `node:test` needs its prefix (a bare `test` would shadow the npm package of that name), which
//! `registry::builtin_name` enforces; `node:assert` is the existing `assert` builtin.

/// Exports of both names (`test` itself is the default export).
pub(crate) const TEST_EXPORTS: &[&str] = &[
    "after",
    "afterEach",
    "before",
    "beforeEach",
    "describe",
    "expect",
    "it",
    "mock",
    "skip",
    "test",
    "todo",
];

pub(crate) const TEST_FACTORIES: &[(&str, &str)] = &[
    ("test", include_str!("../../tooling-test/src/test.js")),
    ("velyr:test", "module.exports = require('test');"),
];
