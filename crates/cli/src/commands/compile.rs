//! `velyr compile`: a copy of this binary with the entry's module graph, the `--include` files and
//! the permission flags appended as a hashed payload (`velyr-runtime-vfs`). At startup every
//! `velyr` binary looks at its own tail first: with a payload it runs the embedded entry, serving
//! modules from memory, and the sources on disk are not needed.

mod graph;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser};
use velyr_runtime_core::RuntimeConfig;
use velyr_runtime_vfs::{Payload, Vfs, VfsLoader, VfsResolver};

use crate::permissions::Permissions;
use crate::run::{self, EngineKind};
use crate::telemetry;

use graph::{Graph, canonical};

/// Where the embedded files are readable at run time (`Velyr.readFile`, `require`): baked into the
/// permissions so a `--deny-read` at run time still wins.
const READ_ROOT: &str = "/$velyr/app";

#[derive(Args)]
pub struct CompileArgs {
    /// Entry module.
    file: PathBuf,

    /// Output executable; defaults to the entry's name without extension.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Extra files to embed, as glob patterns relative to the working directory
    /// (for example `assets/**`).
    #[arg(long, value_delimiter = ',')]
    include: Vec<String>,

    /// Embed imported files that live outside the entry's folder (other than a `node_modules`).
    #[arg(long)]
    allow_outside_root: bool,

    /// Target triple; only the host target is supported.
    #[arg(long)]
    target: Option<String>,

    /// Permission flags are fixed in the executable; at run time it only accepts `--deny-*`.
    #[command(flatten)]
    permissions: Permissions,
}

