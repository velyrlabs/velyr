//! Renders file results as pretty text, TAP 13, JUnit XML or JSON.

use std::fmt::Write;
use std::str::FromStr;

use serde_json::json;

use crate::report::{FileResult, Status, TestResult};

/// Lines of an error message shown by the pretty reporter.
const PRETTY_ERROR_LINES: usize = 10;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Reporter {
    #[default]
    Pretty,
    Tap,
    Junit,
    Json,
}

impl FromStr for Reporter {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        match text {
            "pretty" => Ok(Self::Pretty),
            "tap" => Ok(Self::Tap),
            "junit" => Ok(Self::Junit),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unknown reporter `{other}` (expected pretty, tap, junit or json)"
            )),
        }
    }
}

impl Reporter {
    /// Whether the report can be printed file by file as the files finish (pretty and tap); the
    /// others (junit, json) are one document and come out at the end.
    pub fn streams(self) -> bool {
        matches!(self, Self::Pretty | Self::Tap)
    }
}

/// What a streaming reporter prints before the first file.
pub fn stream_start(reporter: Reporter) -> String {
    match reporter {
        Reporter::Tap => tap_start(),
        _ => String::new(),
    }
}

/// What a streaming reporter prints when `file` finishes. `number` counts the results printed so
/// far (tap numbers them across files).
pub fn stream_file(reporter: Reporter, file: &FileResult, number: &mut usize) -> String {
    match reporter {
        Reporter::Pretty => pretty_file(file),
        Reporter::Tap => tap_file(file, number),
        Reporter::Junit | Reporter::Json => String::new(),
    }
}

/// What a streaming reporter prints after the last file; `files` are in the order of the run.
pub fn stream_end(
    reporter: Reporter,
    files: &[FileResult],
    number: usize,
    elapsed_ms: u64,
) -> String {
    match reporter {
        Reporter::Pretty => pretty_end(files, elapsed_ms),
        Reporter::Tap => tap_end(files, number),
        Reporter::Junit | Reporter::Json => String::new(),
    }
}

/// The report of a run that took `elapsed_ms`.
pub fn render(reporter: Reporter, files: &[FileResult], elapsed_ms: u64) -> String {
    match reporter {
        Reporter::Pretty => pretty(files, elapsed_ms),
        Reporter::Tap => tap(files),
        Reporter::Junit => junit(files, elapsed_ms),
        Reporter::Json => json_report(files, elapsed_ms),
    }
}

fn totals(files: &[FileResult]) -> [usize; 3] {
    [Status::Pass, Status::Fail, Status::Skip]
        .map(|status| files.iter().map(|file| file.count(status)).sum())
}

fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|line| format!("{pad}{line}\n"))
        .collect::<String>()
}

fn pretty(files: &[FileResult], elapsed_ms: u64) -> String {
    let mut out: String = files.iter().map(pretty_file).collect();
    out.push_str(&pretty_end(files, elapsed_ms));
    out
}

fn pretty_file(file: &FileResult) -> String {
    let mut out = String::new();
    let label = if file.failed() { "FAIL" } else { "PASS" };
    let _ = writeln!(out, "{label} {}", file.file);
    let mut shown: Vec<&str> = Vec::new();
    for test in &file.tests {
        let (suites, _) = test.path.split_at(test.path.len().saturating_sub(1));
        let common = suites
            .iter()
            .zip(&shown)
            .take_while(|(a, b)| a == b)
            .count();
        for (depth, suite) in suites.iter().enumerate().skip(common) {
            let _ = writeln!(out, "{}{suite}", "  ".repeat(depth + 1));
        }
        shown = suites.iter().map(String::as_str).collect();
        let pad = "  ".repeat(suites.len() + 1);
        let name = test.path.last().map_or("", String::as_str);
        let mark = match test.status {
            Status::Pass => "ok  ",
            Status::Fail => "FAIL",
            Status::Skip => "skip",
        };
        let _ = writeln!(out, "{pad}{mark} {name} ({} ms)", test.ms);
        if let Some(error) = &test.error {
            let head: Vec<&str> = error.lines().take(PRETTY_ERROR_LINES).collect();
            out.push_str(&indent(&head.join("\n"), pad.len() + 5));
        }
    }
    if let Some(error) = &file.error {
        out.push_str(&indent(error, 2));
    }
    out
}

