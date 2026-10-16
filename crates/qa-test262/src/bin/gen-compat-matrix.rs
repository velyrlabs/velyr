//! `gen-compat-matrix`: renders `docs/COMPATIBILITY-MATRIX.md` from the Test262, Node behavior and
//! npm package result JSON in `compat/results/` (ADR-015). A status is never edited by hand:
//! rerun this after a run.
//!
//! Result files are per run and often cover only some directories, so for each engine the newest
//! result of every test (and of every Node module and package) wins. Read-only on the results.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
#[path = "../matrix_node.rs"]
mod matrix_node;
use matrix_node::{
    NodeResults, aggregate, behavior_status, load_node, package_status, render_json, render_node,
    runs,
};

const ENGINES: [&str; 2] = ["v8", "native"];
/// Directory components that name an area: `test/built-ins/Array`, `test/language/expressions`.
const AREA_DEPTH: usize = 3;

#[derive(Parser)]
#[command(about = "Generate the Test262 compatibility matrix from result JSON")]
struct Args {
    #[arg(long, default_value = "compat/results")]
    results_dir: PathBuf,
    #[arg(long, default_value = "docs/COMPATIBILITY-MATRIX.md")]
    out: PathBuf,
    /// Status data embedded in the `velyr compat` binary.
    #[arg(long, default_value = "crates/cli/src/commands/compat_matrix.json")]
    json_out: PathBuf,
}

#[derive(Default, Clone, Copy)]
struct Counts {
    pass: usize,
    total: usize,
}

