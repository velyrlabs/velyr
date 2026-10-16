//! Outcome classification, the committed expectation manifest and per-directory pass rates.
//!
//! A unit is one subtest (`path::name`, or `path?variant::name`), or a whole file when the
//! harness did not run (`path`): skipped, crashed, timed out or failed to load.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

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

/// One result: `test` is the file path plus the variant query, `subtest` its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub test: String,
    pub subtest: Option<String>,
    pub outcome: Outcome,
}

impl Unit {
    pub fn key(&self) -> String {
        match &self.subtest {
            Some(name) => format!("{}::{}", self.test, one_line(name)),
            None => self.test.clone(),
        }
    }

    /// Directory of the test file, relative to the WPT root.
    pub fn directory(&self) -> &str {
        let path = self.test.split('?').next().unwrap_or_default();
        path.rsplit_once('/').map_or("", |(dir, _)| dir)
    }
}

/// Manifest lines are `key # reason` and parsed trimmed, so a key holds no newline, no ` # `, and
/// no leading or trailing whitespace (those become U+2423).
fn one_line(name: &str) -> String {
    let flat = name
        .replace(['\n', '\r'], " ")
        .replace(" # ", " #\u{200b} ");
    let body = flat.trim();
    let lead = flat.chars().count() - flat.trim_start().chars().count();
    let trail = if body.is_empty() {
        0
    } else {
        flat.chars().count() - flat.trim_end().chars().count()
    };
    format!(
        "{}{body}{}",
        "\u{2423}".repeat(lead),
        "\u{2423}".repeat(trail)
    )
}

/// Unit key -> reason.
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
            Some((key, reason)) => (key.trim().to_owned(), reason.trim().to_owned()),
            None => (line.to_owned(), String::new()),
        })
        .collect()
}

pub fn render_expectations(expectations: &Expectations) -> String {
    expectations
        .iter()
        .map(|(key, reason)| format!("{key} # {reason}\n"))
        .collect()
}

/// Test id (`path` or `path?variant`) of an expectation key.
fn test_of(key: &str) -> &str {
    key.split_once("::").map_or(key, |(test, _)| test)
}

/// Folds a run into `previous`: failing units get an entry (an existing reason is kept), passing
/// units lose theirs, skips are recorded with their reason. For a test that ran, entries the run
/// no longer produced are dropped; tests that did not run stay untouched.
pub fn merge_run(previous: &Expectations, run: &[Unit]) -> Expectations {
    let ran: BTreeSet<&str> = run.iter().map(|unit| unit.test.as_str()).collect();
    let produced: BTreeSet<String> = run.iter().map(Unit::key).collect();
    let mut merged: Expectations = previous
        .iter()
        .filter(|(key, _)| !ran.contains(test_of(key)) || produced.contains(*key))
        .map(|(key, reason)| (key.clone(), reason.clone()))
        .collect();
    for unit in run {
        let key = unit.key();
        match &unit.outcome {
            Outcome::Pass => {
                merged.remove(&key);
            }
            Outcome::Skip(_) => {
                merged.insert(key, reason(&unit.outcome));
            }
            outcome => {
                merged.entry(key).or_insert_with(|| reason(outcome));
            }
        }
    }
    merged
}

