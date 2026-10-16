//! The native test runner behind `velyr test`: the `velyr:test` / `node:test` module (`test.js`),
//! test file discovery, one child process per file (so one isolate per file), and the reporters.
//!
//! Each child prints one sentinel line per result; the parent parses those lines, forwards
//! everything else as the test's own output and renders the results.

mod discover;
mod exec;
mod report;
mod reporters;

pub use discover::{discover, is_test_file};
pub use exec::{Limits, default_jobs, run_files};
pub use report::{FileResult, SENTINEL, Status, TestResult, parse_output, running_after};
pub use reporters::{Reporter, render, stream_end, stream_file, stream_start};

/// Source of the `velyr:test` module, written as a registry factory body (`module.exports = ...`).
pub const TEST_JS: &str = include_str!("test.js");
