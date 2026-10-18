//! `velyr-test262`: runs tc39/test262 against an engine adapter and diffs the result
//! with the committed expectation manifest (ADR-015).

mod frontmatter;
mod host262;
mod report;
mod runner;
mod shard;

use std::collections::BTreeMap;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, ValueEnum};
use serde::Serialize;
use velyr_engine_api::Engine;
use velyr_engine_native::NativeEngine;
use velyr_engine_v8::V8Engine;

use report::{Class, Expectations, Outcome, classify};
use runner::{Harness, TestCase, run_test};
use shard::Shard;

const MAX_THREADS: usize = 8;
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PRINTED_UNEXPECTED: usize = 40;

#[derive(Clone, Copy, ValueEnum)]
enum EngineKind {
    V8,
    Native,
}

impl EngineKind {
    fn name(self) -> &'static str {
        match self {
            Self::V8 => "v8",
            Self::Native => "native",
        }
    }
}

#[derive(Parser)]
#[command(about = "Run test262 against a Velyr engine")]
struct Args {
    #[arg(long, value_enum)]
    engine: EngineKind,
    /// Worker threads (one isolate each), at most 8.
    #[arg(long, default_value_t = MAX_THREADS)]
    threads: usize,
    /// Run only shard N of M (1 <= N <= M): a test belongs to the shard its path hashes to, so
    /// the split is the same on every machine.
    #[arg(long, value_name = "N/M")]
    shard: Option<Shard>,
    /// Rewrite the engine's expectation manifest from this run.
    #[arg(long)]
    update_expectations: bool,
    /// With --update-expectations, overwrite stale reasons (except `post-parity:` ones).
    #[arg(long, requires = "update_expectations")]
    refresh_reasons: bool,
    #[arg(long, default_value = "compat/vendor/test262")]
    root: PathBuf,
    #[arg(long, default_value = "compat/test262/expectations")]
    expectations_dir: PathBuf,
    #[arg(long, default_value = "compat/results")]
    results_dir: PathBuf,
    /// Files or directories relative to the test262 root.
    #[arg(default_value = "test")]
    paths: Vec<PathBuf>,
}

#[derive(Serialize)]
struct Entry<'a> {
    path: &'a str,
    class: Class,
    #[serde(skip_serializing_if = "String::is_empty")]
    detail: String,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Args) -> Result<ExitCode, String> {
    let threads = args.threads.clamp(1, MAX_THREADS);
    let harness = Harness::load(&args.root.join("harness")).map_err(|err| {
        format!(
            "{}: {err} (run scripts/fetch-test262.sh)",
            args.root.display()
        )
    })?;
    let mut files = Vec::new();
    for path in &args.paths {
        collect_tests(&args.root.join(path), &mut files)
            .map_err(|err| format!("{}: {err}", path.display()))?;
    }
    files.sort();
    files.dedup();
    if let Some(shard) = args.shard {
        files.retain(|file| shard.contains(&relative(&args.root, file)));
    }

    panic::set_hook(Box::new(|_| {}));
    let outcomes = match args.engine {
        EngineKind::V8 => run_all(&V8Engine::new(), &harness, &files, threads),
        EngineKind::Native => run_all(&NativeEngine, &harness, &files, threads),
    };
    let run: Vec<(String, Outcome)> = files
        .iter()
        .map(|file| relative(&args.root, file))
        .zip(outcomes)
        .collect();

    let manifest = args
        .expectations_dir
        .join(format!("{}.txt", args.engine.name()));
    let previous = match fs::read_to_string(&manifest) {
        Ok(text) => report::parse_expectations(&text),
        Err(_) => Expectations::new(),
    };
    let classes: Vec<Class> = run
        .iter()
        .map(|(path, outcome)| classify(outcome, previous.contains_key(path)))
        .collect();

    write_results(&args, &run, &classes)?;
    print_summary(&run, &classes);

    if args.update_expectations {
        fs::create_dir_all(&args.expectations_dir).map_err(|e| e.to_string())?;
        let mode = if args.refresh_reasons {
            report::MergeMode::RefreshReasons
        } else {
            report::MergeMode::KeepReasons
        };
        let merged = report::merge_run(&previous, &run, mode);
        fs::write(&manifest, report::render_expectations(&merged)).map_err(|e| e.to_string())?;
        eprintln!("wrote {} ({} entries)", manifest.display(), merged.len());
        return Ok(ExitCode::SUCCESS);
    }
    Ok(if classes.iter().any(|c| c.is_unexpected()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn collect_tests(path: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_file() {
        out.push(path.to_owned());
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        let is_js = path.extension().is_some_and(|ext| ext == "js");
        if path.is_dir() {
            collect_tests(&path, out)?;
        } else if is_js && !path.to_string_lossy().ends_with("_FIXTURE.js") {
            out.push(path);
        }
    }
    Ok(())
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .into_owned()
}

fn run_all<E: Engine + Sync>(
    engine: &E,
    harness: &Harness,
    files: &[PathBuf],
    threads: usize,
) -> Vec<Outcome> {
    let next = AtomicUsize::new(0);
    let mut indexed: Vec<(usize, Outcome)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(file) = files.get(index) else { break };
                        done.push((index, run_file(engine, harness, file)));
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
    indexed.into_iter().map(|(_, outcome)| outcome).collect()
}

fn run_file<E: Engine>(engine: &E, harness: &Harness, file: &Path) -> Outcome {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) => return Outcome::Crash(format!("read: {err}")),
    };
    let abs_path = dunce::canonicalize(file).unwrap_or_else(|_| file.to_owned());
    let case = TestCase { abs_path, text };
    panic::catch_unwind(AssertUnwindSafe(|| {
        run_test(engine, harness, &case, TEST_TIMEOUT)
    }))
    .unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or(payload.downcast_ref::<&str>().copied())
            .unwrap_or("panic");
        Outcome::Crash(message.to_owned())
    })
}

fn write_results(args: &Args, run: &[(String, Outcome)], classes: &[Class]) -> Result<(), String> {
    let entries: Vec<Entry> = run
        .iter()
        .zip(classes)
        .map(|((path, outcome), class)| Entry {
            path,
            class: *class,
            detail: report::reason(outcome),
        })
        .collect();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let sha = fs::read_to_string(args.root.join(".velyr-sha")).unwrap_or_default();
    let doc = serde_json::json!({
        "engine": args.engine.name(),
        "test262_sha": sha.trim(),
        "timestamp": ts,
        "paths": args.paths,
        "summary": tally(classes),
        "results": entries,
    });
    fs::create_dir_all(&args.results_dir).map_err(|e| e.to_string())?;
    let file = args
        .results_dir
        .join(format!("test262-{}-{ts}.json", args.engine.name()));
    let json = serde_json::to_string(&doc).map_err(|e| e.to_string())?;
    fs::write(&file, json).map_err(|e| format!("{}: {e}", file.display()))?;
    eprintln!("results: {}", file.display());
    Ok(())
}

fn tally(classes: &[Class]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for class in classes {
        let name = serde_json::to_value(class)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned));
        *counts.entry(name.unwrap_or_default()).or_insert(0) += 1;
    }
    counts
}

fn print_summary(run: &[(String, Outcome)], classes: &[Class]) {
    let unexpected = run
        .iter()
        .zip(classes)
        .filter(|(_, class)| class.is_unexpected());
    for ((path, outcome), class) in unexpected.take(MAX_PRINTED_UNEXPECTED) {
        eprintln!("{class:?}: {path}: {}", report::reason(outcome));
    }
    eprintln!("total {}: {:?}", run.len(), tally(classes));
}