pub fn compile(args: CompileArgs) -> ExitCode {
    match build(&args) {
        Ok(output) => {
            eprintln!("compiled {}", output.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn build(args: &CompileArgs) -> Result<PathBuf, String> {
    if let Some(target) = &args.target
        && Some(target.as_str()) != host_triple()
    {
        return Err(format!(
            "error: --target {target} is not supported yet (cross-compiling needs that target's velyr binary); omit --target to compile for this host"
        ));
    }
    let exe = std::env::current_exe().map_err(|e| format!("error: current executable: {e}"))?;
    if Payload::read_from(&exe)
        .map_err(|e| format!("error: {e}"))?
        .is_some()
    {
        return Err(
            "error: this executable already carries a payload; compile with a plain velyr".into(),
        );
    }
    let entry = canonical(&args.file)?;
    let root = entry
        .parent()
        .ok_or("error: the entry has no parent directory")?
        .to_path_buf();
    let mut graph = Graph::new(root, args.allow_outside_root);
    graph.add_module(&entry)?;
    for pattern in &args.include {
        graph.add_assets(pattern)?;
    }
    let mut permissions = args.permissions.clone();
    if !permissions.allow_all {
        merge_scope(
            &mut permissions.allow_read,
            Some(vec![READ_ROOT.to_owned()]),
        );
    }
    let payload = Payload {
        entry: graph.virtual_path(&entry),
        permission_args: flag_args(&permissions),
        files: graph.files,
        edges: graph.edges,
    };
    let host = std::fs::read(&exe).map_err(|e| format!("error: {}: {e}", exe.display()))?;
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| default_output(&args.file));
    write_executable(&output, &payload.append_to(host))?;
    Ok(output)
}

/// Triple of the running binary, for the targets velyr ships.
fn host_triple() -> Option<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => Some("aarch64-apple-darwin"),
        ("x86_64", "macos") => Some("x86_64-apple-darwin"),
        ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu"),
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
        _ => None,
    }
}

fn default_output(entry: &Path) -> PathBuf {
    PathBuf::from(entry.file_stem().unwrap_or_default())
}

fn write_executable(output: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(output, bytes).map_err(|e| format!("error: {}: {e}", output.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("error: {}: {e}", output.display()))?;
    }
    // The payload sits after the signed Mach-O image, which macOS still runs (`codesign -v` rejects
    // it; re-signing is not possible with trailing data).
    Ok(())
}

/// The permission flags as the command line spelled them.
fn flag_args(p: &Permissions) -> Vec<String> {
    let mut args = Vec::new();
    if p.allow_all {
        args.push("--allow-all".to_owned());
    }
    let scopes = [
        ("allow-read", &p.allow_read),
        ("allow-write", &p.allow_write),
        ("allow-net", &p.allow_net),
        ("allow-listen", &p.allow_listen),
        ("allow-env", &p.allow_env),
        ("allow-run", &p.allow_run),
        ("allow-native", &p.allow_native),
        ("allow-import", &p.allow_import),
        ("inspect", &p.inspect),
        ("deny-read", &p.deny_read),
        ("deny-write", &p.deny_write),
        ("deny-net", &p.deny_net),
        ("deny-listen", &p.deny_listen),
        ("deny-env", &p.deny_env),
        ("deny-run", &p.deny_run),
        ("deny-native", &p.deny_native),
        ("deny-import", &p.deny_import),
        ("deny-inspect", &p.deny_inspect),
    ];
    for (name, scope) in scopes {
        match scope {
            None => {}
            Some(values) if values.is_empty() => args.push(format!("--{name}")),
            Some(values) => args.push(format!("--{name}={}", values.join(","))),
        }
    }
    args
}

/// Serves the embedded files when this process is a compiled executable. Imports resolve through
/// the recorded edges and modules load from memory; `velyr.json` on disk is not consulted.
pub fn install_vfs(config: &mut RuntimeConfig) {
    if let Some(vfs) = velyr_runtime_vfs::installed() {
        let resolver = VfsResolver::new(vfs.clone(), config.module_resolver.take());
        config.module_resolver = Some(Arc::new(resolver));
        config.module_loader = Some(Arc::new(VfsLoader(vfs.clone())));
    }
}

pub fn is_embedded() -> bool {
    velyr_runtime_vfs::installed().is_some()
}

/// `Some(exit code)` when this binary carries a payload (it ran, or refused to); `None` for a plain
/// velyr. A payload whose hash does not match is never executed.
pub fn run_embedded() -> Option<ExitCode> {
    let exe = std::env::current_exe().ok()?;
    let payload = match Payload::read_from(&exe) {
        Ok(None) => return None,
        Ok(Some(payload)) => payload,
        Err(error) => {
            eprintln!("error: {error}; refusing to run");
            return Some(ExitCode::FAILURE);
        }
    };
    let flags = match runtime_flags(&payload.permission_args, std::env::args().skip(1)) {
        Ok(flags) => flags,
        Err(message) => {
            eprintln!("{message}");
            return Some(ExitCode::FAILURE);
        }
    };
    let entry = PathBuf::from(&payload.entry);
    if !velyr_runtime_vfs::install(Arc::new(Vfs::from_payload(payload))) {
        return None;
    }
    telemetry::init(telemetry::LogFormat::default());
    let code = run::run(&entry, flags, EngineKind::V8, None);
    telemetry::shutdown();
    Some(code)
}

#[derive(Parser)]
#[command(no_binary_name = true)]
struct FlagsOnly {
    #[command(flatten)]
    permissions: Permissions,
}

fn parse_flags(args: impl IntoIterator<Item = String>) -> Permissions {
    FlagsOnly::try_parse_from(args)
        .unwrap_or_else(|e| e.exit())
        .permissions
}

fn only_denies(p: &Permissions) -> Permissions {
    Permissions {
        deny_read: p.deny_read.clone(),
        deny_write: p.deny_write.clone(),
        deny_net: p.deny_net.clone(),
        deny_listen: p.deny_listen.clone(),
        deny_env: p.deny_env.clone(),
        deny_run: p.deny_run.clone(),
        deny_native: p.deny_native.clone(),
        deny_import: p.deny_import.clone(),
        deny_inspect: p.deny_inspect.clone(),
        ..Permissions::default()
    }
}

/// An empty list means "everything"; it wins over any list.
fn merge_scope(base: &mut Option<Vec<String>>, extra: Option<Vec<String>>) {
    *base = match (base.take(), extra) {
        (Some(a), Some(b)) if a.is_empty() || b.is_empty() => Some(Vec::new()),
        (Some(mut a), Some(b)) => {
            a.extend(b);
            Some(a)
        }
        (a, b) => a.or(b),
    };
}

/// The baked flags plus the `--deny-*` flags of this run; a flag that grants anything is refused.
fn runtime_flags(
    baked: &[String],
    user: impl IntoIterator<Item = String>,
) -> Result<Permissions, String> {
    let user = parse_flags(user);
    if user != only_denies(&user) {
        return Err("error: this executable's permissions are fixed at compile time; only --deny-* flags are accepted".into());
    }
    let mut flags = parse_flags(baked.iter().cloned());
    merge_scope(&mut flags.deny_read, user.deny_read);
    merge_scope(&mut flags.deny_write, user.deny_write);
    merge_scope(&mut flags.deny_net, user.deny_net);
    merge_scope(&mut flags.deny_listen, user.deny_listen);
    merge_scope(&mut flags.deny_env, user.deny_env);
    merge_scope(&mut flags.deny_run, user.deny_run);
    merge_scope(&mut flags.deny_native, user.deny_native);
    merge_scope(&mut flags.deny_import, user.deny_import);
    merge_scope(&mut flags.deny_inspect, user.deny_inspect);
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::graph::scan_specifiers;
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn scans_static_dynamic_and_require_specifiers() {
        let source = "import a from './a.ts';\nimport './side.js';\nexport * from \"./b\";\nconst c = await import('./c.js');\nconst d = require('d');\nimport { e } from 'pkg/e';\nconst f = createRequire(import.meta.url)('./f.json');";
        assert_eq!(
            scan_specifiers(source),
            [
                "./a.ts",
                "./side.js",
                "./b",
                "./c.js",
                "d",
                "pkg/e",
                "./f.json"
            ]
        );
    }

    #[test]
    fn flags_round_trip_through_their_spelling() {
        let flags = parse_flags(strings(&[
            "--allow-env=HOME,PATH",
            "--allow-read",
            "--deny-net=evil.test",
        ]));
        assert_eq!(parse_flags(flag_args(&flags)), flags);
    }

    #[test]
    fn runtime_accepts_only_deny_flags_and_merges_them() {
        let baked = strings(&["--allow-env=HOME", "--deny-read=/etc"]);
        let flags = runtime_flags(&baked, strings(&["--deny-read=/var", "--deny-env"])).unwrap();
        assert_eq!(flags.allow_env, Some(strings(&["HOME"])));
        assert_eq!(flags.deny_read, Some(strings(&["/etc", "/var"])));
        assert_eq!(flags.deny_env, Some(Vec::new()));
        assert!(runtime_flags(&baked, strings(&["--allow-net"])).is_err());
        assert!(runtime_flags(&baked, strings(&["--allow-all"])).is_err());
    }
}