fn pretty_end(files: &[FileResult], elapsed_ms: u64) -> String {
    let mut out = String::new();
    let [pass, fail, skip] = totals(files);
    let failed_files = files.iter().filter(|file| file.failed()).count();
    let _ = write!(
        out,
        "\nFiles  {} ({failed_files} failed)\nTests  {pass} passed, {fail} failed, {skip} skipped\nTime   {:.2} s\n",
        files.len(),
        elapsed_ms as f64 / 1000.0
    );
    out
}

fn tap(files: &[FileResult]) -> String {
    let mut number = 0;
    let mut out = tap_start();
    for file in files {
        out.push_str(&tap_file(file, &mut number));
    }
    out.push_str(&tap_end(files, number));
    out
}

fn tap_start() -> String {
    "TAP version 13\n".to_owned()
}

/// The block of one file; `number` counts the results printed so far, across files.
fn tap_file(file: &FileResult, number: &mut usize) -> String {
    let mut out = String::new();
    let mut line = |out: &mut String, ok: bool, name: &str, suffix: &str, error: Option<&str>| {
        *number += 1;
        let head = if ok { "ok" } else { "not ok" };
        let _ = writeln!(out, "{head} {number} - {name}{suffix}");
        if let Some(error) = error {
            out.push_str("  ---\n  message: |\n");
            out.push_str(&indent(error, 4));
            out.push_str("  ...\n");
        }
    };
    let _ = writeln!(out, "# {}", file.file);
    for test in &file.tests {
        let suffix = if test.status == Status::Skip {
            " # SKIP"
        } else {
            ""
        };
        let ok = test.status != Status::Fail;
        line(&mut out, ok, &test.name(), suffix, test.error.as_deref());
    }
    if let Some(error) = &file.error {
        line(&mut out, false, &file.file, "", Some(error));
    }
    out
}

fn tap_end(files: &[FileResult], number: usize) -> String {
    let mut out = String::new();
    let [pass, fail, skip] = totals(files);
    let died = files.iter().filter(|file| file.error.is_some()).count();
    let _ = write!(
        out,
        "1..{number}\n# tests {}\n# pass {pass}\n# fail {}\n# skipped {skip}\n",
        pass + fail + skip + died,
        fail + died
    );
    out
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

fn testcase(out: &mut String, file: &str, test: &TestResult) {
    let _ = write!(
        out,
        "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
        xml(file),
        xml(&test.name()),
        test.ms as f64 / 1000.0
    );
    match (test.status, &test.error) {
        (Status::Fail, error) => {
            let text = error.as_deref().unwrap_or("failed");
            let first = text.lines().next().unwrap_or_default();
            let _ = writeln!(
                out,
                ">\n      <failure message=\"{}\">{}</failure>\n    </testcase>",
                xml(first),
                xml(text)
            );
        }
        (Status::Skip, _) => out.push_str(">\n      <skipped/>\n    </testcase>\n"),
        _ => out.push_str("/>\n"),
    }
}

fn junit(files: &[FileResult], elapsed_ms: u64) -> String {
    let [pass, fail, skip] = totals(files);
    let died = files.iter().filter(|file| file.error.is_some()).count();
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites tests=\"{}\" failures=\"{}\" skipped=\"{skip}\" time=\"{:.3}\">\n",
        pass + fail + skip + died,
        fail + died,
        elapsed_ms as f64 / 1000.0
    );
    for file in files {
        let failures = file.count(Status::Fail) + usize::from(file.error.is_some());
        let _ = writeln!(
            out,
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\" skipped=\"{}\">",
            xml(&file.file),
            file.tests.len() + usize::from(file.error.is_some()),
            file.count(Status::Skip)
        );
        for test in &file.tests {
            testcase(&mut out, &file.file, test);
        }
        if let Some(error) = &file.error {
            let _ = writeln!(
                out,
                "    <testcase classname=\"{0}\" name=\"{0}\">\n      <failure message=\"{1}\">{2}</failure>\n    </testcase>",
                xml(&file.file),
                xml(error.lines().next().unwrap_or_default()),
                xml(error)
            );
        }
        out.push_str("  </testsuite>\n");
    }
    out.push_str("</testsuites>\n");
    out
}

