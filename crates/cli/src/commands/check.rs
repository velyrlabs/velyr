//! `velyr check`: type checking is not part of the runtime (ADR-012). This runs the TypeScript the
//! project installed, as `velyr x tsc --noEmit [files]`, and exits with its status.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(clap::Args)]
pub struct CheckArgs {
    /// Check only these files. `tsc` then ignores tsconfig.json, as it does on its command line.
    #[arg(long, num_args = 1.., value_name = "FILE")]
    files: Vec<PathBuf>,
}

const NOT_INSTALLED: &str = "error: TypeScript is not installed in this project (node_modules/.bin/tsc not found). \
velyr does not bundle a type checker; install one with `velyr add -D typescript`";

fn has_tsc(start: &Path) -> bool {
    start
        .ancestors()
        .any(|dir| dir.join("node_modules/.bin/tsc").symlink_metadata().is_ok())
}

/// `velyr x` grants tsc what it needs to read the project and its environment, nothing else.
fn tsc_command(exe: &Path, files: &[PathBuf]) -> Command {
    let mut command = Command::new(exe);
    command
        .args(["x", "--allow-read", "--allow-env", "tsc", "--noEmit"])
        .args(files);
    command
}

pub fn check(args: CheckArgs) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    if !has_tsc(&cwd) {
        eprintln!("{NOT_INSTALLED}");
        return ExitCode::FAILURE;
    }
    let status = std::env::current_exe().and_then(|exe| tsc_command(&exe, &args.files).status());
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: cannot run tsc: {error}");
            ExitCode::FAILURE
        }
    }
}
