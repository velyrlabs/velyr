//! Dependency lifecycle scripts, denied by default (SPEC section 8, ADR-013).
//!
//! Only `preinstall`, `install`, `postinstall` and `prepare` are considered,
//! and only for packages the caller explicitly allows. Scripts run through the
//! system shell with a minimal environment; `node` on `PATH` is a shim
//! (`node_modules/.velyr/bin/node`) that re-enters the current `velyr` binary.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use velyr_runtime_errors::VelyrError;

use crate::package_json::PackageJson;

/// Events run in this order; every other script name is never run.
pub const LIFECYCLE: [&str; 4] = ["preinstall", "install", "postinstall", "prepare"];

const STATE_FILE: &str = ".velyr-scripts.json";

/// Which packages may run lifecycle scripts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Allow {
    #[default]
    None,
    /// Package names; `.` stands for the root project.
    Packages(BTreeSet<String>),
    All,
}

impl Allow {
    pub fn permits(&self, name: &str, root: bool) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Packages(set) => set.contains(name) || (root && set.contains(".")),
        }
    }
}

/// A package whose scripts may run.
#[derive(Debug, Clone)]
pub struct Target {
    pub name: String,
    pub version: String,
    pub dir: PathBuf,
    pub root: bool,
}

impl Target {
    pub fn label(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

/// A package with scripts that were not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub label: String,
    pub name: String,
    pub events: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    /// Labels (`name@version`) whose scripts ran now.
    pub ran: Vec<String>,
    pub skipped: Vec<Skipped>,
}

fn state_path(project: &Path) -> PathBuf {
    project.join("node_modules").join(STATE_FILE)
}

fn load_state(project: &Path) -> BTreeSet<String> {
    std::fs::read(state_path(project))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_state(project: &Path, state: &BTreeSet<String>) -> Result<(), VelyrError> {
    let bytes = serde_json::to_vec(state)
        .map_err(|e| VelyrError::internal("scripts state").with_cause(e))?;
    std::fs::write(state_path(project), bytes)
        .map_err(|e| VelyrError::internal("cannot write scripts state").with_cause(e))
}

/// Marks packages as not yet scripted (they were just reinstalled).
pub fn forget(project: &Path, labels: &[String]) {
    let mut state = load_state(project);
    let before = state.len();
    for l in labels {
        state.remove(l);
    }
    if state.len() != before {
        let _ = save_state(project, &state);
    }
}

fn events_of(dir: &Path) -> Vec<(&'static str, String)> {
    let Ok(bytes) = std::fs::read(dir.join("package.json")) else {
        return Vec::new();
    };
    let Ok(pkg) = serde_json::from_slice::<PackageJson>(&bytes) else {
        return Vec::new();
    };
    LIFECYCLE
        .iter()
        .filter_map(|e| pkg.scripts.get(*e).map(|c| (*e, c.clone())))
        .collect()
}

/// Env var the shim's `velyr run --node-shim` reads its permission flags from (joined by
/// `\x1f`). Absent means no permissions: the shim never grants authority by itself.
pub const SHIM_FLAGS_ENV: &str = "VELYR_NODE_SHIM_FLAGS";

/// Where the `node` shim of `project` lives; `velyr x` and lifecycle scripts put it on `PATH`.
pub fn shim_dir(project: &Path) -> PathBuf {
    project.join("node_modules").join(".velyr").join("bin")
}

/// Writes the `node` shim (`node` and `node.cmd` on Windows) re-entering `exe` as
/// `velyr run --node-shim <script> -- <args>`; returns its directory.
pub fn ensure_shim(project: &Path, exe: &Path) -> std::io::Result<PathBuf> {
    let dir = shim_dir(project);
    std::fs::create_dir_all(&dir)?;
    write_shim(&dir, exe)?;
    Ok(dir)
}

#[cfg(unix)]
fn write_shim(dir: &Path, exe: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("node");
    let exe = exe.display().to_string().replace('\'', r"'\''");
    let body = format!(
        "#!/bin/sh\n[ $# -gt 0 ] || {{ echo 'node: missing script' >&2; exit 1; }}\n\
         f=\"$1\"\nshift\nexec '{exe}' run --node-shim \"$f\" -- \"$@\"\n"
    );
    if std::fs::read_to_string(&path).is_ok_and(|current| current == body) {
        return Ok(());
    }
    std::fs::write(&path, body)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(windows)]
fn write_shim(dir: &Path, exe: &Path) -> std::io::Result<()> {
    // shortcut: cmd forwards at most 8 script arguments; upgrade with a real launcher.
    std::fs::write(
        dir.join("node.cmd"),
        format!(
            "@\"{}\" run --node-shim %1 -- %2 %3 %4 %5 %6 %7 %8 %9\r\n",
            exe.display()
        ),
    )
}

fn shell(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

fn exec(
    project: &Path,
    target: &Target,
    event: &str,
    command: &str,
    shim: Option<&Path>,
) -> Result<(), VelyrError> {
    let mut dirs = vec![
        target.dir.join("node_modules").join(".bin"),
        project.join("node_modules").join(".bin"),
    ];
    dirs.extend(shim.map(Path::to_path_buf));
    dirs.extend(
        std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths),
    );
    let path = std::env::join_paths(dirs)
        .map_err(|e| VelyrError::internal("invalid PATH for lifecycle script").with_cause(e))?;
    let mut cmd = shell(command);
    cmd.current_dir(&target.dir)
        .env_clear()
        .env("npm_package_name", &target.name)
        .env("npm_lifecycle_event", event)
        .env("PATH", path)
        // The script already runs with the user's full shell authority, so `node` in it gets
        // `-A`; this is the only place the shim is handed flags (an explicit grant).
        .env(SHIM_FLAGS_ENV, "-A")
        .stdin(Stdio::null());
    if let Some(home) = std::env::var_os("HOME") {
        cmd.env("HOME", home);
    }
    let status = cmd.status().map_err(|e| {
        VelyrError::internal(format!("{} {event}: cannot start shell", target.label()))
            .with_cause(e)
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(VelyrError::internal(format!(
            "{} {event} script failed ({status})",
            target.label()
        )))
    }
}

/// Runs the allowed targets' lifecycle scripts and lists the ones left out.
///
/// Dependencies are expected before the root target. Dependency scripts run
/// once per installed `name@version` (see [`forget`]); root scripts run on
/// every call.
pub fn run(
    project: &Path,
    targets: &[Target],
    allow: &Allow,
    velyr_exe: Option<&Path>,
) -> Result<Report, VelyrError> {
    let mut state = load_state(project);
    let mut report = Report::default();
    let mut shim: Option<PathBuf> = None;
    for target in targets {
        let label = target.label();
        if !target.root && state.contains(&label) {
            continue;
        }
        let events = events_of(&target.dir);
        if events.is_empty() {
            continue;
        }
        if !allow.permits(&target.name, target.root) {
            report.skipped.push(Skipped {
                label,
                name: target.name.clone(),
                events: events.iter().map(|(e, _)| (*e).to_owned()).collect(),
            });
            continue;
        }
        if let (None, Some(exe)) = (&shim, velyr_exe) {
            shim = Some(
                ensure_shim(project, exe)
                    .map_err(|e| VelyrError::internal("cannot write node shim").with_cause(e))?,
            );
        }
        for (event, command) in &events {
            exec(project, target, event, command, shim.as_deref())?;
        }
        if !target.root {
            state.insert(label.clone());
            save_state(project, &state)?;
        }
        report.ran.push(label);
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