fn json_report(files: &[FileResult], elapsed_ms: u64) -> String {
    let [pass, fail, skip] = totals(files);
    let status = |s: Status| match s {
        Status::Pass => "pass",
        Status::Fail => "fail",
        Status::Skip => "skip",
    };
    let files_json: Vec<_> = files
        .iter()
        .map(|file| {
            let tests: Vec<_> = file
                .tests
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name(),
                        "path": t.path,
                        "status": status(t.status),
                        "durationMs": t.ms,
                        "error": t.error,
                    })
                })
                .collect();
            json!({ "file": file.file, "tests": tests, "error": file.error })
        })
        .collect();
    let report = json!({
        "files": files_json,
        "summary": {
            "files": files.len(),
            "failedFiles": files.iter().filter(|f| f.failed()).count(),
            "pass": pass,
            "fail": fail,
            "skip": skip,
            "durationMs": elapsed_ms,
        },
    });
    format!("{report}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test(path: &[&str], status: Status, error: Option<&str>) -> TestResult {
        TestResult {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            status,
            ms: 2,
            error: error.map(str::to_owned),
        }
    }

    fn sample() -> Vec<FileResult> {
        vec![
            FileResult {
                file: "a.test.js".into(),
                tests: vec![
                    test(&["math", "adds"], Status::Pass, None),
                    test(&["math", "subtracts"], Status::Fail, Some("boom <1>\nat x")),
                    test(&["later"], Status::Skip, None),
                ],
                finished: true,
                ..FileResult::default()
            },
            FileResult {
                file: "b.test.js".into(),
                error: Some("SyntaxError: nope".into()),
                ..FileResult::default()
            },
        ]
    }

    #[test]
    fn reporter_names() {
        assert_eq!("tap".parse(), Ok(Reporter::Tap));
        assert!("xml".parse::<Reporter>().is_err());
    }

    #[test]
    fn pretty_groups_suites_and_counts() {
        let text = pretty(&sample(), 1500);
        assert!(
            text.contains("FAIL a.test.js\n  math\n    ok   adds (2 ms)"),
            "{text}"
        );
        assert!(text.contains("FAIL subtracts"), "{text}");
        assert!(text.contains("skip later"), "{text}");
        assert!(text.contains("Files  2 (2 failed)"), "{text}");
        assert!(
            text.contains("Tests  1 passed, 1 failed, 1 skipped"),
            "{text}"
        );
        assert!(text.contains("Time   1.50 s"), "{text}");
    }

    #[test]
    fn tap_numbers_every_result_and_marks_skips() {
        let text = tap(&sample());
        assert!(
            text.starts_with("TAP version 13\n# a.test.js\nok 1 - math > adds\n"),
            "{text}"
        );
        assert!(
            text.contains("not ok 2 - math > subtracts\n  ---\n  message: |\n    boom <1>"),
            "{text}"
        );
        assert!(text.contains("ok 3 - later # SKIP"), "{text}");
        assert!(text.contains("not ok 4 - b.test.js"), "{text}");
        assert!(
            text.contains("1..4\n# tests 4\n# pass 1\n# fail 2\n# skipped 1\n"),
            "{text}"
        );
    }

    #[test]
    fn streamed_pieces_add_up_to_the_whole_report() {
        for reporter in [Reporter::Pretty, Reporter::Tap] {
            assert!(reporter.streams());
            let files = sample();
            let mut number = 0;
            let mut text = stream_start(reporter);
            for file in &files {
                text.push_str(&stream_file(reporter, file, &mut number));
            }
            text.push_str(&stream_end(reporter, &files, number, 1500));
            assert_eq!(text, render(reporter, &files, 1500));
        }
        assert!(!Reporter::Json.streams() && !Reporter::Junit.streams());
    }

    #[test]
    fn junit_escapes_and_counts() {
        let text = junit(&sample(), 1500);
        assert!(
            text.contains("<testsuites tests=\"4\" failures=\"2\" skipped=\"1\""),
            "{text}"
        );
        assert!(text.contains("message=\"boom &lt;1&gt;\""), "{text}");
        assert!(text.contains("<skipped/>"), "{text}");
    }

    #[test]
    fn json_has_files_and_a_summary() {
        let value: serde_json::Value = serde_json::from_str(&json_report(&sample(), 9)).unwrap();
        assert_eq!(value["summary"]["fail"], 1);
        assert_eq!(value["summary"]["failedFiles"], 2);
        assert_eq!(value["files"][0]["tests"][1]["name"], "math > subtracts");
        assert_eq!(value["files"][1]["error"], "SyntaxError: nope");
    }
}