/// Gives repeated subtest names within one test a ` (n)` suffix so every key is unique.
pub fn dedupe_names(units: &mut [Unit]) {
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    for unit in units {
        let Some(name) = &mut unit.subtest else {
            continue;
        };
        let count = seen.entry((unit.test.clone(), name.clone())).or_insert(0);
        *count += 1;
        if *count > 1 {
            name.push_str(&format!(" ({count})"));
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Rate {
    pub pass: usize,
    pub total: usize,
    pub pass_rate: f64,
}

/// Pass rate per directory of the test file; skipped units are not counted.
pub fn directory_rates(units: &[Unit]) -> BTreeMap<String, Rate> {
    let mut rates: BTreeMap<String, Rate> = BTreeMap::new();
    for unit in units {
        if matches!(unit.outcome, Outcome::Skip(_)) {
            continue;
        }
        let rate = rates.entry(unit.directory().to_owned()).or_default();
        rate.total += 1;
        rate.pass += usize::from(unit.outcome == Outcome::Pass);
    }
    for rate in rates.values_mut() {
        rate.pass_rate = (rate.pass as f64 / rate.total as f64 * 10_000.0).round() / 100.0;
    }
    rates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(test: &str, subtest: Option<&str>, outcome: Outcome) -> Unit {
        Unit {
            test: test.into(),
            subtest: subtest.map(str::to_owned),
            outcome,
        }
    }

    #[test]
    fn classify_table() {
        assert_eq!(classify(&Outcome::Pass, false), Class::Pass);
        assert_eq!(classify(&Outcome::Pass, true), Class::UnexpectedPass);
        assert_eq!(
            classify(&Outcome::Fail("x".into()), true),
            Class::ExpectedFail
        );
        assert_eq!(
            classify(&Outcome::Fail("x".into()), false),
            Class::UnexpectedFail
        );
        assert_eq!(classify(&Outcome::Timeout, false), Class::Timeout);
        assert_eq!(classify(&Outcome::Crash("x".into()), false), Class::Crash);
        assert_eq!(classify(&Outcome::Skip("x".into()), true), Class::Skipped);
    }

    #[test]
    fn keys_are_single_line_and_parse_back() {
        let tricky = unit("a/b.any.js?x", Some("two\nlines # and hash"), Outcome::Pass);
        let padded = unit("a/b.any.js", Some(" input: "), Outcome::Pass);
        let mut map = Expectations::new();
        map.insert(tricky.key(), "reason".into());
        map.insert(padded.key(), "reason".into());
        let text = render_expectations(&map);
        assert_eq!(text.lines().count(), 2);
        assert_eq!(parse_expectations(&text), map);
    }

    #[test]
    fn repeated_names_get_a_suffix() {
        let mut units = vec![
            unit("a.any.js", Some("same"), Outcome::Pass),
            unit("a.any.js", Some("same"), Outcome::Pass),
            unit("b.any.js", Some("same"), Outcome::Pass),
        ];
        dedupe_names(&mut units);
        let keys: Vec<_> = units.iter().map(Unit::key).collect();
        assert_eq!(
            keys,
            ["a.any.js::same", "a.any.js::same (2)", "b.any.js::same"]
        );
    }

    #[test]
    fn merge_records_failures_drops_passes_and_stale_entries() {
        let mut previous = Expectations::new();
        previous.insert("a.any.js::old".into(), "kept reason".into());
        previous.insert("a.any.js::now-passes".into(), "was failing".into());
        previous.insert("a.any.js".into(), "harness used to fail".into());
        previous.insert("other.any.js::x".into(), "not run".into());
        let run = vec![
            unit("a.any.js", Some("old"), Outcome::Fail("boom".into())),
            unit("a.any.js", Some("now-passes"), Outcome::Pass),
            unit(
                "a.any.js",
                Some("new"),
                Outcome::Fail("line one\nline two".into()),
            ),
            unit("s.any.js", None, Outcome::Skip("no server".into())),
        ];
        let merged = merge_run(&previous, &run);
        assert_eq!(merged["a.any.js::old"], "kept reason");
        assert_eq!(merged["a.any.js::new"], "line one");
        assert_eq!(merged["s.any.js"], "skip: no server");
        assert_eq!(merged["other.any.js::x"], "not run");
        assert!(!merged.contains_key("a.any.js::now-passes"));
        assert!(!merged.contains_key("a.any.js"));
    }

    #[test]
    fn rates_are_per_directory_and_ignore_skips() {
        let units = vec![
            unit("url/a.any.js", Some("1"), Outcome::Pass),
            unit("url/a.any.js", Some("2"), Outcome::Fail("x".into())),
            unit("url/b.any.js?v", Some("1"), Outcome::Pass),
            unit("url/c.any.js", None, Outcome::Skip("s".into())),
            unit("enc/d.any.js", Some("1"), Outcome::Pass),
        ];
        let rates = directory_rates(&units);
        assert_eq!(
            rates["url"],
            Rate {
                pass: 2,
                total: 3,
                pass_rate: 66.67
            }
        );
        assert_eq!(
            rates["enc"],
            Rate {
                pass: 1,
                total: 1,
                pass_rate: 100.0
            }
        );
    }
}