/// Test path -> class of its newest result, and the newest Test262 revision seen.
#[derive(Default)]
struct EngineResults {
    classes: BTreeMap<String, String>,
    sha: String,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let result = ENGINES
        .iter()
        .map(|engine| load(&args.results_dir, engine))
        .collect::<Result<Vec<_>, String>>()
        .and_then(|loaded| Ok((loaded, load_node(&args.results_dir)?)))
        .and_then(|(loaded, node)| {
            fs::write(&args.out, render(&loaded, &node))
                .map_err(|e| format!("{}: {e}", args.out.display()))?;
            fs::write(&args.json_out, render_json(&node))
                .map_err(|e| format!("{}: {e}", args.json_out.display()))
        });
    match result {
        Ok(()) => {
            eprintln!("wrote {}", args.out.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn load(dir: &Path, engine: &str) -> Result<EngineResults, String> {
    let mut loaded = EngineResults::default();
    for (_, doc) in runs(dir, &format!("test262-{engine}-"))? {
        for result in doc["results"].as_array().into_iter().flatten() {
            if let (Some(path), Some(class)) = (result["path"].as_str(), result["class"].as_str()) {
                loaded.classes.insert(path.to_owned(), class.to_owned());
            }
        }
        if let Some(sha) = doc["test262_sha"].as_str().filter(|sha| !sha.is_empty()) {
            loaded.sha = sha.to_owned();
        }
    }
    Ok(loaded)
}

/// `test/built-ins/Array/isArray/x.js` -> `test/built-ins/Array`; a file directly under `test/`
/// or one level down is grouped by its directory.
fn area(path: &str) -> String {
    let dirs: Vec<&str> = path.split('/').collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    dirs[..dirs.len().min(AREA_DEPTH)].join("/")
}

fn tally(results: &EngineResults) -> BTreeMap<String, Counts> {
    let mut areas: BTreeMap<String, Counts> = BTreeMap::new();
    for (path, class) in &results.classes {
        let counts = areas.entry(area(path)).or_default();
        counts.total += 1;
        counts.pass += usize::from(class == "pass" || class == "unexpected_pass");
    }
    areas
}

fn cell(counts: Option<Counts>) -> String {
    match counts {
        Some(c) if c.total > 0 => format!(
            "{}/{} ({:.1}%)",
            c.pass,
            c.total,
            100.0 * c.pass as f64 / c.total as f64
        ),
        _ => "-".to_owned(),
    }
}

fn render_areas(results: &[EngineResults], node: &NodeResults, out: &mut String) {
    let native = results.get(1).map(tally).unwrap_or_default();
    let (pass, total) = native
        .values()
        .fold((0, 0), |(p, t), c| (p + c.pass, t + c.total));
    let ecma = if total == 0 {
        "planned"
    } else {
        behavior_status(pass, total)
    };
    let core = aggregate(
        node.behavior
            .values()
            .map(|b| behavior_status(b.pass, b.total)),
    );
    let npm = aggregate(node.packages.values().map(package_status));
    out.push_str("| Area | Target | Status | Evidence |\n| --- | --- | --- | --- |\n");
    for (area, target, status, evidence) in [
        (
            "ECMAScript",
            "ECMA-262 / Test262",
            ecma,
            "[table](#test262-ecmascript)",
        ),
        (
            "Web APIs",
            "Minimum Common Web API",
            "planned",
            "WPT manifest",
        ),
        ("ESM", "Velyr native", "planned", "module suite"),
        ("CommonJS", "Node compatibility", "planned", "compat suite"),
        (
            "package.json exports",
            "Node semantics",
            "planned",
            "resolver suite",
        ),
        (
            "npm registry",
            "package consumption",
            npm,
            "[table](#npm-packages)",
        ),
        (
            "Node core",
            "per module",
            core,
            "[table](#node-core-modules)",
        ),
        ("Node-API", "versioned subset", "planned", "addon corpus"),
        (
            "React",
            "current supported target",
            "planned",
            "acceptance app",
        ),
        (
            "Vue",
            "current supported target",
            "planned",
            "acceptance app",
        ),
        (
            "Vite",
            "current supported target",
            "planned",
            "acceptance app",
        ),
    ] {
        out.push_str(&format!("| {area} | {target} | {status} | {evidence} |\n"));
    }
    out.push_str(
        "\nStatus legend: `unsupported` (nothing passes or install/smoke failed), `partial` \
         (some tests fail, or only part of the API is covered), `experimental` (every sampled \
         test passes; a small sample does not prove `supported`), `planned` (no evidence yet).\n\n",
    );
}

fn render(results: &[EngineResults], node: &NodeResults) -> String {
    let mut out = String::from(
        "# Compatibility matrix\n\n\
         Generated by `scripts/gen-compat-matrix` from `compat/results/`; do not edit by hand.\n\n",
    );
    render_areas(results, node, &mut out);
    render_node(node, &mut out);
    out.push_str("## Test262 (ECMAScript)\n\n");
    out.push_str(&render_test262(results));
    out.push_str(
        "\n## Rule\n\nNever edit a `Supported` status by hand. \
         CI/release automation owns supported evidence.\n",
    );
    out
}

fn render_test262(results: &[EngineResults]) -> String {
    let tallies: Vec<_> = results.iter().map(tally).collect();
    let mut areas: Vec<&String> = tallies.iter().flat_map(BTreeMap::keys).collect();
    areas.sort();
    areas.dedup();
    let mut out = String::from(
        "Each cell is passing tests / tests run (skipped tests count as run, not passing), \
         using the newest result of every test.\n\n",
    );
    for (engine, results) in ENGINES.iter().zip(results) {
        let sha = if results.sha.is_empty() {
            "unknown"
        } else {
            &results.sha
        };
        out.push_str(&format!("- {engine}: Test262 {sha}\n"));
    }
    out.push_str("\n| Area | v8 | native |\n| --- | --- | --- |\n");
    for area in areas {
        let cells: Vec<String> = tallies.iter().map(|t| cell(t.get(area).copied())).collect();
        out.push_str(&format!("| {area} | {} |\n", cells.join(" | ")));
    }
    let totals: Vec<String> = tallies
        .iter()
        .map(|t| {
            cell(Some(t.values().fold(Counts::default(), |sum, c| Counts {
                pass: sum.pass + c.pass,
                total: sum.total + c.total,
            })))
        })
        .collect();
    out.push_str(&format!("| **Total** | {} |\n", totals.join(" | ")));
    out
}

#[cfg(test)]
mod tests {
    use super::matrix_node::{Behavior, Package};
    use super::*;

    fn results(entries: &[(&str, &str)]) -> EngineResults {
        EngineResults {
            classes: entries
                .iter()
                .map(|(p, c)| ((*p).to_owned(), (*c).to_owned()))
                .collect(),
            sha: "abc".into(),
        }
    }

    #[test]
    fn areas_are_the_first_three_directories() {
        assert_eq!(
            area("test/built-ins/Array/isArray/x.js"),
            "test/built-ins/Array"
        );
        assert_eq!(area("test/harness/x.js"), "test/harness");
        assert_eq!(area("test/x.js"), "test");
    }

    #[test]
    fn renders_pass_rates_per_area_and_engine() {
        let v8 = results(&[
            ("test/built-ins/Array/a.js", "pass"),
            ("test/built-ins/Array/b.js", "expected_fail"),
        ]);
        let native = results(&[
            ("test/built-ins/Array/a.js", "unexpected_pass"),
            ("test/language/x/a.js", "skipped"),
        ]);
        let text = render(&[v8, native], &NodeResults::default());
        assert!(
            text.contains("| test/built-ins/Array | 1/2 (50.0%) | 1/1 (100.0%) |"),
            "{text}"
        );
        assert!(
            text.contains("| test/language/x | - | 0/1 (0.0%) |"),
            "{text}"
        );
        assert!(
            text.contains("| **Total** | 1/2 (50.0%) | 1/2 (50.0%) |"),
            "{text}"
        );
    }

    fn package(installed: bool, smoke: &str, partial: bool) -> Package {
        Package {
            version: "1.0.0".into(),
            installed,
            smoke: smoke.into(),
            partial,
            evidence: "node-packages-1.json".into(),
        }
    }

    #[test]
    fn behavior_status_follows_the_pass_rate() {
        assert_eq!(behavior_status(0, 4), "unsupported");
        assert_eq!(behavior_status(3, 4), "partial");
        assert_eq!(behavior_status(4, 4), "experimental");
        assert_eq!(behavior_status(0, 0), "unsupported");
    }

    #[test]
    fn package_status_needs_install_and_smoke() {
        assert_eq!(
            package_status(&package(false, "skip", false)),
            "unsupported"
        );
        assert_eq!(package_status(&package(true, "fail", false)), "unsupported");
        assert_eq!(
            package_status(&package(true, "pass", true)),
            "partial (no server)"
        );
        assert_eq!(
            package_status(&package(true, "pass", false)),
            "experimental"
        );
    }

    #[test]
    fn renders_node_modules_and_packages_with_evidence_links() {
        let mut node = NodeResults::default();
        node.behavior.insert(
            "path".into(),
            Behavior {
                total: 3,
                pass: 3,
                evidence: "node-behavior-path-1.json".into(),
            },
        );
        node.packages
            .insert("zod".into(), package(true, "pass", false));
        node.packages
            .insert("pg".into(), package(true, "pass", true));
        let text = render(&[], &node);
        assert!(
            text.contains(
                "| path | 3/3 (100.0%) | experimental | [result](../compat/results/node-behavior-path-1.json) |"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "| zod | 1.0.0 | experimental | [result](../compat/results/node-packages-1.json), [smoke](../compat/node/smoke/zod.mjs) |"
            ),
            "{text}"
        );
        assert!(
            text.contains("| pg | 1.0.0 | partial (no server) |"),
            "{text}"
        );
        assert!(
            text.contains("| Node core | per module | experimental |"),
            "{text}"
        );
        assert!(
            text.contains("Never edit a `Supported` status by hand"),
            "{text}"
        );
    }

    #[test]
    fn the_newest_node_result_wins_per_module_and_package() {
        let dir = tempfile::tempdir().unwrap();
        let behavior = |name: &str, ts: u64, passed: u64| {
            let doc = serde_json::json!({
                "timestamp": ts, "module": "path", "total": 4, "passed": passed,
            });
            fs::write(dir.path().join(name), doc.to_string()).unwrap();
        };
        behavior("node-behavior-path-2.json", 2, 4);
        behavior("node-behavior-path-1.json", 1, 0);
        let packages = |name: &str, ts: u64, smoke: &str, pkg: &str| {
            let doc = serde_json::json!({
                "timestamp": ts,
                "packages": {pkg: {"version": "2.0.0", "installed": true, "smoke": smoke}},
            });
            fs::write(dir.path().join(name), doc.to_string()).unwrap();
        };
        packages("node-packages-1.json", 1, "fail", "zod");
        packages("node-packages-2.json", 2, "pass", "zod");
        packages("node-packages-3.json", 3, "pass", "koa");
        let node = load_node(dir.path()).unwrap();
        assert_eq!(node.behavior["path"].pass, 4);
        assert_eq!(node.behavior["path"].evidence, "node-behavior-path-2.json");
        assert_eq!(node.packages["zod"].smoke, "pass");
        assert_eq!(node.packages["zod"].evidence, "node-packages-2.json");
        assert_eq!(node.packages.len(), 2);
    }

    #[test]
    fn the_newest_result_of_a_test_wins() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, ts: u64, class: &str| {
            let doc = serde_json::json!({
                "timestamp": ts,
                "results": [{"path": "test/a/b/c.js", "class": class}],
            });
            fs::write(dir.path().join(name), doc.to_string()).unwrap();
        };
        write("test262-v8-2.json", 2, "pass");
        write("test262-v8-1.json", 1, "unexpected_fail");
        write("test262-native-9.json", 9, "crash");
        write("npm-corpus-5.json", 5, "pass");
        let loaded = load(dir.path(), "v8").unwrap();
        assert_eq!(loaded.classes["test/a/b/c.js"], "pass");
        assert_eq!(load(dir.path(), "native").unwrap().classes.len(), 1);
    }
}
