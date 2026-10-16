//! `velyr-wpt`: runs the selected web-platform-tests in the Velyr runtime and diffs the result
//! with the committed expectations (ADR-015).

mod env;
mod manifest;
mod meta;
mod report;
mod runner;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use serde::Serialize;

use report::{Class, Expectations, Unit, classify};
use runner::Runner;

const MAX_THREADS: usize = 8;
const MAX_PRINTED_UNEXPECTED: usize = 40;

#[derive(Parser)]
#[command(about = "Run web-platform-tests against the Velyr runtime")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the manifest (or the given files and directories) and report against the expectations.
    Run(RunArgs),
}

#[derive(clap::Args)]
struct RunArgs {
    /// Worker threads (one isolate each), at most 8.
    #[arg(long, default_value_t = 4)]
    threads: usize,
    /// Rewrite the expectations file from this run.
    #[arg(long)]
    update_expectations: bool,
    #[arg(long, default_value = "compat/vendor/wpt")]
    root: PathBuf,
    #[arg(long, default_value = "compat/wpt/manifest.txt")]
    manifest: PathBuf,
    #[arg(long, default_value = "compat/wpt/expectations.txt")]
    expectations: PathBuf,
    #[arg(long, default_value = "compat/wpt.lock")]
    lock: PathBuf,
    #[arg(long, default_value = "compat/results")]
    results_dir: PathBuf,
    /// Test files or directories relative to the WPT root; replaces the manifest selection.
    paths: Vec<String>,
}

#[derive(Serialize)]
struct Entry<'a> {
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtest: Option<&'a str>,
    class: Class,
    #[serde(skip_serializing_if = "String::is_empty")]
    detail: String,
}

fn main() -> ExitCode {
    let Command::Run(args) = Cli::parse().command;
    match run(&args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &RunArgs) -> Result<ExitCode, String> {
    let runner = Runner::load(&args.root)
        .map_err(|err| format!("{}: {err} (run scripts/fetch-wpt.sh)", args.root.display()))?;
    let entries = if args.paths.is_empty() {
        let text = fs::read_to_string(&args.manifest)
            .map_err(|err| format!("{}: {err}", args.manifest.display()))?;
        manifest::parse(&text)
    } else {
        args.paths.clone()
    };
    let files = manifest::expand(&args.root, &entries).map_err(|err| err.to_string())?;
    let units = run_all(&runner, &files, args.threads.clamp(1, MAX_THREADS));

    let previous = match fs::read_to_string(&args.expectations) {
        Ok(text) => report::parse_expectations(&text),
        Err(_) => Expectations::new(),
    };
    let classes: Vec<Class> = units
        .iter()
        .map(|unit| classify(&unit.outcome, previous.contains_key(&unit.key())))
        .collect();

    write_results(args, &files, &units, &classes)?;
    print_summary(&units, &classes);

    if args.update_expectations {
        let merged = report::merge_run(&previous, &units);
        fs::write(&args.expectations, report::render_expectations(&merged))
            .map_err(|err| format!("{}: {err}", args.expectations.display()))?;
        eprintln!(
            "wrote {} ({} entries)",
            args.expectations.display(),
            merged.len()
        );
        return Ok(ExitCode::SUCCESS);
    }
    Ok(if classes.iter().any(|class| class.is_unexpected()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn run_all(runner: &Runner, files: &[String], threads: usize) -> Vec<Unit> {
    let next = AtomicUsize::new(0);
    let mut indexed: Vec<(usize, Vec<Unit>)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(file) = files.get(index) else { break };
                        done.push((index, runner.run(file)));
                    }
                    done
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("worker thread"))
            .collect()
    });
    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().flat_map(|(_, units)| units).collect()
}

fn write_results(
    args: &RunArgs,
    files: &[String],
    units: &[Unit],
    classes: &[Class],
) -> Result<(), String> {
    let results: Vec<Entry> = units
        .iter()
        .zip(classes)
        .map(|(unit, class)| Entry {
            path: &unit.test,
            subtest: unit.subtest.as_deref(),
            class: *class,
            detail: report::reason(&unit.outcome),
        })
        .collect();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let sha = fs::read_to_string(&args.lock)
        .ok()
        .and_then(|lock| {
            lock.lines()
                .find_map(|l| l.strip_prefix("sha=").map(str::to_owned))
        })
        .unwrap_or_default();
    let doc = serde_json::json!({
        "suite": "wpt",
        "wpt_sha": sha,
        "timestamp": timestamp,
        "files": files.len(),
        "summary": tally(classes),
        "directories": report::directory_rates(units),
        "results": results,
    });
    fs::create_dir_all(&args.results_dir).map_err(|err| err.to_string())?;
    let file = args.results_dir.join(format!("wpt-{timestamp}.json"));
    let json = serde_json::to_string(&doc).map_err(|err| err.to_string())?;
    fs::write(&file, json).map_err(|err| format!("{}: {err}", file.display()))?;
    eprintln!("results: {}", file.display());
    Ok(())
}

fn tally(classes: &[Class]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for class in classes {
        let name = serde_json::to_value(class)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned));
        *counts.entry(name.unwrap_or_default()).or_insert(0) += 1;
    }
    counts
}

fn print_summary(units: &[Unit], classes: &[Class]) {
    let unexpected = units
        .iter()
        .zip(classes)
        .filter(|(_, class)| class.is_unexpected());
    for (unit, class) in unexpected.take(MAX_PRINTED_UNEXPECTED) {
        eprintln!(
            "{class:?}: {}: {}",
            unit.key(),
            report::reason(&unit.outcome)
        );
    }
    for (directory, rate) in report::directory_rates(units) {
        let name = if directory.is_empty() {
            "."
        } else {
            &directory
        };
        eprintln!(
            "{name}: {}/{} ({:.2}%)",
            rate.pass, rate.total, rate.pass_rate
        );
    }
    eprintln!("total {}: {:?}", units.len(), tally(classes));
}
