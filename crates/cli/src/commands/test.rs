//! `velyr test`: finds the test files, runs each one in its own `velyr run` child (one isolate per
//! file, up to `--jobs` at a time) with exactly the permission flags of this command line, and
//! reports the results.
//!
//! A test that runs past `--timeout` fails (a test that blocks its isolate gets its file killed,
//! so the tests after it in that file do not run) and a file past `--file-timeout` fails; the
//! other files go on. The pretty and tap reports are printed file by file as the files end, junit
//! and json at the end. `--watch` restarts the whole run when anything under the searched paths
//! changes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Mutex;
use std::time::Instant;

use velyr_tooling_test::{
    FileResult, Limits, Reporter, default_jobs, discover, render, run_files, stream_end,
    stream_file, stream_start,
};

use crate::permissions::Permissions;
use crate::watch_mode;

#[derive(clap::Args)]
pub struct TestArgs {
    /// Test files, or directories searched for `*.test.*`, `*.spec.*` (ts, tsx, js, mjs) and
    /// `__tests__` files. Default: the current directory.
    paths: Vec<PathBuf>,

    /// Run only the tests whose full name ("suite > test") contains TEXT.
    #[arg(long, value_name = "TEXT")]
    filter: Option<String>,

    /// How results are printed: pretty, tap, junit or json.
    #[arg(long, default_value = "pretty", value_name = "NAME")]
    reporter: Reporter,

    /// Files run at once (default: half the CPUs, at most 4; each one is an isolate).
    #[arg(long, short = 'j', value_name = "N")]
    jobs: Option<usize>,

    /// Milliseconds one test may take. One that overruns fails; the rest of its file does not run.
    #[arg(long, default_value_t = 5000, value_name = "MS")]
    timeout: u64,

    /// Milliseconds one whole test file may take before it is killed and fails.
    #[arg(long, default_value_t = 60_000, value_name = "MS")]
    file_timeout: u64,

    /// Rerun everything when a test file, something it imports or anything under the searched
    /// paths changes.
    #[arg(long)]
    watch: bool,

    #[command(flatten)]
    #[allow(dead_code)]
    // The child gets the raw flags of the command line, see `permission_flags`.
    permissions: Permissions,
}

/// The permission flags of this command line, as written. They all take their value after `=`,
/// so one argument is one flag. Children get these and nothing more.
fn permission_flags(argv: &[OsString]) -> Vec<OsString> {
    argv.iter()
        .skip(1)
        .skip_while(|arg| *arg != "test")
        .skip(1)
        .filter(|arg| {
            let text = arg.to_string_lossy();
            text == "-A"
                || text.starts_with("--allow-")
                || text.starts_with("--deny-")
                || text == "--inspect"
                || text.starts_with("--inspect=")
        })
        .cloned()
        .collect()
}

/// `velyr run <flags> <file> -- --velyr-test [--velyr-test-filter=TEXT]`.
fn child_command(
    exe: &Path,
    flags: &[OsString],
    filter: Option<&str>,
    timeout_ms: u64,
    file: &Path,
) -> Command {
    let mut command = Command::new(exe);
    command
        .arg("run")
        .args(flags)
        .arg(file)
        .arg("--")
        .arg("--velyr-test")
        .arg(format!("--velyr-test-timeout={timeout_ms}"));
    if let Some(filter) = filter {
        command.arg(format!("--velyr-test-filter={filter}"));
    }
    command
}

pub fn test(args: TestArgs) -> ExitCode {
    #[cfg(not(feature = "node"))]
    {
        let _ = args;
        eprintln!("error: `velyr test` needs the node profile");
        ExitCode::FAILURE
    }
    #[cfg(feature = "node")]
    {
        match execute(args) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(feature = "node")]
fn execute(args: TestArgs) -> Result<ExitCode, String> {
    let roots = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths
    };
    if args.watch {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let searched: Vec<String> = roots
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        return Ok(watch_mode::supervise(&cwd, &searched));
    }
    let files: Vec<PathBuf> = discover(&roots)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|file| {
            file.strip_prefix(".")
                .map(Path::to_path_buf)
                .unwrap_or(file)
        })
        .collect();
    if files.is_empty() {
        return Err("no test files found".into());
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot find velyr: {e}"))?;
    let flags = permission_flags(&std::env::args_os().collect::<Vec<_>>());
    let limits = Limits {
        test_ms: args.timeout,
        file_ms: args.file_timeout,
    };
    let reporter = args.reporter;
    // Streaming reporters print each file as it ends; `printed` numbers the tap results and
    // keeps two files from interleaving. Whatever the tests printed goes to stderr, so a
    // machine-readable report stays clean.
    let printed = Mutex::new(0);
    let on_done = |result: &FileResult| {
        let mut number = printed.lock().expect("printing lock");
        if reporter.streams() {
            print!("{}", stream_file(reporter, result, &mut number));
        }
        if !result.output.trim().is_empty() {
            eprintln!(
                "--- output of {} ---\n{}",
                result.file,
                result.output.trim_end()
            );
        }
    };
    let started = Instant::now();
    print!("{}", stream_start(reporter));
    let results = run_files(
        &files,
        args.jobs.unwrap_or_else(default_jobs),
        limits,
        |file| child_command(&exe, &flags, args.filter.as_deref(), args.timeout, file),
        on_done,
    );
    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if reporter.streams() {
        let number = *printed.lock().expect("printing lock");
        print!("{}", stream_end(reporter, &results, number, elapsed));
    } else {
        print!("{}", render(reporter, &results, elapsed));
    }
    Ok(if results.iter().any(|r| r.failed()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn children_get_the_permission_flags_and_nothing_else() {
        let flags = permission_flags(&argv(&[
            "velyr",
            "--log-format",
            "json",
            "test",
            "--allow-read=.",
            "dir",
            "--filter",
            "x",
            "--deny-net=evil.test",
            "-A",
            "--reporter=tap",
            "--inspect=127.0.0.1:9229",
        ]));
        assert_eq!(
            flags,
            argv(&[
                "--allow-read=.",
                "--deny-net=evil.test",
                "-A",
                "--inspect=127.0.0.1:9229"
            ])
        );
    }

    #[test]
    fn the_child_command_marks_the_run_and_carries_the_filter() {
        let command = child_command(
            Path::new("/bin/velyr"),
            &argv(&["--allow-read=."]),
            Some("adds"),
            750,
            Path::new("a.test.js"),
        );
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "run",
                "--allow-read=.",
                "a.test.js",
                "--",
                "--velyr-test",
                "--velyr-test-timeout=750",
                "--velyr-test-filter=adds"
            ]
        );
    }
}
