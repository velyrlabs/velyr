//! `velyr run --watch[=paths]`. A supervisor process re-executes this binary with the same
//! arguments (minus `--watch`) and restarts the child when a module of its import graph, the
//! entry, or one of the given paths changes. Flags and permissions are preserved because the
//! child gets the original command line.
//!
//! The child reports every module the resolver hands out to a file named by `VELYR_WATCH_REPORT`
//! (see [`recording`]); the supervisor polls that file and watches the directories of the modules.
//!
//! Known limits: the whole process restarts, not only the isolate; modules loaded by `require()`
//! of CommonJS do not go through the ESM resolver and are not in the graph; without the npm
//! resolver (profiles minimal and web) nothing is reported, so only the entry directory and the
//! given paths are watched; SIGKILL on the supervisor can leave its child running.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::sync::Arc;
use std::time::Duration;

use velyr_io_fs::FsWatcher;
use velyr_runtime_core::ModuleResolverHandle;
use velyr_runtime_modules::{ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result};

const REPORT_ENV: &str = "VELYR_WATCH_REPORT";
const REPORT_PREFIX: &str = "velyr-watch-";
/// How often the supervisor reads the graph report and checks the child.
const POLL: Duration = Duration::from_millis(200);

/// Wraps `inner` so that every resolved module path is appended to the supervisor's report. A
/// no-op unless this process was started by [`supervise`].
pub fn recording(inner: Option<ModuleResolverHandle>) -> Option<ModuleResolverHandle> {
    let report = std::env::var_os(REPORT_ENV).map(PathBuf::from);
    match (inner, report) {
        (Some(inner), Some(report)) if is_supervisor_report(&report) => {
            Some(Arc::new(Recorder { inner, report }))
        }
        (inner, _) => inner,
    }
}

/// Only a file [`create_report`] made is written to: a regular file named `velyr-watch-*` that
/// already exists directly in the system temp dir. The runtime strips the variable from every
/// spawn; this keeps a hand-set one from clobbering anything else.
fn is_supervisor_report(path: &Path) -> bool {
    let named = path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(REPORT_PREFIX));
    let in_temp = path
        .parent()
        .and_then(|dir| dir.canonicalize().ok())
        .is_some_and(|dir| std::env::temp_dir().canonicalize().ok() == Some(dir));
    let regular = path.symlink_metadata().is_ok_and(|meta| meta.is_file());
    named && in_temp && regular
}

/// A new, empty, owner-only report file with an unpredictable name.
fn create_report() -> std::result::Result<PathBuf, String> {
    use std::hash::{BuildHasher, Hasher};
    use std::os::unix::fs::OpenOptionsExt;
    let nonce = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let path = std::env::temp_dir().join(format!(
        "{REPORT_PREFIX}{}-{nonce:x}.txt",
        std::process::id()
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("error: cannot create the watch report: {e}"))?;
    Ok(path)
}

struct Recorder {
    inner: ModuleResolverHandle,
    report: PathBuf,
}

impl ModuleResolver for Recorder {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule> {
        let resolved = self.inner.resolve(specifier, referrer, ctx)?;
        if resolved.path.is_absolute()
            && let Ok(mut report) = std::fs::OpenOptions::new().append(true).open(&self.report)
        {
            let _ = writeln!(report, "{}", resolved.path.display());
        }
        Ok(resolved)
    }
}

/// Runs `file` under a supervisor until interrupted. `extra` are the paths of `--watch=a,b`.
pub fn supervise(file: &Path, extra: &[String]) -> ExitCode {
    match watch(file, extra) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn canonical(path: &Path) -> std::result::Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|e| format!("error: cannot watch {}: {e}", path.display()))
}

fn watch(file: &Path, extra: &[String]) -> std::result::Result<(), String> {
    let entry = canonical(file)?;
    let mut explicit = Vec::new();
    for path in extra.iter().filter(|path| !path.is_empty()) {
        explicit.push(canonical(Path::new(path))?);
    }
    let report = create_report()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("error: cannot start the watcher runtime: {e}"))?;
    let result = runtime.block_on(supervise_loop(&entry, &explicit, &report));
    let _ = std::fs::remove_file(&report);
    result
}

/// The command line of this process without `--watch` / `--watch=...` (arguments after `--`
/// belong to the script).
fn child_args() -> Vec<OsString> {
    let mut after_separator = false;
    std::env::args_os()
        .skip(1)
        .filter(|arg| {
            after_separator |= arg == "--";
            after_separator || !arg.to_string_lossy().starts_with("--watch")
        })
        .collect()
}

fn spawn(args: &[OsString], report: &Path) -> std::result::Result<Child, String> {
    let exe = std::env::current_exe().map_err(|e| format!("error: {e}"))?;
    Command::new(exe)
        .args(args)
        .env(REPORT_ENV, report)
        .spawn()
        .map_err(|e| format!("error: cannot start the script: {e}"))
}

fn stop(child: &mut Option<Child>) {
    if let Some(mut child) = child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Watches the directory of `module` (editors replace files, so the file itself is not enough).
fn watch_parent(watcher: &mut FsWatcher, dirs: &mut HashSet<PathBuf>, module: &Path) {
    if let Some(dir) = module.parent()
        && dirs.insert(dir.to_path_buf())
    {
        let _ = watcher.add(dir, false);
    }
}

async fn supervise_loop(
    entry: &Path,
    explicit: &[PathBuf],
    report: &Path,
) -> std::result::Result<(), String> {
    let args = child_args();
    let mut watcher = FsWatcher::open(&[], false).map_err(|e| format!("error: {e}"))?;
    let mut dirs = HashSet::new();
    let mut files = HashSet::from([entry.to_path_buf()]);
    for path in explicit {
        watcher
            .add(path, true)
            .map_err(|e| format!("error: cannot watch {}: {e}", path.display()))?;
    }
    watch_parent(&mut watcher, &mut dirs, entry);
    let mut child = Some(spawn(&args, report)?);
    let mut poll = tokio::time::interval(POLL);
    loop {
        tokio::select! {
            event = watcher.next() => {
                let Some(event) = event else { return Ok(()) };
                let relevant = event.paths.iter().any(|path| {
                    files.contains(path) || explicit.iter().any(|root| path.starts_with(root))
                });
                if relevant {
                    stop(&mut child);
                    eprintln!("[watch] change detected, restarting");
                    child = Some(spawn(&args, report)?);
                }
            }
            _ = poll.tick() => {
                for line in std::fs::read_to_string(report).unwrap_or_default().lines() {
                    let module = PathBuf::from(line);
                    if files.insert(module.clone()) {
                        watch_parent(&mut watcher, &mut dirs, &module);
                    }
                }
                let exited = match child.as_mut().map(Child::try_wait) {
                    Some(Ok(Some(status))) => Some(status),
                    _ => None,
                };
                if let Some(status) = exited {
                    child = None;
                    eprintln!("[watch] the script exited ({status}), waiting for changes");
                }
            }
            _ = tokio::signal::ctrl_c() => {
                stop(&mut child);
                return Ok(());
            }
        }
    }
}
