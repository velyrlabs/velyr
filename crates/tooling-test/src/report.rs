//! What a test file produced, and how the child's output is turned into it.

use serde_json::Value;

/// Prefix of the lines the runner in a child prints for the parent; `\u{1}` keeps them apart from
/// anything a test prints on its own.
pub const SENTINEL: &str = "\u{1}velyr-test ";
/// Lines of stderr kept as the reason a file died.
const ERROR_TAIL_LINES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    /// Suite names, then the test name.
    pub path: Vec<String>,
    pub status: Status,
    pub ms: u64,
    pub error: Option<String>,
}

impl TestResult {
    pub fn name(&self) -> String {
        self.path.join(" > ")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileResult {
    pub file: String,
    pub tests: Vec<TestResult>,
    /// The runner reached its end (as opposed to the file dying first).
    pub finished: bool,
    /// Why the file produced no verdict: a crash, a syntax error, a denied permission.
    pub error: Option<String>,
    /// What the test printed on its own (stdout then stderr).
    pub output: String,
}

impl FileResult {
    pub fn failed(&self) -> bool {
        self.error.is_some() || self.tests.iter().any(|t| t.status == Status::Fail)
    }

    pub fn count(&self, status: Status) -> usize {
        self.tests.iter().filter(|t| t.status == status).count()
    }
}

fn event_to_result(event: &Value) -> Option<TestResult> {
    let path = event["path"]
        .as_array()?
        .iter()
        .filter_map(|part| part.as_str().map(str::to_owned))
        .collect();
    let status = match event["status"].as_str()? {
        "pass" => Status::Pass,
        "fail" => Status::Fail,
        "skip" => Status::Skip,
        _ => return None,
    };
    Some(TestResult {
        path,
        status,
        ms: event["ms"].as_u64().unwrap_or(0),
        error: event["error"].as_str().map(str::to_owned),
    })
}

/// The test that is running once `line` of the child's stdout has been seen: a `start` event
/// begins one, a `test` or `end` event finishes it, any other line changes nothing.
pub fn running_after(line: &str, running: Option<Vec<String>>) -> Option<Vec<String>> {
    let Some(json) = line.strip_prefix(SENTINEL) else {
        return running;
    };
    let Ok(event) = serde_json::from_str::<Value>(json) else {
        return running;
    };
    match event["type"].as_str() {
        Some("start") => event["path"].as_array().map(|path| {
            path.iter()
                .filter_map(|part| part.as_str().map(str::to_owned))
                .collect()
        }),
        Some("test" | "end") => None,
        _ => running,
    }
}

/// Builds the result of `file` from its child's stdout and stderr and whether it exited with 0.
pub fn parse_output(file: &str, stdout: &str, stderr: &str, success: bool) -> FileResult {
    let mut result = FileResult {
        file: file.to_owned(),
        ..FileResult::default()
    };
    for line in stdout.lines() {
        let Some(json) = line.strip_prefix(SENTINEL) else {
            result.output.push_str(line);
            result.output.push('\n');
            continue;
        };
        let Ok(event) = serde_json::from_str::<Value>(json) else {
            continue;
        };
        match event["type"].as_str() {
            Some("test") => result.tests.extend(event_to_result(&event)),
            Some("end") => result.finished = true,
            _ => {}
        }
    }
    result.output.push_str(stderr);
    if !result.finished && !success {
        let lines: Vec<&str> = stderr.lines().collect();
        let tail = lines[lines.len().saturating_sub(ERROR_TAIL_LINES)..].join("\n");
        result.error = Some(if tail.trim().is_empty() {
            "the file exited with an error before the tests finished".to_owned()
        } else {
            tail
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(json: &str) -> String {
        format!("{SENTINEL}{json}\n")
    }

    #[test]
    fn events_become_results_and_other_lines_stay_output() {
        let stdout = format!(
            "hello\n{}{}{}",
            line(r#"{"type":"test","path":["a","b"],"status":"pass","ms":3}"#),
            line(r#"{"type":"test","path":["c"],"status":"fail","ms":1,"error":"boom"}"#),
            line(r#"{"type":"end"}"#)
        );
        let result = parse_output("f.test.js", &stdout, "warn\n", false);
        assert_eq!(result.tests.len(), 2);
        assert_eq!(result.tests[0].name(), "a > b");
        assert_eq!(result.tests[1].error.as_deref(), Some("boom"));
        assert!(result.finished && result.failed() && result.error.is_none());
        assert_eq!(result.output, "hello\nwarn\n");
    }

    #[test]
    fn a_file_that_dies_before_the_end_fails_with_its_stderr() {
        let result = parse_output("f.test.js", "", "SyntaxError: nope\n", false);
        assert!(result.failed());
        assert_eq!(result.error.as_deref(), Some("SyntaxError: nope"));
    }

    #[test]
    fn start_events_track_the_running_test() {
        let path = |names: &[&str]| Some(names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>());
        let start = line(r#"{"type":"start","path":["a","b"]}"#);
        assert_eq!(running_after(start.trim_end(), None), path(&["a", "b"]));
        let done = line(r#"{"type":"test","path":["a","b"],"status":"pass","ms":1}"#);
        assert_eq!(running_after(done.trim_end(), path(&["a", "b"])), None);
        assert_eq!(running_after("plain output", path(&["x"])), path(&["x"]));
    }

    #[test]
    fn a_file_with_no_tests_that_exits_cleanly_passes() {
        let result = parse_output("f.test.js", "", "", true);
        assert!(!result.failed() && result.tests.is_empty());
    }

    #[test]
    fn malformed_events_are_ignored() {
        let stdout = format!("{SENTINEL}not json\n{}", line(r#"{"type":"end"}"#));
        assert!(parse_output("f", &stdout, "", true).finished);
    }
}
