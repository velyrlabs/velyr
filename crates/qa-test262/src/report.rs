//! Outcome classification and the committed expectation manifest.

use std::collections::BTreeMap;

use serde::Serialize;

/// What running one test produced, before comparing with the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail(String),
    Timeout,
    Crash(String),
    Skip(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Pass,
    ExpectedFail,
    UnexpectedFail,
    UnexpectedPass,
    Timeout,
    Crash,
    Skipped,
}

impl Class {
    /// True for results that must fail the run.
    pub fn is_unexpected(self) -> bool {
        matches!(
            self,
            Self::UnexpectedFail | Self::UnexpectedPass | Self::Timeout | Self::Crash
        )
    }
}

/// Test path (relative to the test262 root) -> reason.
pub type Expectations = BTreeMap<String, String>;

pub fn classify(outcome: &Outcome, expected: bool) -> Class {
    match (outcome, expected) {
        (Outcome::Skip(_), _) => Class::Skipped,
        (Outcome::Pass, false) => Class::Pass,
        (Outcome::Pass, true) => Class::UnexpectedPass,
        (_, true) => Class::ExpectedFail,
        (Outcome::Fail(_), false) => Class::UnexpectedFail,
        (Outcome::Timeout, false) => Class::Timeout,
        (Outcome::Crash(_), false) => Class::Crash,
    }
}

const MAX_REASON_CHARS: usize = 200;

/// One-line description of an outcome, used as the manifest reason and in the JSON report.
pub fn reason(outcome: &Outcome) -> String {
    let (prefix, message) = match outcome {
        Outcome::Pass => return String::new(),
        Outcome::Timeout => return "timeout".to_owned(),
        Outcome::Fail(message) => ("", message),
        Outcome::Crash(message) => ("crash: ", message),
        Outcome::Skip(message) => ("skip: ", message),
    };
    let line = message.lines().next().unwrap_or_default();
    format!("{prefix}{line}")
        .chars()
        .take(MAX_REASON_CHARS)
        .collect()
}

pub fn parse_expectations(text: &str) -> Expectations {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| match line.split_once(" # ") {
            Some((path, reason)) => (path.trim().to_owned(), reason.trim().to_owned()),
            None => (line.to_owned(), String::new()),
        })
        .collect()
}

pub fn render_expectations(expectations: &Expectations) -> String {
    expectations
        .iter()
        .map(|(path, reason)| format!("{path} # {reason}\n"))
        .collect()
}

/// Folds a run into `previous`: tests that failed get an entry (an existing reason is
/// kept), tests that passed lose theirs, skipped tests are recorded with their skip reason,
/// and tests not run stay untouched.
pub fn merge_run(previous: &Expectations, run: &[(String, Outcome)]) -> Expectations {
    let mut merged = previous.clone();
    for (path, outcome) in run {
        match outcome {
            Outcome::Skip(_) => {
                merged.insert(path.clone(), reason(outcome));
            }
            Outcome::Pass => {
                merged.remove(path);
            }
            _ => {
                merged
                    .entry(path.clone())
                    .or_insert_with(|| reason(outcome));
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail() -> Outcome {
        Outcome::Fail("boom".into())
    }

    #[test]
    fn classify_table() {
        assert_eq!(classify(&Outcome::Pass, false), Class::Pass);
        assert_eq!(classify(&Outcome::Pass, true), Class::UnexpectedPass);
        assert_eq!(classify(&fail(), true), Class::ExpectedFail);
        assert_eq!(classify(&fail(), false), Class::UnexpectedFail);
        assert_eq!(classify(&Outcome::Timeout, false), Class::Timeout);
        assert_eq!(classify(&Outcome::Timeout, true), Class::ExpectedFail);
        assert_eq!(classify(&Outcome::Crash("x".into()), false), Class::Crash);
        assert_eq!(
            classify(&Outcome::Crash("x".into()), true),
            Class::ExpectedFail
        );
        assert_eq!(classify(&Outcome::Skip("x".into()), true), Class::Skipped);
    }

    #[test]
    fn unexpected_classes() {
        assert!(Class::Crash.is_unexpected());
        assert!(Class::UnexpectedPass.is_unexpected());
        assert!(!Class::ExpectedFail.is_unexpected());
        assert!(!Class::Skipped.is_unexpected());
    }

    #[test]
    fn parse_ignores_comments_and_blanks() {
        let parsed = parse_expectations("# header\n\na/b.js # reason one\nc.js\n");
        assert_eq!(parsed.get("a/b.js").map(String::as_str), Some("reason one"));
        assert_eq!(parsed.get("c.js").map(String::as_str), Some(""));
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn render_roundtrips_sorted() {
        let mut map = Expectations::new();
        map.insert("z.js".into(), "late".into());
        map.insert("a.js".into(), "early".into());
        let text = render_expectations(&map);
        assert!(text.find("a.js").unwrap() < text.find("z.js").unwrap());
        assert_eq!(parse_expectations(&text), map);
    }

    #[test]
    fn merge_keeps_reason_drops_passes_records_skips_and_preserves_untouched() {
        let mut previous = Expectations::new();
        previous.insert("old-fail.js".into(), "manual reason".into());
        previous.insert("now-pass.js".into(), "was failing".into());
        previous.insert("not-run.js".into(), "elsewhere".into());
        previous.insert("skipped.js".into(), "kept".into());
        let run = vec![
            ("old-fail.js".to_owned(), fail()),
            ("now-pass.js".to_owned(), Outcome::Pass),
            (
                "new-fail.js".to_owned(),
                Outcome::Fail("line one\nline two".into()),
            ),
            ("new-timeout.js".to_owned(), Outcome::Timeout),
            ("skipped.js".to_owned(), Outcome::Skip("s".into())),
        ];
        let merged = merge_run(&previous, &run);
        assert_eq!(merged["old-fail.js"], "manual reason");
        assert!(!merged.contains_key("now-pass.js"));
        assert_eq!(merged["not-run.js"], "elsewhere");
        assert_eq!(merged["skipped.js"], "skip: s");
        assert_eq!(merged["new-fail.js"], "line one");
        assert_eq!(merged["new-timeout.js"], "timeout");
    }
}
