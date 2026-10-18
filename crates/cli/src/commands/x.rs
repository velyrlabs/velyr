//! `velyr x <bin> [args]`: runs `node_modules/.bin/<bin>` through velyr's Node compat layer.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use velyr_compat_npm::scripts::ensure_shim;
use velyr_runtime_permissions::Request;

use crate::node_shim;
use crate::permissions::Permissions;
use crate::run::{self, EngineKind, Worker};

const SCRIPT_EXTENSIONS: [&str; 6] = ["js", "mjs", "cjs", "ts", "mts", "cts"];

pub fn x(bin: &str, args: &[String], flags: Permissions, raw_flags: Vec<String>) -> ExitCode {
    match execute(bin, args, flags, &raw_flags) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn execute(
    bin: &str,
    args: &[String],
    flags: Permissions,
    raw_flags: &[String],
) -> Result<(), String> {
    if bin.is_empty() || bin == ".." || bin.contains(['/', '\\']) {
        return Err(format!(
            "error: `{bin}` is not a bin name (no path separators)"
        ));
    }
    let cwd = std::env::current_dir().map_err(|e| format!("error: cannot read the cwd: {e}"))?;
    let (project, link) = find_bin(&cwd, bin)?;
    let target = dunce::canonicalize(&link)
        .map_err(|e| format!("error: cannot resolve {}: {e}", link.display()))?;
    let root = dunce::canonicalize(&project).map_err(|e| format!("error: {e}"))?;
    // The entry directory is the project, so the import root covers its node_modules.
    let prepared = run::prepare(&root.join("x"), flags)?;
    if !target.starts_with(&root) {
        let path = target.to_string_lossy();
        if !prepared
            .capabilities
            .check(&Request::FsRead(&path))
            .is_allow()
        {
            return Err(format!(
                "error: `{bin}` points outside the project ({path}); allow it with --allow-read"
            ));
        }
    }
    let entry = entry_for(&root, bin, &target)?;
    let exe = std::env::current_exe().map_err(|e| format!("error: cannot find velyr: {e}"))?;
    let shim =
        ensure_shim(&root, &exe).map_err(|e| format!("error: cannot write node shim: {e}"))?;
    node_shim::pin_flags(raw_flags);
    let dirs = [shim, root.join("node_modules").join(".bin")];
    let script = format!(
        "process.argv.splice(1, process.argv.length, {}); {} {}",
        serde_json::to_string(&target.to_string_lossy()).unwrap_or_default(),
        node_shim::argv_script(args),
        node_shim::path_script(&dirs),
    );
    run::run_prepared(
        &entry,
        &prepared,
        Some(script),
        Worker::default(),
        EngineKind::default(),
    )
}

/// Walks up from `start` to the first `node_modules/.bin/<bin>`; returns the project and the link.
fn find_bin(start: &Path, bin: &str) -> Result<(PathBuf, PathBuf), String> {
    for dir in start.ancestors() {
        let link = dir.join("node_modules").join(".bin").join(bin);
        if link.symlink_metadata().is_ok() {
            return Ok((dir.to_path_buf(), link));
        }
    }
    Err(format!(
        "error: `{bin}` not found in node_modules/.bin (looked from {})",
        start.display()
    ))
}

/// The file to run for `target`: itself when it has a script extension, else (a bin without one)
/// a CommonJS wrapper under `node_modules/.velyr/x`. Only Node scripts are accepted.
fn entry_for(root: &Path, bin: &str, target: &Path) -> Result<PathBuf, String> {
    let mut head = [0u8; 256];
    let n = std::fs::File::open(target)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|e| format!("error: cannot read {}: {e}", target.display()))?;
    let head = String::from_utf8_lossy(&head[..n]);
    let shebang = head
        .strip_prefix("#!")
        .map(|r| r.lines().next().unwrap_or_default());
    let by_extension = target
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SCRIPT_EXTENSIONS.contains(&e));
    match shebang {
        Some(line) if !line.contains("node") => {
            return Err(format!(
                "error: `{bin}` is not a Node.js script (shebang `#!{line}`)"
            ));
        }
        None if !by_extension => {
            return Err(format!("error: `{bin}` is not a Node.js script"));
        }
        _ => {}
    }
    if by_extension {
        return Ok(target.to_path_buf());
    }
    let dir = root.join("node_modules").join(".velyr").join("x");
    std::fs::create_dir_all(&dir).map_err(|e| format!("error: {e}"))?;
    let wrapper = dir.join(format!("{bin}.cjs"));
    let path = serde_json::to_string(&target.to_string_lossy()).unwrap_or_default();
    std::fs::write(&wrapper, format!("require({path});\n"))
        .map_err(|e| format!("error: cannot write {}: {e}", wrapper.display()))?;
    Ok(wrapper)
}
