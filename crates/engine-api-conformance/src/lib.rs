//! Conformance suite for `velyr-engine-api` implementations.
//!
//! Depends only on the engine-api contract, so a case that needs an
//! engine-specific type does not compile (ADR-009). Each adapter calls
//! [`run_all`] from its own `tests/conformance.rs`.

mod cases;
mod serialize;

use std::fmt;

use velyr_engine_api::{Engine, EngineFeatures};

/// What happened to one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseResult {
    pub name: &'static str,
    pub outcome: Outcome,
}

#[derive(Debug, Default)]
pub struct Report {
    pub results: Vec<CaseResult>,
}

impl Report {
    fn count(&self, pick: fn(&Outcome) -> bool) -> usize {
        self.results.iter().filter(|r| pick(&r.outcome)).count()
    }

    pub fn passed(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Pass))
    }

    pub fn failed(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Fail(_)))
    }

    pub fn skipped(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Skip(_)))
    }

    pub fn outcome(&self, name: &str) -> Option<&Outcome> {
        self.results
            .iter()
            .find(|r| r.name == name)
            .map(|r| &r.outcome)
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for r in &self.results {
            match &r.outcome {
                Outcome::Pass => writeln!(f, "pass {}", r.name)?,
                Outcome::Fail(why) => writeln!(f, "FAIL {}: {why}", r.name)?,
                Outcome::Skip(why) => writeln!(f, "skip {}: {why}", r.name)?,
            }
        }
        write!(
            f,
            "{} passed, {} failed, {} skipped",
            self.passed(),
            self.failed(),
            self.skipped()
        )
    }
}

/// A case runs on a fresh isolate of the engine under test.
pub(crate) struct Case<E: Engine> {
    pub name: &'static str,
    /// Feature the engine must declare, with its name for the skip reason.
    pub requires: Option<(&'static str, EngineFeatures)>,
    pub run: fn(&E) -> Result<(), String>,
}

/// Runs every case the engine's declared features allow; skips the rest with a reason.
pub fn run_all<E: Engine>(engine: &E) -> Report {
    let features = engine.features();
    let results = cases::all::<E>()
        .into_iter()
        .chain(serialize::all::<E>())
        .map(|case| {
            let outcome = match case.requires {
                Some((label, flag)) if !features.contains(flag) => {
                    Outcome::Skip(format!("engine does not declare {label}"))
                }
                _ => match (case.run)(engine) {
                    Ok(()) => Outcome::Pass,
                    Err(why) => Outcome::Fail(why),
                },
            };
            CaseResult {
                name: case.name,
                outcome,
            }
        })
        .collect();
    Report { results }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_counts_and_displays_each_outcome() {
        let report = Report {
            results: vec![
                CaseResult {
                    name: "a",
                    outcome: Outcome::Pass,
                },
                CaseResult {
                    name: "b",
                    outcome: Outcome::Fail("boom".into()),
                },
                CaseResult {
                    name: "c",
                    outcome: Outcome::Skip("no promises".into()),
                },
            ],
        };
        assert_eq!(
            (report.passed(), report.failed(), report.skipped()),
            (1, 1, 1)
        );
        assert_eq!(
            report.outcome("c"),
            Some(&Outcome::Skip("no promises".into()))
        );
        let text = report.to_string();
        assert!(text.contains("FAIL b: boom") && text.contains("skip c: no promises"));
    }
}
